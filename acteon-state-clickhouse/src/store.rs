use std::time::Duration;

use async_trait::async_trait;

use acteon_state::error::StateError;
use acteon_state::key::{KeyKind, StateKey};
use acteon_state::store::{CasResult, StateStore};

use crate::config::ClickHouseConfig;
use crate::migrations;

/// Row type used when reading the `value` column from the state table.
#[derive(clickhouse::Row, serde::Deserialize)]
struct ValueRow {
    value: String,
}

/// Row type used when reading `value` and `version` from the state table.
#[derive(clickhouse::Row, serde::Deserialize)]
struct ValueVersionRow {
    value: String,
    version: u64,
}

/// Row type used when reading the `version` column from the state table.
#[derive(clickhouse::Row, serde::Deserialize)]
struct VersionRow {
    version: u64,
}

/// Row type used for key-value pairs in scan results.
#[derive(clickhouse::Row, serde::Deserialize)]
struct KeyValueRow {
    key: String,
    value: String,
}

/// Row type used when inserting into the state table.
#[derive(clickhouse::Row, serde::Serialize)]
struct StateRow {
    key: String,
    value: String,
    version: u64,
    is_deleted: u8,
    expires_at: Option<i64>,
}

/// Row type used when inserting into the timeout index table.
#[derive(clickhouse::Row, serde::Serialize)]
struct TimeoutIndexRow {
    key: String,
    expires_at_ms: i64,
    version: u64,
    is_deleted: u8,
}

/// Row type used for reading keys from timeout index.
#[derive(clickhouse::Row, serde::Deserialize)]
struct TimeoutKeyRow {
    key: String,
}

/// Row type used for reading version from timeout index.
#[derive(clickhouse::Row, serde::Deserialize)]
struct TimeoutVersionRow {
    version: u64,
}

/// `ClickHouse`-backed implementation of [`StateStore`].
///
/// Uses a `ReplacingMergeTree(version)` table to emulate mutable state on top
/// of `ClickHouse`'s append-only storage. All reads use the `FINAL` keyword to
/// obtain the deduplicated (latest-version) view of each key.
///
/// TTL is handled via an `expires_at` column: reads filter out expired rows
/// with `WHERE (expires_at IS NULL OR expires_at > now64(3))`.
///
/// Soft deletes are indicated by `is_deleted = 1`. Deleted and expired rows
/// are excluded from all read operations.
pub struct ClickHouseStateStore {
    client: clickhouse::Client,
    state_table: String,
    timeout_index_table: String,
}

impl ClickHouseStateStore {
    /// Create a new `ClickHouseStateStore` from the provided configuration.
    ///
    /// Connects to `ClickHouse` and runs migrations to ensure the required
    /// tables exist.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::Connection`] if the client cannot connect or
    /// migrations fail.
    pub async fn new(config: ClickHouseConfig) -> Result<Self, StateError> {
        let client = clickhouse::Client::default()
            .with_url(&config.url)
            .with_database(&config.database);

        migrations::run_migrations(&client, &config)
            .await
            .map_err(|e| StateError::Connection(e.to_string()))?;

        Ok(Self {
            client,
            state_table: config.state_table(),
            timeout_index_table: config.timeout_index_table(),
        })
    }

    /// Create a `ClickHouseStateStore` from an existing client and config.
    ///
    /// Useful for sharing a client across the store and lock backends.
    /// Runs migrations on creation.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::Backend`] if migrations fail.
    pub async fn from_client(
        client: clickhouse::Client,
        config: &ClickHouseConfig,
    ) -> Result<Self, StateError> {
        migrations::run_migrations(&client, config)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(Self {
            client,
            state_table: config.state_table(),
            timeout_index_table: config.timeout_index_table(),
        })
    }

    /// Compute the `expires_at` millisecond timestamp from an optional TTL.
    #[allow(clippy::cast_possible_wrap)]
    fn expires_at_millis(ttl: Option<Duration>) -> Option<i64> {
        ttl.map(|d| {
            let now = chrono::Utc::now();
            (now + d).timestamp_millis()
        })
    }

    /// Read the current version for a given key (regardless of deleted/expired state).
    async fn current_version(&self, canonical: &str) -> Result<Option<u64>, StateError> {
        let table = &self.state_table;
        let query = format!("SELECT version FROM {table} FINAL WHERE key = ?");

        let rows = self
            .client
            .query(&query)
            .bind(canonical)
            .fetch_all::<VersionRow>()
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(rows.first().map(|r| r.version))
    }

