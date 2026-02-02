use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use etcd_client::{Client, Compare, CompareOp, Txn, TxnOp};
use tokio::sync::Mutex;

use acteon_state::error::StateError;
use acteon_state::lock::{DistributedLock, LockGuard};

use crate::config::EtcdConfig;

/// Retry interval when polling for lock acquisition.
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// etcd-backed implementation of [`DistributedLock`].
///
/// Uses etcd leases for TTL-based locking. Each lock acquisition creates a
/// lease with the requested TTL and then conditionally writes the lock key
/// only if it does not already exist.
pub struct EtcdDistributedLock {
    client: Arc<Mutex<Client>>,
    config: Arc<EtcdConfig>,
}

impl EtcdDistributedLock {
    /// Create a new `EtcdDistributedLock` by connecting to etcd.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::Connection`] if the connection cannot be
    /// established.
    pub async fn new(config: EtcdConfig) -> Result<Self, StateError> {
        let client = Client::connect(
            config.endpoints.clone(),
            Some(etcd_client::ConnectOptions::new().with_timeout(config.connect_timeout)),
        )
        .await
        .map_err(|e| StateError::Connection(e.to_string()))?;

        Ok(Self {
            client: Arc::new(Mutex::new(client)),
            config: Arc::new(config),
        })
    }

    /// Create an `EtcdDistributedLock` from an existing client and config.
    ///
    /// Useful for sharing a client between the store and lock backends.
    pub fn from_client(client: Arc<Mutex<Client>>, config: Arc<EtcdConfig>) -> Self {
        Self { client, config }
    }
}

