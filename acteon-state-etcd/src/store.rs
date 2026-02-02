use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use etcd_client::{Client, Compare, CompareOp, Txn, TxnOp};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use acteon_state::error::StateError;
use acteon_state::key::StateKey;
use acteon_state::store::{CasResult, StateStore};

use crate::config::EtcdConfig;

/// Maximum number of retries for transactional operations that encounter
/// version conflicts (e.g. `set`, `increment`).
const MAX_RETRIES: usize = 10;

/// JSON envelope stored as the value for each etcd key.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredValue {
    value: String,
    version: u64,
    /// Epoch seconds at which this entry expires, or `None` for no expiry.
    expires_at: Option<i64>,
}

impl StoredValue {
    /// Check whether this entry has expired relative to the current time.
    fn is_expired(&self) -> bool {
        self.expires_at
            .is_some_and(|exp| chrono::Utc::now().timestamp() >= exp)
    }
}

/// etcd-backed implementation of [`StateStore`].
///
/// Stores values as JSON-encoded [`StoredValue`] envelopes. Versioning and
/// expiry are managed at the application level within this envelope.
/// Atomicity is achieved via etcd transactions that compare on `mod_revision`.
pub struct EtcdStateStore {
    client: Arc<Mutex<Client>>,
    config: Arc<EtcdConfig>,
}

impl EtcdStateStore {
    /// Create a new `EtcdStateStore` by connecting to etcd.
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

    /// Create an `EtcdStateStore` from an existing client and config.
    ///
    /// Useful for sharing a client between the store and lock backends.
    pub fn from_client(client: Arc<Mutex<Client>>, config: Arc<EtcdConfig>) -> Self {
        Self { client, config }
    }

    /// Compute the `expires_at` epoch timestamp from an optional TTL.
    fn expires_at_from_ttl(ttl: Option<Duration>) -> Option<i64> {
        ttl.map(|d| chrono::Utc::now().timestamp() + i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
    }

    /// Get a stored value, deserializing and checking for expiry.
    /// Returns `(StoredValue, mod_revision)` if found and not expired.
    async fn get_stored(&self, etcd_key: &str) -> Result<Option<(StoredValue, i64)>, StateError> {
        let mut client = self.client.lock().await;
        let resp = client
            .get(etcd_key, None)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        let Some(kv) = resp.kvs().first() else {
            return Ok(None);
        };

        let stored: StoredValue = serde_json::from_slice(kv.value())
            .map_err(|e| StateError::Serialization(e.to_string()))?;

        if stored.is_expired() {
            // Clean up the expired key in the background.
            let _ = client.delete(etcd_key, None).await;
            return Ok(None);
        }

        Ok(Some((stored, kv.mod_revision())))
    }
}

#[async_trait]
impl StateStore for EtcdStateStore {
    async fn check_and_set(
        &self,
        key: &StateKey,
        value: &str,
        ttl: Option<Duration>,
    ) -> Result<bool, StateError> {
        let etcd_key = self.config.render_key(key);

        // Check if the key already exists and is not expired.
        if let Some((existing, _)) = self.get_stored(&etcd_key).await? {
            if !existing.is_expired() {
                return Ok(false);
            }
        }

        let stored = StoredValue {
            value: value.to_owned(),
            version: 1,
            expires_at: Self::expires_at_from_ttl(ttl),
        };
        let payload =
            serde_json::to_vec(&stored).map_err(|e| StateError::Serialization(e.to_string()))?;

        // Transaction: only put if the key does not exist (create_revision == 0).
        let txn = Txn::new()
            .when([Compare::create_revision(
                etcd_key.clone(),
                CompareOp::Equal,
                0,
            )])
            .and_then([TxnOp::put(etcd_key, payload, None)])
            .or_else([]);

        let mut client = self.client.lock().await;
        let resp = client
            .txn(txn)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(resp.succeeded())
    }

    async fn get(&self, key: &StateKey) -> Result<Option<String>, StateError> {
        let etcd_key = self.config.render_key(key);

        match self.get_stored(&etcd_key).await? {
            Some((stored, _)) => Ok(Some(stored.value)),
            None => Ok(None),
        }
    }

    async fn set(
        &self,
        key: &StateKey,
        value: &str,
        ttl: Option<Duration>,
    ) -> Result<(), StateError> {
        let etcd_key = self.config.render_key(key);

        for _ in 0..MAX_RETRIES {
            let current = self.get_stored(&etcd_key).await?;

            let new_version = current.as_ref().map_or(1, |(stored, _)| stored.version + 1);

            let stored = StoredValue {
                value: value.to_owned(),
                version: new_version,
                expires_at: Self::expires_at_from_ttl(ttl),
            };
            let payload = serde_json::to_vec(&stored)
                .map_err(|e| StateError::Serialization(e.to_string()))?;

            let mut client = self.client.lock().await;

            if let Some((_, mod_revision)) = current {
                // Key exists: conditionally update if mod_revision matches.
                let txn = Txn::new()
                    .when([Compare::mod_revision(
                        etcd_key.clone(),
                        CompareOp::Equal,
                        mod_revision,
                    )])
                    .and_then([TxnOp::put(etcd_key.clone(), payload, None)])
                    .or_else([]);

                let resp = client
                    .txn(txn)
                    .await
                    .map_err(|e| StateError::Backend(e.to_string()))?;

                if resp.succeeded() {
                    return Ok(());
                }
                // Conflict: retry with fresh state.
            } else {
                // Key does not exist: unconditionally put.
                client
                    .put(etcd_key.clone(), payload, None)
                    .await
                    .map_err(|e| StateError::Backend(e.to_string()))?;
                return Ok(());
            }
        }

        Err(StateError::Backend(
            "set: exceeded maximum retries due to concurrent modifications".into(),
        ))
    }