    /// Read the current live (non-deleted, non-expired) value and version for a key.
    async fn current_live_row(
        &self,
        canonical: &str,
    ) -> Result<Option<ValueVersionRow>, StateError> {
        let table = &self.state_table;
        let query = format!(
            "SELECT value, version FROM {table} FINAL \
             WHERE key = ? AND is_deleted = 0 \
             AND (expires_at IS NULL OR expires_at > now64(3))"
        );

        let rows = self
            .client
            .query(&query)
            .bind(canonical)
            .fetch_all::<ValueVersionRow>()
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(rows.into_iter().next())
    }

    /// Insert a new row into the state table.
    async fn insert_row(&self, row: &StateRow) -> Result<(), StateError> {
        let mut insert = self
            .client
            .insert(&self.state_table)
            .map_err(|e| StateError::Backend(e.to_string()))?;

        insert
            .write(row)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        insert
            .end()
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(())
    }
}

#[async_trait]
impl StateStore for ClickHouseStateStore {
    async fn get(&self, key: &StateKey) -> Result<Option<String>, StateError> {
        let canonical = key.canonical();
        let table = &self.state_table;

        let query = format!(
            "SELECT value FROM {table} FINAL \
             WHERE key = ? AND is_deleted = 0 \
             AND (expires_at IS NULL OR expires_at > now64(3))"
        );

        let rows = self
            .client
            .query(&query)
            .bind(&canonical)
            .fetch_all::<ValueRow>()
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(rows.into_iter().next().map(|r| r.value))
    }

    async fn set(
        &self,
        key: &StateKey,
        value: &str,
        ttl: Option<Duration>,
    ) -> Result<(), StateError> {
        let canonical = key.canonical();
        let current_version = self.current_version(&canonical).await?;
        let new_version = current_version.map_or(1, |v| v + 1);

        let row = StateRow {
            key: canonical,
            value: value.to_owned(),
            version: new_version,
            is_deleted: 0,
            expires_at: Self::expires_at_millis(ttl),
        };

        self.insert_row(&row).await
    }

    async fn check_and_set(
        &self,
        key: &StateKey,
        value: &str,
        ttl: Option<Duration>,
    ) -> Result<bool, StateError> {
        let canonical = key.canonical();

        // Check if the key is currently live (non-deleted, non-expired).
        let existing = self.current_live_row(&canonical).await?;

        if existing.is_some() {
            return Ok(false);
        }

        // Key does not exist or is expired/deleted. Get the latest version for
        // the ReplacingMergeTree ordering.
        let current_version = self.current_version(&canonical).await?;
        let new_version = current_version.map_or(1, |v| v + 1);

        let row = StateRow {
            key: canonical,
            value: value.to_owned(),
            version: new_version,
            is_deleted: 0,
            expires_at: Self::expires_at_millis(ttl),
        };

        self.insert_row(&row).await?;
        Ok(true)
    }

    async fn delete(&self, key: &StateKey) -> Result<bool, StateError> {
        let canonical = key.canonical();

        // Check if the key is currently live.
        let existing = self.current_live_row(&canonical).await?;

        let Some(row) = existing else {
            return Ok(false);
        };

        // Insert a tombstone row with version + 1 and is_deleted = 1.
        let tombstone = StateRow {
            key: canonical,
            value: String::new(),
            version: row.version + 1,
            is_deleted: 1,
            expires_at: None,
        };

        self.insert_row(&tombstone).await?;
        Ok(true)
    }

    async fn increment(
        &self,
        key: &StateKey,
        delta: i64,
        ttl: Option<Duration>,
    ) -> Result<i64, StateError> {
        let canonical = key.canonical();

        // Read current live value and version.
        let existing = self.current_live_row(&canonical).await?;

        let (current_value, current_version) = if let Some(row) = existing {
            let parsed = row
                .value
                .parse::<i64>()
                .map_err(|e| StateError::Serialization(e.to_string()))?;
            (parsed, row.version)
        } else {
            // No live row; start from 0. Get the latest version for ordering.
            let v = self.current_version(&canonical).await?.unwrap_or(0);
            (0, v)
        };

        let new_value = current_value + delta;
        let new_version = current_version + 1;

        let row = StateRow {
            key: canonical,
            value: new_value.to_string(),
            version: new_version,
            is_deleted: 0,
            expires_at: Self::expires_at_millis(ttl),
        };

        self.insert_row(&row).await?;
        Ok(new_value)
    }

    async fn compare_and_swap(
        &self,
        key: &StateKey,
        expected_version: u64,
        new_value: &str,
        ttl: Option<Duration>,
    ) -> Result<CasResult, StateError> {
        let canonical = key.canonical();

        // Read the current live row.
        let existing = self.current_live_row(&canonical).await?;

        let Some(row) = existing else {
            return Ok(CasResult::Conflict {
                current_value: None,
                current_version: 0,
            });
        };

        if row.version != expected_version {
            return Ok(CasResult::Conflict {
                current_value: Some(row.value),
                current_version: row.version,
            });
        }

        // Version matches; insert the new value with version + 1.
        let new_row = StateRow {
            key: canonical,
            value: new_value.to_owned(),
            version: row.version + 1,
            is_deleted: 0,
            expires_at: Self::expires_at_millis(ttl),
        };

        self.insert_row(&new_row).await?;
        Ok(CasResult::Ok)
    }

