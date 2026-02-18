//! Payload encryption for audit records.
//!
//! This module provides an [`EncryptingAuditStore`] wrapper that encrypts the
//! `action_payload` field in audit records before storage and decrypts it on
//! read. All other audit fields remain in plaintext so they are queryable.

use std::sync::Arc;

use async_trait::async_trait;

use acteon_crypto::PayloadEncryptor;

use crate::error::AuditError;
use crate::record::{AuditPage, AuditQuery, AuditRecord};
use crate::store::AuditStore;

/// An audit store wrapper that encrypts `action_payload` before storage and
/// decrypts it on read.
///
/// Wrapping order should be: `EncryptingAuditStore(RedactingAuditStore(Inner))`
/// so that redaction happens on plaintext before encryption.
pub struct EncryptingAuditStore {
    inner: Arc<dyn AuditStore>,
    encryptor: Arc<PayloadEncryptor>,
}

impl EncryptingAuditStore {
    /// Create a new `EncryptingAuditStore` wrapping the given inner store.
    pub fn new(inner: Arc<dyn AuditStore>, encryptor: Arc<PayloadEncryptor>) -> Self {
        Self { inner, encryptor }
    }

    /// Encrypt a JSON payload value into an `ENC[...]` string value.
    fn encrypt_payload(
        &self,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, AuditError> {
        let encrypted = self
            .encryptor
            .encrypt_json(payload)
            .map_err(|e| AuditError::Storage(format!("payload encryption failed: {e}")))?;
        Ok(serde_json::Value::String(encrypted))
    }

    /// Decrypt a payload value. If it is a string matching `ENC[...]`, decrypt
    /// and parse back to JSON. Otherwise pass through unchanged (backward compat).
    fn decrypt_payload(
        &self,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, AuditError> {
        if let serde_json::Value::String(s) = payload
            && acteon_crypto::is_encrypted(s)
        {
            return self
                .encryptor
                .decrypt_json(s)
                .map_err(|e| AuditError::Storage(format!("payload decryption failed: {e}")));
        }
        // Not encrypted — return as-is (backward compat with pre-encryption records).
        Ok(payload.clone())
    }

    /// Decrypt the `action_payload` field of a record in place.
    fn decrypt_record(&self, record: &mut AuditRecord) -> Result<(), AuditError> {
        if let Some(ref payload) = record.action_payload {
            record.action_payload = Some(self.decrypt_payload(payload)?);
        }
        Ok(())
    }
}

#[async_trait]
impl AuditStore for EncryptingAuditStore {
    async fn record(&self, entry: AuditRecord) -> Result<(), AuditError> {
        let mut encrypted = entry;
        if let Some(ref payload) = encrypted.action_payload {
            encrypted.action_payload = Some(self.encrypt_payload(payload)?);
        }
        self.inner.record(encrypted).await
    }

    async fn get_by_action_id(&self, action_id: &str) -> Result<Option<AuditRecord>, AuditError> {
        match self.inner.get_by_action_id(action_id).await? {
            Some(mut record) => {
                self.decrypt_record(&mut record)?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<AuditRecord>, AuditError> {
        match self.inner.get_by_id(id).await? {
            Some(mut record) => {
                self.decrypt_record(&mut record)?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }

    async fn query(&self, query: &AuditQuery) -> Result<AuditPage, AuditError> {
        let mut page = self.inner.query(query).await?;
        for record in &mut page.records {
            self.decrypt_record(record)?;
        }
        Ok(page)
    }

    async fn cleanup_expired(&self) -> Result<u64, AuditError> {
        self.inner.cleanup_expired().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{AuditPage, AuditQuery, AuditRecord};
    use acteon_crypto::parse_master_key;
    use serde_json::json;
    use std::sync::Mutex;

    fn test_encryptor() -> Arc<PayloadEncryptor> {
        let key = parse_master_key(&"42".repeat(32)).unwrap();
        Arc::new(PayloadEncryptor::new(key))
    }

    /// In-memory audit store for testing.
    struct MemoryAudit {
        records: Mutex<Vec<AuditRecord>>,
    }

    impl MemoryAudit {
        fn new() -> Self {
            Self {
                records: Mutex::new(Vec::new()),
            }
        }

        /// Get the raw stored records (without decryption) for inspection.
        fn raw_records(&self) -> Vec<AuditRecord> {
            self.records.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl AuditStore for MemoryAudit {
        async fn record(&self, entry: AuditRecord) -> Result<(), AuditError> {
            self.records.lock().unwrap().push(entry);
            Ok(())
        }

        async fn get_by_action_id(
            &self,
            action_id: &str,
        ) -> Result<Option<AuditRecord>, AuditError> {
            Ok(self
                .records
                .lock()
                .unwrap()
                .iter()
                .find(|r| r.action_id == action_id)
                .cloned())
        }

        async fn get_by_id(&self, id: &str) -> Result<Option<AuditRecord>, AuditError> {
            Ok(self
                .records
                .lock()
                .unwrap()
                .iter()
                .find(|r| r.id == id)
                .cloned())
        }

        async fn query(&self, _query: &AuditQuery) -> Result<AuditPage, AuditError> {
            let records = self.records.lock().unwrap().clone();
            Ok(AuditPage {
                records,
                total: 0,
                limit: 100,
                offset: 0,
            })
        }

        async fn cleanup_expired(&self) -> Result<u64, AuditError> {
            Ok(0)
        }
    }

    fn make_record(id: &str, payload: Option<serde_json::Value>) -> AuditRecord {
        let now = chrono::Utc::now();
        AuditRecord {
            id: id.to_string(),
            action_id: format!("action-{id}"),
            chain_id: None,
            namespace: "ns".to_string(),
            tenant: "t".to_string(),
            provider: "webhook".to_string(),
            action_type: "test".to_string(),
            verdict: "allow".to_string(),
            matched_rule: None,
            outcome: "executed".to_string(),
            action_payload: payload,
            verdict_details: json!({}),
            outcome_details: json!({}),
            metadata: json!({}),
            dispatched_at: now,
            completed_at: now,
            duration_ms: 10,
            expires_at: None,
            caller_id: String::new(),
            auth_method: String::new(),
            record_hash: None,
            previous_hash: None,
            sequence_number: None,
        }
    }

    #[tokio::test]
    async fn roundtrip_encrypt_decrypt() {
        let inner = Arc::new(MemoryAudit::new());
        let enc = test_encryptor();
        let store = EncryptingAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>, enc);

        let record = make_record("r1", Some(json!({"secret": "value123"})));
        store.record(record).await.unwrap();

        // Raw stored payload should be encrypted.
        let raw = &inner.raw_records()[0];
        if let Some(serde_json::Value::String(s)) = &raw.action_payload {
            assert!(
                acteon_crypto::is_encrypted(s),
                "stored payload should be encrypted"
            );
        } else {
            panic!("expected encrypted string payload");
        }

        // Reading back should decrypt.
        let fetched = store.get_by_id("r1").await.unwrap().unwrap();
        assert_eq!(fetched.action_payload, Some(json!({"secret": "value123"})));
    }

    #[tokio::test]
    async fn backward_compat_plain_records() {
        let inner = Arc::new(MemoryAudit::new());
        let enc = test_encryptor();
        let store = EncryptingAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>, enc);

        // Insert a plain (unencrypted) record directly into the inner store.
        let record = make_record("r2", Some(json!({"plain": true})));
        inner.record(record).await.unwrap();

        // Reading through the encrypting store should still work.
        let fetched = store.get_by_id("r2").await.unwrap().unwrap();
        assert_eq!(fetched.action_payload, Some(json!({"plain": true})));
    }

    #[tokio::test]
    async fn no_payload_passthrough() {
        let inner = Arc::new(MemoryAudit::new());
        let enc = test_encryptor();
        let store = EncryptingAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>, enc);

        let record = make_record("r3", None);
        store.record(record).await.unwrap();

        let fetched = store.get_by_id("r3").await.unwrap().unwrap();
        assert!(fetched.action_payload.is_none());
    }

    #[tokio::test]
    async fn query_decrypts_all_records() {
        let inner = Arc::new(MemoryAudit::new());
        let enc = test_encryptor();
        let store = EncryptingAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>, enc);

        store
            .record(make_record("q1", Some(json!({"a": 1}))))
            .await
            .unwrap();
        store
            .record(make_record("q2", Some(json!({"b": 2}))))
            .await
            .unwrap();

        let page = store
            .query(&AuditQuery {
                limit: Some(10),
                ..AuditQuery::default()
            })
            .await
            .unwrap();

        assert_eq!(page.records.len(), 2);
        assert_eq!(page.records[0].action_payload, Some(json!({"a": 1})));
        assert_eq!(page.records[1].action_payload, Some(json!({"b": 2})));
    }

    #[tokio::test]
    async fn get_by_action_id_decrypts() {
        let inner = Arc::new(MemoryAudit::new());
        let enc = test_encryptor();
        let store = EncryptingAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>, enc);

        store
            .record(make_record("a1", Some(json!({"key": "val"}))))
            .await
            .unwrap();

        let fetched = store.get_by_action_id("action-a1").await.unwrap().unwrap();
        assert_eq!(fetched.action_payload, Some(json!({"key": "val"})));
    }
}
