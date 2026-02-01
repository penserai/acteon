use std::time::Duration;

use async_trait::async_trait;

use crate::error::StateError;

/// A held distributed lock. Dropping without explicit release is allowed
/// (the lock will expire after its TTL), but explicit release is preferred.
#[async_trait]
pub trait LockGuard: Send + Sync {
    /// Extend the lock's TTL.
    async fn extend(&self, duration: Duration) -> Result<(), StateError>;

    /// Explicitly release the lock.
    async fn release(self: Box<Self>) -> Result<(), StateError>;

    /// Check if the lock is still held by this guard.
    async fn is_held(&self) -> Result<bool, StateError>;
}

/// Trait for acquiring distributed locks.
#[async_trait]
pub trait DistributedLock: Send + Sync {
    /// Try to acquire a lock with the given name and TTL.
    /// Returns `None` if the lock is already held by another owner.
    async fn try_acquire(
        &self,
        name: &str,
        ttl: Duration,
    ) -> Result<Option<Box<dyn LockGuard>>, StateError>;

    /// Acquire a lock, waiting up to `timeout` for it to become available.
    async fn acquire(
        &self,
        name: &str,
        ttl: Duration,
        timeout: Duration,
    ) -> Result<Box<dyn LockGuard>, StateError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verify object safety of both traits.
    fn _assert_dyn_lock_guard(_: &dyn LockGuard) {}
    fn _assert_dyn_distributed_lock(_: &dyn DistributedLock) {}
}
