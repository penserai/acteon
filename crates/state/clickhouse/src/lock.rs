use std::time::Duration;

use async_trait::async_trait;
use tokio::time::Instant;

use acteon_state::error::StateError;
use acteon_state::lock::{DistributedLock, LockGuard};

use crate::config::ClickHouseConfig;
use crate::migrations;

/// Retry interval when polling for lock acquisition.
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Row type used when reading lock state.
#[derive(clickhouse::Row, serde::Deserialize)]
struct LockRow {
    owner: String,
}

/// Row type used when reading only the version from the locks table.
#[derive(clickhouse::Row, serde::Deserialize)]
struct LockVersionRow {
    version: u64,
}

/// Row type used when inserting into the locks table.
#[derive(clickhouse::Row, serde::Serialize)]
struct LockInsertRow {
    name: String,
    owner: String,
    expires_at: i64,
    version: u64,
}

/// Row type used for existence checks.
#[derive(clickhouse::Row, serde::Deserialize)]
struct LockExistsRow {
    #[allow(dead_code)]
    owner: String,
}

/// `ClickHouse`-backed implementation of [`DistributedLock`].
///
/// Uses a `ReplacingMergeTree(version)` table ordered by lock name. Lock
/// ownership is tracked via an `owner` column (a UUID) and an `expires_at`
/// timestamp. Expired locks are considered released and can be re-acquired.
///
/// **Note:** `ClickHouse` does not support atomic conditional inserts. Lock
/// acquisition uses a best-effort read-then-write strategy with a re-check
/// after insertion. Under heavy contention, two processes may briefly believe
/// they hold the same lock. Use this backend only when eventual consistency
/// for locking is acceptable.
pub struct ClickHouseDistributedLock {
    client: clickhouse::Client,
    locks_table: String,
}