#[async_trait]
impl DistributedLock for EtcdDistributedLock {
    async fn try_acquire(
        &self,
        name: &str,
        ttl: Duration,
    ) -> Result<Option<Box<dyn LockGuard>>, StateError> {
        let lock_key = self.config.lock_key(name);
        let owner = uuid::Uuid::new_v4().to_string();
        let ttl_secs = i64::try_from(ttl.as_secs().max(1)).unwrap_or(i64::MAX);

        let mut client = self.client.lock().await;

        // Create a lease with the requested TTL.
        let lease_resp = client
            .lease_grant(ttl_secs, None)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;
        let lease_id = lease_resp.id();

        // Attach the put to the lease so etcd auto-expires it.
        let put_options = etcd_client::PutOptions::new().with_lease(lease_id);

        // Transaction: only put if the key does not exist (create_revision == 0).
        let txn = Txn::new()
            .when([Compare::create_revision(
                lock_key.clone(),
                CompareOp::Equal,
                0,
            )])
            .and_then([TxnOp::put(
                lock_key.clone(),
                owner.as_bytes(),
                Some(put_options),
            )])
            .or_else([]);

        let resp = client
            .txn(txn)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        if resp.succeeded() {
            Ok(Some(Box::new(EtcdLockGuard {
                client: Arc::clone(&self.client),
                lock_key,
                owner,
                lease_id,
            })))
        } else {
            // Lock is held by someone else; revoke the unused lease.
            let _ = client.lease_revoke(lease_id).await;
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

/// A held distributed lock backed by etcd.
///
/// The lock is associated with an etcd lease. Dropping the guard without
/// calling [`release`](LockGuard::release) is safe; the lease will expire
/// after its TTL and etcd will automatically delete the key.
/// Explicit release is preferred for prompt cleanup.
pub struct EtcdLockGuard {
    client: Arc<Mutex<Client>>,
    lock_key: String,
    owner: String,
    lease_id: i64,
}

#[async_trait]
impl LockGuard for EtcdLockGuard {
    async fn extend(&self, duration: Duration) -> Result<(), StateError> {
        // First verify we still own the lock.
        if !self.is_held().await? {
            return Err(StateError::LockExpired(format!(
                "lock {} is no longer held by this owner",
                self.lock_key
            )));
        }

        // Keep the lease alive. etcd's LeaseKeepAlive refreshes the TTL.
        // For a simple extend we use lease_keep_alive once.
        let mut client = self.client.lock().await;
        let (mut keeper, _stream) = client
            .lease_keep_alive(self.lease_id)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        keeper
            .keep_alive()
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        // If the caller wants a different TTL than the original lease,
        // we revoke the old lease and create a new one, re-attaching
        // the key. However, etcd's keep_alive only resets to the
        // original grant TTL. For simplicity (and matching the Redis/PG
        // pattern), we grant a new lease and re-put the key atomically.
        drop(keeper);

        let ttl_secs = i64::try_from(duration.as_secs().max(1)).unwrap_or(i64::MAX);
        let lease_resp = client
            .lease_grant(ttl_secs, None)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;
        let new_lease_id = lease_resp.id();

        let put_options = etcd_client::PutOptions::new().with_lease(new_lease_id);

        // Only re-put if we still own the key (value == owner).
        let resp = client
            .get(self.lock_key.clone(), None)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        let Some(kv) = resp.kvs().first() else {
            let _ = client.lease_revoke(new_lease_id).await;
            return Err(StateError::LockExpired(format!(
                "lock {} has been released",
                self.lock_key
            )));
        };

        let current_owner = String::from_utf8(kv.value().to_vec()).unwrap_or_default();

        if current_owner != self.owner {
            let _ = client.lease_revoke(new_lease_id).await;
            return Err(StateError::LockExpired(format!(
                "lock {} is no longer held by this owner",
                self.lock_key
            )));
        }

        let mod_revision = kv.mod_revision();
        let txn = Txn::new()
            .when([Compare::mod_revision(
                self.lock_key.clone(),
                CompareOp::Equal,
                mod_revision,
            )])
            .and_then([TxnOp::put(
                self.lock_key.clone(),
                self.owner.as_bytes(),
                Some(put_options),
            )])
            .or_else([]);

        let txn_resp = client
            .txn(txn)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        if txn_resp.succeeded() {
            // Revoke the old lease (the key is now on the new lease).
            let _ = client.lease_revoke(self.lease_id).await;
            Ok(())
        } else {
            let _ = client.lease_revoke(new_lease_id).await;
            Err(StateError::LockExpired(format!(
                "lock {} was concurrently modified during extend",
                self.lock_key
            )))
        }
    }

    async fn release(self: Box<Self>) -> Result<(), StateError> {
        let mut client = self.client.lock().await;

        // Delete the lock key.
        let _ = client
            .delete(self.lock_key.clone(), None)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        // Revoke the lease to clean up server-side resources.
        let _ = client
            .lease_revoke(self.lease_id)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(())
    }

    async fn is_held(&self) -> Result<bool, StateError> {
        let mut client = self.client.lock().await;
        let resp = client
            .get(self.lock_key.clone(), None)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        let Some(kv) = resp.kvs().first() else {
            return Ok(false);
        };

        // Verify ownership by comparing the stored value to our owner ID.
        let current_owner = String::from_utf8(kv.value().to_vec()).unwrap_or_default();

        Ok(current_owner == self.owner)
    }
}

#[cfg(all(test, feature = "integration"))]
mod integration_tests {
    use super::*;
    use crate::config::EtcdConfig;

    fn test_config() -> EtcdConfig {
        let endpoints = std::env::var("ETCD_ENDPOINTS")
            .map(|s| s.split(',').map(String::from).collect())
            .unwrap_or_else(|_| vec!["http://localhost:2379".to_string()]);
        EtcdConfig {
            endpoints,
            prefix: format!("acteon-test-{}", uuid::Uuid::new_v4()),
            ..EtcdConfig::default()
        }
    }

    #[tokio::test]
    async fn lock_conformance() {
        let config = test_config();
        let lock = EtcdDistributedLock::new(config)
            .await
            .expect("etcd connection should succeed");
        acteon_state::testing::run_lock_conformance_tests(&lock)
            .await
            .expect("lock conformance tests should pass");
    }
}
