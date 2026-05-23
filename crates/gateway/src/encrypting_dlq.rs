//! Encrypting wrapper for dead-letter queue sinks.
//!
//! [`EncryptingDeadLetterSink`] intercepts `push` and `drain` calls to
//! encrypt action payloads before storage and decrypt them on retrieval.
//! This closes the DLQ plaintext island — even for the in-memory backend,
//! payloads are held only in ciphertext, providing defense-in-depth.

use std::sync::Arc;

use acteon_core::Action;
use acteon_crypto::PayloadEncryptor;
use acteon_executor::{DeadLetterEntry, DeadLetterSink};
use async_trait::async_trait;

/// A [`DeadLetterSink`] wrapper that encrypts action payloads before
/// delegating to the inner sink, and decrypts on drain.
///
/// `len()` and `is_empty()` are delegated directly without transformation.
pub struct EncryptingDeadLetterSink {
    inner: Arc<dyn DeadLetterSink>,
    encryptor: Arc<PayloadEncryptor>,
}

impl EncryptingDeadLetterSink {
    /// Create a new encrypting wrapper around an existing DLQ sink.
    pub fn new(inner: Arc<dyn DeadLetterSink>, encryptor: Arc<PayloadEncryptor>) -> Self {
        Self { inner, encryptor }
    }

    /// Encrypt an action's payload in place.
    ///
    /// Serializes the payload JSON, encrypts it, and replaces the payload
    /// with `Value::String(encrypted_envelope)`.
    fn encrypt_payload(&self, action: &mut Action) {
        match self.encryptor.encrypt_json(&action.payload) {
            Ok(encrypted) => {
                action.payload = serde_json::Value::String(encrypted);
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to encrypt DLQ payload, storing as-is");
            }
        }
    }

    /// Decrypt an action's payload in place.
    ///
    /// If the payload is a `Value::String` matching the `ENC[...]` envelope,
    /// it is decrypted and parsed back to the original JSON value. Non-encrypted
    /// payloads pass through unchanged.
    fn decrypt_payload(&self, action: &mut Action) {
        if let serde_json::Value::String(ref s) = action.payload
            && acteon_crypto::is_encrypted(s)
        {
            match self.encryptor.decrypt_json(s) {
                Ok(decrypted) => {
                    action.payload = decrypted;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to decrypt DLQ payload, returning as-is");
                }
            }
        }
    }
}

#[async_trait]
impl DeadLetterSink for EncryptingDeadLetterSink {
    async fn push(&self, mut action: Action, error: String, attempts: u32) {
        self.encrypt_payload(&mut action);
        self.inner.push(action, error, attempts).await;
    }

    async fn drain(&self) -> Vec<DeadLetterEntry> {
        let mut entries = self.inner.drain().await;
        for entry in &mut entries {
            self.decrypt_payload(&mut entry.action);
        }
        entries
    }

    async fn len(&self) -> usize {
        self.inner.len().await
    }

    async fn is_empty(&self) -> bool {
        self.inner.is_empty().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_crypto::parse_master_key;
    use acteon_executor::DeadLetterQueue;

    fn test_encryptor() -> Arc<PayloadEncryptor> {
        let key = parse_master_key(&"ab".repeat(32)).unwrap();
        Arc::new(PayloadEncryptor::new(key))
    }

    fn test_action(payload: serde_json::Value) -> Action {
        Action::new("ns", "tenant", "provider", "type", payload)
    }

    #[tokio::test]
    async fn push_encrypts_payload() {
        let inner = Arc::new(DeadLetterQueue::new());
        let enc = test_encryptor();
        let sink =
            EncryptingDeadLetterSink::new(Arc::clone(&inner) as Arc<dyn DeadLetterSink>, enc);

        let action = test_action(serde_json::json!({"secret": "hunter2"}));
        sink.push(action, "test error".into(), 3).await;

        // Read directly from inner — payload should be encrypted.
        let entries = inner.drain();
        assert_eq!(entries.len(), 1);
        match &entries[0].action.payload {
            serde_json::Value::String(s) => {
                assert!(
                    acteon_crypto::is_encrypted(s),
                    "inner DLQ should hold encrypted payload"
                );
            }
            other => panic!("expected String(ENC[...]), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn drain_decrypts_payload() {
        let inner: Arc<dyn DeadLetterSink> = Arc::new(DeadLetterQueue::new());
        let enc = test_encryptor();
        let sink = EncryptingDeadLetterSink::new(Arc::clone(&inner), Arc::clone(&enc));

        let original = serde_json::json!({"api_key": "sk-12345", "nested": [1, 2]});
        let action = test_action(original.clone());
        sink.push(action, "err".into(), 1).await;

        // Drain through encrypting wrapper — should get back plaintext.
        let entries = sink.drain().await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action.payload, original);
        assert_eq!(entries[0].error, "err");
        assert_eq!(entries[0].attempts, 1);
    }

    #[tokio::test]
    async fn len_and_is_empty_delegate() {
        let inner: Arc<dyn DeadLetterSink> = Arc::new(DeadLetterQueue::new());
        let enc = test_encryptor();
        let sink = EncryptingDeadLetterSink::new(Arc::clone(&inner), enc);

        assert!(sink.is_empty().await);
        assert_eq!(sink.len().await, 0);

        sink.push(test_action(serde_json::json!({})), "e".into(), 1)
            .await;

        assert!(!sink.is_empty().await);
        assert_eq!(sink.len().await, 1);
    }

    #[tokio::test]
    async fn roundtrip_preserves_all_fields() {
        let inner: Arc<dyn DeadLetterSink> = Arc::new(DeadLetterQueue::new());
        let enc = test_encryptor();
        let sink = EncryptingDeadLetterSink::new(Arc::clone(&inner), Arc::clone(&enc));

        let payload = serde_json::json!({
            "user": "alice",
            "ssn": "123-45-6789",
            "nested": {"key": "value"},
            "list": [true, null, 42]
        });
        let action = test_action(payload.clone());
        let action_id = action.id.clone();

        sink.push(action, "permanent failure".into(), 5).await;

        let entries = sink.drain().await;
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.action.id, action_id);
        assert_eq!(entry.action.payload, payload);
        assert_eq!(entry.error, "permanent failure");
        assert_eq!(entry.attempts, 5);
        assert_eq!(entry.action.namespace.as_str(), "ns");
        assert_eq!(entry.action.tenant.as_str(), "tenant");
    }

    #[tokio::test]
    async fn non_encrypted_payloads_pass_through_on_drain() {
        // Simulate a DLQ entry that was pushed without encryption (e.g.,
        // before encryption was enabled).
        let inner: Arc<dyn DeadLetterSink> = Arc::new(DeadLetterQueue::new());
        let enc = test_encryptor();

        // Push directly to inner (unencrypted).
        let payload = serde_json::json!({"plain": true});
        inner
            .push(test_action(payload.clone()), "e".into(), 1)
            .await;

        // Drain through encrypting wrapper.
        let sink = EncryptingDeadLetterSink::new(Arc::clone(&inner), enc);
        let entries = sink.drain().await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action.payload, payload);
    }
}
