//! Redis-backed distributed locking.
//!
//! This module provides a [`RedisDistributedLock`] implementation that uses
//! Redis `SET NX PX` commands (via Lua scripts) to implement distributed locks.
//!
//! # Safety Warning
//!
//! **This lock provides at-most-once delivery guarantees only in non-clustered,
//! non-failover scenarios.** In Redis Cluster or Sentinel deployments, mutual
//! exclusion can be violated during failover events. See the [Guarantees](#guarantees)
//! section for details.
//!
//! For strict mutual exclusion requirements, use the `PostgreSQL` or `DynamoDB`
//! backends instead.
//!
//! # How It Works
//!
//! Locks are acquired using the Redis `SET key value NX PX milliseconds` pattern:
//!
//! - **NX** (Not eXists): The key is only set if it doesn't already exist.
//! - **PX** (expiration): The key automatically expires after the TTL to prevent
//!   deadlocks if the lock holder crashes.
//! - **Owner token**: A UUID owner token is stored as the value, ensuring that
//!   only the lock holder can release or extend the lock.
//!
//! All lock operations (acquire, extend, release) use Lua scripts to ensure
//! atomicity at the Redis level.
//!
//! # Guarantees
//!
//! ## Single Redis Instance
//!
//! When using a single Redis instance (standalone mode), this implementation
//! provides **full mutual exclusion**: at most one client can hold a given lock
//! at any time, assuming the lock TTL is longer than the critical section.
//!
//! ## Redis Cluster / Sentinel
//!
//! **Important:** When using Redis Cluster or Redis Sentinel, this lock
//! implementation does **not** provide strong mutual exclusion guarantees
//! during failover events.
//!
//! The issue is that Redis replication is asynchronous. If the master fails
//! immediately after a lock is acquired but before the write is replicated to
//! a replica, the newly promoted master will not have the lock key. This allows
//! a second client to acquire the "same" lock, violating mutual exclusion.
//!
//! ## When to Use This Lock
//!
//! This implementation is appropriate when:
//!
//! - **Development/testing**: Simplicity and ease of setup are priorities.
//! - **Single Redis instance**: No replication or clustering is involved.
//! - **Idempotent operations**: Your application can tolerate occasional
//!   duplicate execution (e.g., sending the same notification twice during
//!   a rare failover is acceptable).
//! - **Best-effort coordination**: The lock is used for optimization (e.g.,
//!   reducing duplicate work) rather than strict correctness requirements.
//!
//! ## When to Use Alternatives
//!
//! If you require strong consistency guarantees, consider these alternatives:
//!
//! - **`PostgreSQL` advisory locks**: Uses database transactions for ACID
//!   guarantees. Locks survive failover with synchronous replication.
//! - **`DynamoDB` with conditional writes**: Provides strong consistency when
//!   using consistent reads and conditional expressions.
//! - **Redlock algorithm**: A distributed lock algorithm using multiple
//!   independent Redis instances. Note: Redlock has been criticized for
//!   not providing the guarantees it claims; evaluate carefully.
//! - **`ZooKeeper` / etcd**: Purpose-built coordination services with strong
//!   consistency guarantees.
//!
//! # Example
//!
//! ```ignore
//! use std::time::Duration;
//! use acteon_state::DistributedLock;
//! use acteon_state_redis::{RedisConfig, RedisDistributedLock};
//!
//! let config = RedisConfig::new("redis://localhost:6379");
//! let lock = RedisDistributedLock::new(&config)?;
//!
//! // Acquire lock with 30s TTL and 5s timeout
//! let guard = lock.acquire("my-lock", Duration::from_secs(30), Duration::from_secs(5)).await?;
//!
//! // Critical section...
//!
//! guard.release().await?;
//! ```

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
/// Uses `SET NX PX` via Lua scripts to guarantee atomicity. See the
/// [module-level documentation](self) for important information about
/// consistency guarantees and failover behavior.
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
