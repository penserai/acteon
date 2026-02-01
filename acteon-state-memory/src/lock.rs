use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;
use tokio::time::Instant;
use uuid::Uuid;

use acteon_state::error::StateError;
use acteon_state::lock::{DistributedLock, LockGuard};

/// Internal entry representing a held lock.
#[derive(Debug, Clone)]
struct LockEntry {
    owner: String,
    expires_at: Instant,
}

impl LockEntry {
    fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }
}

/// In-memory [`DistributedLock`] backed by a [`DashMap`].
///
/// Lock expiry is lazy: expired entries are evicted on the next acquire
/// attempt for the same lock name.
#[derive(Debug, Clone, Default)]
pub struct MemoryDistributedLock {
    locks: Arc<DashMap<String, LockEntry>>,
}

impl MemoryDistributedLock {
    /// Create a new in-memory distributed lock manager.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl DistributedLock for MemoryDistributedLock {
    async fn try_acquire(
        &self,
        name: &str,
        ttl: Duration,
    ) -> Result<Option<Box<dyn LockGuard>>, StateError> {
        let key = name.to_owned();

        // Remove expired entries lazily.
        self.locks.remove_if(&key, |_, entry| entry.is_expired());

        // Try to insert only if vacant.
        let owner = Uuid::new_v4().to_string();
        match self.locks.entry(key.clone()) {
            dashmap::mapref::entry::Entry::Occupied(_) => Ok(None),
            dashmap::mapref::entry::Entry::Vacant(vacant) => {
                vacant.insert(LockEntry {
                    owner: owner.clone(),
                    expires_at: Instant::now() + ttl,
                });
                Ok(Some(Box::new(MemoryLockGuard {
                    locks: Arc::clone(&self.locks),
                    name: key,
                    owner,
                })))
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

            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// Guard for a lock acquired via [`MemoryDistributedLock`].
///
/// Holds an `Arc` reference to the backing map so the lock can be released
/// or extended without going through the parent [`MemoryDistributedLock`].
#[derive(Debug)]
pub struct MemoryLockGuard {
    locks: Arc<DashMap<String, LockEntry>>,
    name: String,
    owner: String,
}

#[async_trait]
impl LockGuard for MemoryLockGuard {
    async fn extend(&self, duration: Duration) -> Result<(), StateError> {
        let mut entry = self
            .locks
            .get_mut(&self.name)
            .ok_or_else(|| StateError::LockExpired(self.name.clone()))?;

        if entry.owner != self.owner {
            return Err(StateError::LockExpired(self.name.clone()));
        }

        if entry.is_expired() {
            return Err(StateError::LockExpired(self.name.clone()));
        }

        entry.expires_at = Instant::now() + duration;
        Ok(())
    }

    async fn release(self: Box<Self>) -> Result<(), StateError> {
        // Only remove if we are still the owner.
        self.locks
            .remove_if(&self.name, |_, entry| entry.owner == self.owner);
        Ok(())
    }

    async fn is_held(&self) -> Result<bool, StateError> {
        match self.locks.get(&self.name) {
            Some(entry) => Ok(entry.owner == self.owner && !entry.is_expired()),
            None => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use acteon_state::testing::run_lock_conformance_tests;

    use super::*;

    #[tokio::test]
    async fn conformance() {
        let lock = MemoryDistributedLock::new();
        run_lock_conformance_tests(&lock)
            .await
            .expect("lock conformance tests should pass");
    }

    #[tokio::test(start_paused = true)]
    async fn lock_expires_after_ttl() {
        let lock = MemoryDistributedLock::new();

        let guard = lock
            .try_acquire("expire-lock", Duration::from_secs(2))
            .await
            .unwrap()
            .expect("should acquire");

        // Lock should be held.
        assert!(guard.is_held().await.unwrap());

        // Advance past TTL.
        tokio::time::advance(Duration::from_secs(3)).await;

        // Guard should report not held.
        assert!(!guard.is_held().await.unwrap());

        // Another caller should be able to acquire.
        let guard2 = lock
            .try_acquire("expire-lock", Duration::from_secs(10))
            .await
            .unwrap();
        assert!(guard2.is_some(), "should acquire after TTL expiry");
    }

    #[tokio::test(start_paused = true)]
    async fn acquire_waits_until_released() {
        let lock = MemoryDistributedLock::new();

        let guard = lock
            .try_acquire("wait-lock", Duration::from_secs(1))
            .await
            .unwrap()
            .expect("should acquire");

        // Spawn a task that releases the lock after some time.
        let lock_clone = lock.clone();
        let handle = tokio::spawn(async move {
            lock_clone
                .acquire("wait-lock", Duration::from_secs(5), Duration::from_secs(10))
                .await
        });

        // Advance time so the first lock expires.
        tokio::time::advance(Duration::from_secs(2)).await;

        // The original guard should no longer be held.
        assert!(!guard.is_held().await.unwrap());

        let result = handle.await.unwrap();
        assert!(
            result.is_ok(),
            "acquire should succeed after original TTL expires"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn acquire_timeout() {
        let lock = MemoryDistributedLock::new();

        let _guard = lock
            .try_acquire("timeout-lock", Duration::from_secs(60))
            .await
            .unwrap()
            .expect("should acquire");

        // Try to acquire with a short timeout -- should fail.
        let result = lock
            .acquire(
                "timeout-lock",
                Duration::from_secs(5),
                Duration::from_secs(1),
            )
            .await;

        assert!(
            matches!(result, Err(StateError::Timeout(_))),
            "should time out when lock is held"
        );
    }

    #[tokio::test]
    async fn concurrent_lock_contention() {
        let lock = Arc::new(MemoryDistributedLock::new());
        let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));

        let mut handles = Vec::new();

        for _ in 0..10 {
            let lock = Arc::clone(&lock);
            let counter = Arc::clone(&counter);
            handles.push(tokio::spawn(async move {
                let guard = lock
                    .acquire(
                        "contention-lock",
                        Duration::from_millis(200),
                        Duration::from_secs(5),
                    )
                    .await
                    .expect("should eventually acquire");

                // Simulate work inside the critical section.
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

                guard.release().await.expect("release should succeed");
            }));
        }

        for h in handles {
            h.await.expect("task should not panic");
        }

        assert_eq!(
            counter.load(std::sync::atomic::Ordering::SeqCst),
            10,
            "all tasks should have completed"
        );
    }
}