    async fn scan_keys(
        &self,
        namespace: &str,
        tenant: &str,
        kind: KeyKind,
        prefix: Option<&str>,
    ) -> Result<Vec<(String, String)>, StateError> {
        let key_prefix = match prefix {
            Some(p) => format!("{namespace}:{tenant}:{kind}:{p}%"),
            None => format!("{namespace}:{tenant}:{kind}:%"),
        };

        let query = format!(
            "SELECT key, value FROM {} FINAL \
             WHERE key LIKE ? AND is_deleted = 0 \
             AND (expires_at IS NULL OR expires_at > now64(3)) \
             ORDER BY key",
            self.state_table
        );

        let rows = self
            .client
            .query(&query)
            .bind(&key_prefix)
            .fetch_all::<KeyValueRow>()
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(rows.into_iter().map(|r| (r.key, r.value)).collect())
    }

    async fn scan_keys_by_kind(&self, kind: KeyKind) -> Result<Vec<(String, String)>, StateError> {
        // Match keys where the third colon-separated segment is the kind.
        // Pattern: %:*:{kind}:%
        let pattern = format!("%:%:{kind}:%");

        let query = format!(
            "SELECT key, value FROM {} FINAL \
             WHERE key LIKE ? AND is_deleted = 0 \
             AND (expires_at IS NULL OR expires_at > now64(3)) \
             ORDER BY key",
            self.state_table
        );

        let rows = self
            .client
            .query(&query)
            .bind(&pattern)
            .fetch_all::<KeyValueRow>()
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(rows.into_iter().map(|r| (r.key, r.value)).collect())
    }

    async fn index_timeout(&self, key: &StateKey, expires_at_ms: i64) -> Result<(), StateError> {
        let canonical = key.canonical();

        // Insert with version 1 (ReplacingMergeTree will keep the latest version)
        let row = TimeoutIndexRow {
            key: canonical,
            expires_at_ms,
            version: 1,
            is_deleted: 0,
        };

        let mut insert = self
            .client
            .insert(&self.timeout_index_table)
            .map_err(|e| StateError::Backend(e.to_string()))?;

        insert
            .write(&row)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        insert
            .end()
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(())
    }

    async fn remove_timeout_index(&self, key: &StateKey) -> Result<(), StateError> {
        let canonical = key.canonical();

        // Insert a tombstone row with is_deleted = 1
        // First, get the current row to increment version
        let query = format!(
            "SELECT max(version) as version FROM {} FINAL WHERE key = ?",
            self.timeout_index_table
        );

        let rows = self
            .client
            .query(&query)
            .bind(&canonical)
            .fetch_all::<TimeoutVersionRow>()
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        let current_version = rows.first().map_or(0, |r| r.version);

        let tombstone = TimeoutIndexRow {
            key: canonical,
            expires_at_ms: 0,
            version: current_version + 1,
            is_deleted: 1,
        };

        let mut insert = self
            .client
            .insert(&self.timeout_index_table)
            .map_err(|e| StateError::Backend(e.to_string()))?;

        insert
            .write(&tombstone)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        insert
            .end()
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(())
    }

    async fn get_expired_timeouts(&self, now_ms: i64) -> Result<Vec<String>, StateError> {
        // Query using the ORDER BY (expires_at_ms, key) index - O(log N + M)
        let query = format!(
            "SELECT key FROM {} FINAL \
             WHERE expires_at_ms <= ? AND is_deleted = 0 \
             ORDER BY expires_at_ms",
            self.timeout_index_table
        );

        let rows = self
            .client
            .query(&query)
            .bind(now_ms)
            .fetch_all::<TimeoutKeyRow>()
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(rows.into_iter().map(|r| r.key).collect())
    }
}

#[cfg(all(test, feature = "integration"))]
mod integration_tests {
    use super::*;

    fn test_config() -> ClickHouseConfig {
        ClickHouseConfig {
            url: std::env::var("CLICKHOUSE_URL")
                .unwrap_or_else(|_| "http://localhost:8123".to_string()),
            table_prefix: format!("test_{}_", uuid::Uuid::new_v4().simple()),
            ..ClickHouseConfig::default()
        }
    }

    #[tokio::test]
    async fn store_conformance() {
        let config = test_config();
        let store = ClickHouseStateStore::new(config)
            .await
            .expect("client creation should succeed");
        acteon_state::testing::run_store_conformance_tests(&store)
            .await
            .expect("conformance tests should pass");
    }
}
