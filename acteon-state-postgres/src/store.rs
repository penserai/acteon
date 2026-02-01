use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sqlx::PgPool;

use acteon_state::error::StateError;
use acteon_state::key::StateKey;
use acteon_state::store::{CasResult, StateStore};

use crate::config::PostgresConfig;
use crate::migrations;

/// PostgreSQL-backed implementation of [`StateStore`].
///
/// Uses `sqlx::PgPool` for connection pooling. TTL is handled via an
/// `expires_at TIMESTAMPTZ` column: reads filter out expired rows with
/// `WHERE (expires_at IS NULL OR expires_at > NOW())`.
pub struct PostgresStateStore {
    pool: PgPool,
    config: Arc<PostgresConfig>,
}

impl PostgresStateStore {
    /// Create a new `PostgresStateStore` from the provided configuration.
    ///
    /// Connects to `PostgreSQL`, creates the connection pool, and runs
    /// migrations to ensure the required tables exist.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::Connection`] if pool creation fails, or
    /// [`StateError::Backend`] if migrations fail.
    pub async fn new(config: PostgresConfig) -> Result<Self, StateError> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(config.pool_size)
            .connect(&config.url)
            .await
            .map_err(|e| StateError::Connection(e.to_string()))?;

        migrations::run_migrations(&pool, &config)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(Self {
            pool,
            config: Arc::new(config),
        })
    }

    /// Create a `PostgresStateStore` from an existing pool and config.
    ///
    /// This is useful for sharing a pool across the store and lock backends.
    /// Runs migrations on creation.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::Backend`] if migrations fail.
    pub async fn from_pool(pool: PgPool, config: PostgresConfig) -> Result<Self, StateError> {
        migrations::run_migrations(&pool, &config)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(Self {
            pool,
            config: Arc::new(config),
        })
    }

    /// Compute the `expires_at` timestamp from an optional TTL.
    fn expires_at_from_ttl(ttl: Option<Duration>) -> Option<chrono::DateTime<chrono::Utc>> {
        ttl.map(|d| chrono::Utc::now() + d)
    }
}

#[async_trait]
impl StateStore for PostgresStateStore {
    async fn check_and_set(
        &self,
        key: &StateKey,
        value: &str,
        ttl: Option<Duration>,
    ) -> Result<bool, StateError> {
        let canonical = key.canonical();
        let expires_at = Self::expires_at_from_ttl(ttl);
        let table = self.config.state_table();

        // First, delete any expired row for this key so the INSERT can succeed.
        let delete_expired = format!(
            "DELETE FROM {table} WHERE key = $1 AND expires_at IS NOT NULL AND expires_at <= NOW()"
        );
        sqlx::query(&delete_expired)
            .bind(&canonical)
            .execute(&self.pool)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        // INSERT ... ON CONFLICT DO NOTHING: only inserts if the key is absent.
        let query = format!(
            "INSERT INTO {table} (key, value, version, expires_at) \
             VALUES ($1, $2, 1, $3) \
             ON CONFLICT (key) DO NOTHING"
        );

        let result = sqlx::query(&query)
            .bind(&canonical)
            .bind(value)
            .bind(expires_at)
            .execute(&self.pool)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(result.rows_affected() > 0)
    }

    async fn get(&self, key: &StateKey) -> Result<Option<String>, StateError> {
        let canonical = key.canonical();
        let table = self.config.state_table();

        let query = format!(
            "SELECT value FROM {table} \
             WHERE key = $1 AND (expires_at IS NULL OR expires_at > NOW())"
        );

        let row: Option<(String,)> = sqlx::query_as(&query)
            .bind(&canonical)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(row.map(|(v,)| v))
    }

    async fn set(
        &self,
        key: &StateKey,
        value: &str,
        ttl: Option<Duration>,
    ) -> Result<(), StateError> {
        let canonical = key.canonical();
        let expires_at = Self::expires_at_from_ttl(ttl);
        let table = self.config.state_table();

        let query = format!(
            "INSERT INTO {table} (key, value, version, expires_at) \
             VALUES ($1, $2, 1, $3) \
             ON CONFLICT (key) DO UPDATE \
             SET value = EXCLUDED.value, \
                 version = {table}.version + 1, \
                 expires_at = EXCLUDED.expires_at"
        );

        sqlx::query(&query)
            .bind(&canonical)
            .bind(value)
            .bind(expires_at)
            .execute(&self.pool)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(())
    }

