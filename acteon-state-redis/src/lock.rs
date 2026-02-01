use std::time::Duration;

use async_trait::async_trait;
use deadpool_redis::{Config, Pool, Runtime};
use redis::{AsyncCommands, Script};

use acteon_state::error::StateError;
use acteon_state::lock::{DistributedLock, LockGuard};

use crate::config::RedisConfig;
use crate::scripts;

/// Redis-backed implementation of [`DistributedLock`].
///
/// Uses `SET NX PX` via Lua scripts to guarantee atomicity.
pub struct RedisDistributedLock {
    pool: Pool,
    prefix: String,
}

impl RedisDistributedLock {
    /// Create a new `RedisDistributedLock` from the provided configuration.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::Connection`] if the pool cannot be created.
    pub fn new(config: &RedisConfig) -> Result<Self, StateError> {
        let cfg = Config::from_url(&config.url);
        let pool = cfg
            .builder()
            .map(|b| {
                b.max_size(config.pool_size)
                    .wait_timeout(Some(config.connection_timeout))
                    .runtime(Runtime::Tokio1)
                    .build()
            })
            .map_err(|e| StateError::Connection(e.to_string()))?
            .map_err(|e| StateError::Connection(e.to_string()))?;

        Ok(Self {
            pool,
            prefix: config.prefix.clone(),
        })
    }

    /// Build the full Redis key for a lock.
    fn lock_key(&self, name: &str) -> String {
        format!("{}:lock:{}", self.prefix, name)
    }

    /// Obtain a connection from the pool.
    async fn conn(&self) -> Result<deadpool_redis::Connection, StateError> {
        self.pool
            .get()
            .await
            .map_err(|e| StateError::Connection(e.to_string()))
    }
}

/// Retry interval when polling for lock acquisition.
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[async_trait]
impl DistributedLock for RedisDistributedLock {
    async fn try_acquire(
        &self,
        name: &str,
        ttl: Duration,
    ) -> Result<Option<Box<dyn LockGuard>>, StateError> {
        let redis_key = self.lock_key(name);
        let owner = uuid::Uuid::new_v4().to_string();
        let ttl_ms = i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX);

        let mut conn = self.conn().await?;
        let script = Script::new(scripts::LOCK_ACQUIRE);
        let result: i64 = script
            .key(&redis_key)
            .arg(&owner)
            .arg(ttl_ms)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        if result == 1 {
            Ok(Some(Box::new(RedisLockGuard {
                pool: self.pool.clone(),
                redis_key,
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
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            if let Some(guard) = self.try_acquire(name, ttl).await? {
                return Ok(guard);
            }

            if tokio::time::Instant::now() >= deadline {
                return Err(StateError::Timeout(timeout));
            }

            let remaining = deadline - tokio::time::Instant::now();
            let sleep_dur = LOCK_POLL_INTERVAL.min(remaining);
            tokio::time::sleep(sleep_dur).await;
        }
    }
}

/// A held distributed lock backed by Redis.
///
/// Dropping the guard without calling [`release`](LockGuard::release) is safe;
/// the lock will expire after its TTL. Explicit release is preferred for prompt
/// cleanup.
pub struct RedisLockGuard {
    pool: Pool,
    redis_key: String,
    owner: String,
}

impl RedisLockGuard {
    /// Obtain a connection from the pool.
    async fn conn(&self) -> Result<deadpool_redis::Connection, StateError> {
        self.pool
            .get()
            .await
            .map_err(|e| StateError::Connection(e.to_string()))
    }
}

#[async_trait]
impl LockGuard for RedisLockGuard {
    async fn extend(&self, duration: Duration) -> Result<(), StateError> {
        let ttl_ms = i64::try_from(duration.as_millis()).unwrap_or(i64::MAX);
        let mut conn = self.conn().await?;

        let script = Script::new(scripts::LOCK_EXTEND);
        let result: i64 = script
            .key(&self.redis_key)
            .arg(&self.owner)
            .arg(ttl_ms)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        if result == 1 {
            Ok(())
        } else {
            Err(StateError::LockExpired(format!(
                "lock {} is no longer held by this owner",
                self.redis_key
            )))
        }
    }

    async fn release(self: Box<Self>) -> Result<(), StateError> {
        let mut conn = self.conn().await?;

        let script = Script::new(scripts::LOCK_RELEASE);
        let result: i64 = script
            .key(&self.redis_key)
            .arg(&self.owner)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        if result == 1 {
            Ok(())
        } else {
            Err(StateError::LockExpired(format!(
                "lock {} was not held by this owner at release time",
                self.redis_key
            )))
        }
    }

    async fn is_held(&self) -> Result<bool, StateError> {
        let mut conn = self.conn().await?;
        let current_owner: Option<String> = conn
            .get(&self.redis_key)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(current_owner.as_deref() == Some(&self.owner))
    }
}

#[cfg(all(test, feature = "integration"))]
mod integration_tests {
    use super::*;
    use crate::config::RedisConfig;

    fn test_config() -> RedisConfig {
        RedisConfig {
            url: std::env::var("REDIS_URL")
                .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string()),
            prefix: format!("acteon-test-{}", uuid::Uuid::new_v4()),
            ..RedisConfig::default()
        }
    }

    #[tokio::test]
    async fn lock_conformance() {
        let config = test_config();
        let lock = RedisDistributedLock::new(&config).expect("pool creation should succeed");
        acteon_state::testing::run_lock_conformance_tests(&lock)
            .await
            .expect("conformance tests should pass");
    }
}