impl ClickHouseDistributedLock {
    /// Create a new `ClickHouseDistributedLock` from the provided configuration.
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
            locks_table: config.locks_table(),
        })
    }

    /// Create a `ClickHouseDistributedLock` from an existing client and config.
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
            locks_table: config.locks_table(),
        })
    }

    /// Read the current active lock row for the given name (non-expired, non-empty owner).
    async fn active_lock(&self, name: &str) -> Result<Option<LockRow>, StateError> {
        let table = &self.locks_table;
        let query = format!(
            "SELECT owner FROM {table} FINAL \
             WHERE name = ? AND expires_at > now64(3) AND owner != ''"
        );

        let rows = self
            .client
            .query(&query)
            .bind(name)
            .fetch_all::<LockRow>()
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(rows.into_iter().next())
    }

    /// Read the current maximum version for the given lock name.
    async fn current_version(&self, name: &str) -> Result<Option<u64>, StateError> {
        let table = &self.locks_table;
        let query = format!("SELECT version FROM {table} FINAL WHERE name = ?");

        let rows = self
            .client
            .query(&query)
            .bind(name)
            .fetch_all::<LockVersionRow>()
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(rows.first().map(|r| r.version))
    }

    /// Insert a lock row.
    async fn insert_lock(&self, row: &LockInsertRow) -> Result<(), StateError> {
        let mut insert = self
            .client
            .insert(&self.locks_table)
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
impl DistributedLock for ClickHouseDistributedLock {
    async fn try_acquire(
        &self,
        name: &str,
        ttl: Duration,
    ) -> Result<Option<Box<dyn LockGuard>>, StateError> {
        // Check if there is an active (non-expired) lock held by someone.
        let active = self.active_lock(name).await?;

        if active.is_some() {
            return Ok(None);
        }

        // No active lock. Get the current version for this name.
        let current_version = self.current_version(name).await?;
        let new_version = current_version.map_or(1, |v| v + 1);

        let owner = uuid::Uuid::new_v4().to_string();
        #[allow(clippy::cast_possible_wrap)]
        let expires_at = (chrono::Utc::now() + ttl).timestamp_millis();

        let row = LockInsertRow {
            name: name.to_owned(),
            owner: owner.clone(),
            expires_at,
            version: new_version,
        };

        self.insert_lock(&row).await?;

        // Re-check: verify we are the owner after insertion. Because
        // ReplacingMergeTree keeps the row with the highest version, our
        // row wins only if no other writer inserted a higher version
        // concurrently.
        let recheck = self.active_lock(name).await?;

        match recheck {
            Some(lock_row) if lock_row.owner == owner => Ok(Some(Box::new(ClickHouseLockGuard {
                client: self.client.clone(),
                locks_table: self.locks_table.clone(),
                name: name.to_owned(),
                owner,
            }))),
            _ => {
                // Another writer won the race. Our row will be collapsed
                // during the next merge.
                Ok(None)
            }
        }
    }

    async fn acquire(
        &self,
        name: &str,
        ttl: Duration,
        timeout: Duration,
    ) -> Result<Box<dyn LockGuard>, StateError> {
        let deadline = Instant::now() + timeout;

        loop {
            if let Some(guard) = self.try_acquire(name, ttl).await? {
                return Ok(guard);
            }

            if Instant::now() >= deadline {
                return Err(StateError::Timeout(timeout));
            }

            let remaining = deadline - Instant::now();
            let sleep_dur = LOCK_POLL_INTERVAL.min(remaining);
            tokio::time::sleep(sleep_dur).await;
        }
    }
}

/// A held distributed lock backed by `ClickHouse`.
///
/// Dropping the guard without calling [`release`](LockGuard::release) is safe;
/// the lock will expire after its TTL. Explicit release is preferred for prompt
/// cleanup.
pub struct ClickHouseLockGuard {
    client: clickhouse::Client,
    locks_table: String,
    name: String,
    owner: String,
}

#[async_trait]
impl LockGuard for ClickHouseLockGuard {
    async fn extend(&self, duration: Duration) -> Result<(), StateError> {
        let table = &self.locks_table;

        // Verify the lock is still held by us.
        let query = format!(
            "SELECT version FROM {table} FINAL \
             WHERE name = ? AND owner = ? AND expires_at > now64(3)"
        );

        let rows = self
            .client
            .query(&query)
            .bind(&self.name)
            .bind(&self.owner)
            .fetch_all::<LockVersionRow>()
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        let current = rows
            .first()
            .ok_or_else(|| StateError::LockExpired(self.name.clone()))?;

        // Insert a new row with an extended expiration and version + 1.
        #[allow(clippy::cast_possible_wrap)]
        let new_expires_at = (chrono::Utc::now() + duration).timestamp_millis();

        let row = LockInsertRow {
            name: self.name.clone(),
            owner: self.owner.clone(),
            expires_at: new_expires_at,
            version: current.version + 1,
        };

        let mut insert = self
            .client
            .insert(&self.locks_table)
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

    async fn release(self: Box<Self>) -> Result<(), StateError> {
        let table = &self.locks_table;

        // Get the current version for this lock name.
        let query = format!("SELECT version FROM {table} FINAL WHERE name = ?");

        let rows = self
            .client
            .query(&query)
            .bind(&self.name)
            .fetch_all::<LockVersionRow>()
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        let new_version = rows.first().map_or(1, |r| r.version + 1);

        // Insert a "released" row with an empty owner and an epoch-zero
        // expiration so that the lock is immediately considered expired.
        let row = LockInsertRow {
            name: self.name.clone(),
            owner: String::new(),
            expires_at: 0,
            version: new_version,
        };

        let mut insert = self
            .client
            .insert(&self.locks_table)
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

    async fn is_held(&self) -> Result<bool, StateError> {
        let table = &self.locks_table;

        let query = format!(
            "SELECT owner FROM {table} FINAL \
             WHERE name = ? AND owner = ? AND expires_at > now64(3)"
        );

        let rows = self
            .client
            .query(&query)
            .bind(&self.name)
            .bind(&self.owner)
            .fetch_all::<LockExistsRow>()
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(!rows.is_empty())
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
    async fn lock_conformance() {
        let config = test_config();
        let lock = ClickHouseDistributedLock::new(config)
            .await
            .expect("client creation should succeed");
        acteon_state::testing::run_lock_conformance_tests(&lock)
            .await
            .expect("lock conformance tests should pass");
    }
}