    async fn delete(&self, key: &StateKey) -> Result<bool, StateError> {
        let canonical = key.canonical();
        let table = self.config.state_table();

        // Only delete non-expired rows so that deleting an expired key returns false.
        let query = format!(
            "DELETE FROM {table} \
             WHERE key = $1 AND (expires_at IS NULL OR expires_at > NOW())"
        );

        let result = sqlx::query(&query)
            .bind(&canonical)
            .execute(&self.pool)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(result.rows_affected() > 0)
    }

    async fn increment(
        &self,
        key: &StateKey,
        delta: i64,
        ttl: Option<Duration>,
    ) -> Result<i64, StateError> {
        let canonical = key.canonical();
        let expires_at = Self::expires_at_from_ttl(ttl);
        let table = self.config.state_table();

        // Delete expired row first so the counter starts fresh.
        let delete_expired = format!(
            "DELETE FROM {table} WHERE key = $1 AND expires_at IS NOT NULL AND expires_at <= NOW()"
        );
        sqlx::query(&delete_expired)
            .bind(&canonical)
            .execute(&self.pool)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        // Upsert: insert with delta as the initial value, or update by adding delta.
        let query = format!(
            "INSERT INTO {table} (key, value, version, expires_at) \
             VALUES ($1, $2::text, 1, $3) \
             ON CONFLICT (key) DO UPDATE \
             SET value = ({table}.value::bigint + $2)::text, \
                 version = {table}.version + 1, \
                 expires_at = COALESCE(EXCLUDED.expires_at, {table}.expires_at) \
             RETURNING value"
        );

        let row: (String,) = sqlx::query_as(&query)
            .bind(&canonical)
            .bind(delta)
            .bind(expires_at)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        row.0
            .parse::<i64>()
            .map_err(|e| StateError::Serialization(e.to_string()))
    }

    async fn compare_and_swap(
        &self,
        key: &StateKey,
        expected_version: u64,
        new_value: &str,
        ttl: Option<Duration>,
    ) -> Result<CasResult, StateError> {
        let canonical = key.canonical();
        let expires_at = Self::expires_at_from_ttl(ttl);
        let table = self.config.state_table();
        let expected_version = i64::try_from(expected_version).unwrap_or(i64::MAX);

        // First, read the current row (only non-expired).
        let select_query = format!(
            "SELECT value, version FROM {table} \
             WHERE key = $1 AND (expires_at IS NULL OR expires_at > NOW())"
        );

        let current: Option<(String, i64)> = sqlx::query_as(&select_query)
            .bind(&canonical)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        let Some((current_value, current_version)) = current else {
            return Ok(CasResult::Conflict {
                current_value: None,
                current_version: 0,
            });
        };

        if current_version != expected_version {
            return Ok(CasResult::Conflict {
                current_value: Some(current_value),
                current_version: u64::try_from(current_version).unwrap_or(0),
            });
        }

        // Conditional update: only succeed if version still matches.
        let update_query = format!(
            "UPDATE {table} \
             SET value = $1, version = version + 1, expires_at = $2 \
             WHERE key = $3 AND version = $4"
        );

        let result = sqlx::query(&update_query)
            .bind(new_value)
            .bind(expires_at)
            .bind(&canonical)
            .bind(expected_version)
            .execute(&self.pool)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        if result.rows_affected() > 0 {
            Ok(CasResult::Ok)
        } else {
            // Concurrent modification occurred between our SELECT and UPDATE.
            // Re-read the current state for the conflict response.
            let current: Option<(String, i64)> = sqlx::query_as(&select_query)
                .bind(&canonical)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| StateError::Backend(e.to_string()))?;

            match current {
                Some((val, ver)) => Ok(CasResult::Conflict {
                    current_value: Some(val),
                    current_version: u64::try_from(ver).unwrap_or(0),
                }),
                None => Ok(CasResult::Conflict {
                    current_value: None,
                    current_version: 0,
                }),
            }
        }
    }
}

#[cfg(all(test, feature = "integration"))]
mod integration_tests {
    use super::*;

    fn test_config() -> PostgresConfig {
        PostgresConfig {
            url: std::env::var("DATABASE_URL")
                .unwrap_or_else(|_| "postgres://localhost:5432/acteon_test".to_string()),
            table_prefix: format!("test_{}_", uuid::Uuid::new_v4().simple()),
            ..PostgresConfig::default()
        }
    }

    #[tokio::test]
    async fn store_conformance() {
        let config = test_config();
        let store = PostgresStateStore::new(config)
            .await
            .expect("pool creation should succeed");
        acteon_state::testing::run_store_conformance_tests(&store)
            .await
            .expect("conformance tests should pass");
    }
}