    async fn delete(&self, key: &StateKey) -> Result<bool, StateError> {
        let etcd_key = self.config.render_key(key);

        // First check if it exists and is not expired.
        let exists = self.get_stored(&etcd_key).await?.is_some();

        if !exists {
            return Ok(false);
        }

        let mut client = self.client.lock().await;
        let resp = client
            .delete(etcd_key, None)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(resp.deleted() > 0)
    }

    async fn increment(
        &self,
        key: &StateKey,
        delta: i64,
        ttl: Option<Duration>,
    ) -> Result<i64, StateError> {
        let etcd_key = self.config.render_key(key);

        for _ in 0..MAX_RETRIES {
            let current = self.get_stored(&etcd_key).await?;

            let (current_val, new_version) = match &current {
                Some((stored, _)) => {
                    let val: i64 = stored.value.parse().map_err(|e: std::num::ParseIntError| {
                        StateError::Serialization(e.to_string())
                    })?;
                    (val, stored.version + 1)
                }
                None => (0, 1),
            };

            let new_val = current_val + delta;
            let stored = StoredValue {
                value: new_val.to_string(),
                version: new_version,
                expires_at: Self::expires_at_from_ttl(ttl),
            };
            let payload = serde_json::to_vec(&stored)
                .map_err(|e| StateError::Serialization(e.to_string()))?;

            let mut client = self.client.lock().await;

            if let Some((_, mod_revision)) = current {
                let txn = Txn::new()
                    .when([Compare::mod_revision(
                        etcd_key.clone(),
                        CompareOp::Equal,
                        mod_revision,
                    )])
                    .and_then([TxnOp::put(etcd_key.clone(), payload, None)])
                    .or_else([]);

                let resp = client
                    .txn(txn)
                    .await
                    .map_err(|e| StateError::Backend(e.to_string()))?;

                if resp.succeeded() {
                    return Ok(new_val);
                }
                // Conflict: retry.
            } else {
                // Key does not exist: create with transaction.
                let txn = Txn::new()
                    .when([Compare::create_revision(
                        etcd_key.clone(),
                        CompareOp::Equal,
                        0,
                    )])
                    .and_then([TxnOp::put(etcd_key.clone(), payload, None)])
                    .or_else([]);

                let resp = client
                    .txn(txn)
                    .await
                    .map_err(|e| StateError::Backend(e.to_string()))?;

                if resp.succeeded() {
                    return Ok(new_val);
                }
                // Another writer created the key: retry.
            }
        }

        Err(StateError::Backend(
            "increment: exceeded maximum retries due to concurrent modifications".into(),
        ))
    }

    async fn compare_and_swap(
        &self,
        key: &StateKey,
        expected_version: u64,
        new_value: &str,
        ttl: Option<Duration>,
    ) -> Result<CasResult, StateError> {
        let etcd_key = self.config.render_key(key);

        let current = self.get_stored(&etcd_key).await?;

        let Some((stored, mod_revision)) = current else {
            return Ok(CasResult::Conflict {
                current_value: None,
                current_version: 0,
            });
        };

        if stored.version != expected_version {
            return Ok(CasResult::Conflict {
                current_value: Some(stored.value),
                current_version: stored.version,
            });
        }

        let new_stored = StoredValue {
            value: new_value.to_owned(),
            version: stored.version + 1,
            expires_at: Self::expires_at_from_ttl(ttl),
        };
        let payload = serde_json::to_vec(&new_stored)
            .map_err(|e| StateError::Serialization(e.to_string()))?;

        // Transaction: only update if mod_revision has not changed.
        let txn = Txn::new()
            .when([Compare::mod_revision(
                etcd_key.clone(),
                CompareOp::Equal,
                mod_revision,
            )])
            .and_then([TxnOp::put(etcd_key.clone(), payload, None)])
            .or_else([TxnOp::get(etcd_key, None)]);

        let mut client = self.client.lock().await;
        let resp = client
            .txn(txn)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        if resp.succeeded() {
            return Ok(CasResult::Ok);
        }

        // Transaction failed: the key was concurrently modified.
        // Read the conflict value from the else branch response.
        for op_resp in resp.op_responses() {
            if let etcd_client::TxnOpResponse::Get(get_resp) = op_resp {
                if let Some(kv) = get_resp.kvs().first() {
                    let conflict_stored: StoredValue = serde_json::from_slice(kv.value())
                        .map_err(|e| StateError::Serialization(e.to_string()))?;
                    return Ok(CasResult::Conflict {
                        current_value: Some(conflict_stored.value),
                        current_version: conflict_stored.version,
                    });
                }
            }
        }

        Ok(CasResult::Conflict {
            current_value: None,
            current_version: 0,
        })
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
    async fn store_conformance() {
        let config = test_config();
        let store = EtcdStateStore::new(config)
            .await
            .expect("etcd connection should succeed");
        acteon_state::testing::run_store_conformance_tests(&store)
            .await
            .expect("conformance tests should pass");
    }
}
