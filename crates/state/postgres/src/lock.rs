use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sqlx::PgPool;
use tokio::time::Instant;

use acteon_state::error::StateError;
use acteon_state::lock::{DistributedLock, LockGuard};

use crate::config::PostgresConfig;
use crate::migrations;

/// Retry interval when polling for lock acquisition.
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// PostgreSQL-backed implementation of [`DistributedLock`].
///
/// Uses row-based locking in the `{prefix}locks` table. Expired locks are
/// cleaned up before each acquire attempt to ensure stale entries do not block
/// new acquisitions.
pub struct PostgresDistributedLock {
    pool: PgPool,
    config: Arc<PostgresConfig>,
}

impl PostgresDistributedLock {
    /// Create a new `PostgresDistributedLock` from the provided configuration.
    ///
    /// Connects to `PostgreSQL`, creates the connection pool, and runs
    /// migrations to ensure the required tables exist.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::Connection`] if pool creation fails, or
    /// [`StateError::Backend`] if migrations fail.
    pub async fn new(config: PostgresConfig) -> Result<Self, StateError> {
        let connect_options = crate::store::build_connect_options(&config)?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(config.pool_size)
            .connect_with(connect_options)
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

    /// Create a `PostgresDistributedLock` from an existing pool and config.
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

    /// Remove expired lock entries from the locks table.
    async fn clean_expired_locks(&self) -> Result<(), StateError> {
        let table = self.config.locks_table();
        let query = format!("DELETE FROM {table} WHERE expires_at <= NOW()");

        sqlx::query(&query)
            .execute(&self.pool)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(())
    }
}

#[async_trait]
impl DistributedLock for PostgresDistributedLock {
    async fn try_acquire(
        &self,
        name: &str,
        ttl: Duration,
    ) -> Result<Option<Box<dyn LockGuard>>, StateError> {
        // Clean up any expired locks first.
        self.clean_expired_locks().await?;

        let table = self.config.locks_table();
        let owner = uuid::Uuid::new_v4().to_string();
        let expires_at = chrono::Utc::now() + ttl;

        let query = format!(
            "INSERT INTO {table} (name, owner, expires_at) \
             VALUES ($1, $2, $3) \
             ON CONFLICT (name) DO NOTHING"
        );

        let result = sqlx::query(&query)
            .bind(name)
            .bind(&owner)
            .bind(expires_at)
            .execute(&self.pool)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        if result.rows_affected() > 0 {
            Ok(Some(Box::new(PostgresLockGuard {
                pool: self.pool.clone(),
                config: Arc::clone(&self.config),
                name: name.to_owned(),
                owner,
            })))
        } else {
            Ok(None)
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

/// A held distributed lock backed by `PostgreSQL`.
///
/// Dropping the guard without calling [`release`](LockGuard::release) is safe;
/// the lock will expire after its TTL. Explicit release is preferred for prompt
/// cleanup.
pub struct PostgresLockGuard {
    pool: PgPool,
    config: Arc<PostgresConfig>,
    name: String,
    owner: String,
}

#[async_trait]
impl LockGuard for PostgresLockGuard {
    async fn extend(&self, duration: Duration) -> Result<(), StateError> {
        let table = self.config.locks_table();
        let new_expires_at = chrono::Utc::now() + duration;

        let query = format!(
            "UPDATE {table} \
             SET expires_at = $1 \
             WHERE name = $2 AND owner = $3 AND expires_at > NOW()"
        );

        let result = sqlx::query(&query)
            .bind(new_expires_at)
            .bind(&self.name)
            .bind(&self.owner)
            .execute(&self.pool)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        if result.rows_affected() > 0 {
            Ok(())
        } else {
            Err(StateError::LockExpired(self.name.clone()))
        }
    }

    async fn release(self: Box<Self>) -> Result<(), StateError> {
        let table = self.config.locks_table();

        let query = format!("DELETE FROM {table} WHERE name = $1 AND owner = $2");

        sqlx::query(&query)
            .bind(&self.name)
            .bind(&self.owner)
            .execute(&self.pool)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(())
    }

    async fn is_held(&self) -> Result<bool, StateError> {
        let table = self.config.locks_table();

        let query = format!(
            "SELECT 1 FROM {table} \
             WHERE name = $1 AND owner = $2 AND expires_at > NOW()"
        );

        let row: Option<(i32,)> = sqlx::query_as(&query)
            .bind(&self.name)
            .bind(&self.owner)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(row.is_some())
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
    async fn lock_conformance() {
        let config = test_config();
        let lock = PostgresDistributedLock::new(config)
            .await
            .expect("pool creation should succeed");
        acteon_state::testing::run_lock_conformance_tests(&lock)
            .await
            .expect("lock conformance tests should pass");
    }
}
