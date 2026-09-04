//! Payload encryption at the dead-letter storage boundary.
//! Encryption failures never reach the underlying sink. Unreadable ciphertext
//! is retained in the underlying queue and excluded from redelivery.

use acteon_core::Action;
use acteon_crypto::{CryptoError, PayloadEncryptor};
use acteon_executor::{DeadLetterEntry, DeadLetterError, DeadLetterSink};
use async_trait::async_trait;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

/// Injectable cryptographic boundary, including key-service failure handling.
pub trait DeadLetterCipher: Send + Sync {
    fn encrypt(&self, payload: &serde_json::Value) -> Result<String, CryptoError>;
    fn decrypt(&self, payload: &str) -> Result<serde_json::Value, CryptoError>;
}

impl DeadLetterCipher for PayloadEncryptor {
    fn encrypt(&self, payload: &serde_json::Value) -> Result<String, CryptoError> {
        self.encrypt_json(payload)
    }
    fn decrypt(&self, payload: &str) -> Result<serde_json::Value, CryptoError> {
        self.decrypt_json(payload)
    }
}

/// Encrypts payloads before persistence; failures are counted without logging payloads.
pub struct EncryptingDeadLetterSink {
    inner: Arc<dyn DeadLetterSink>,
    encryptor: Arc<dyn DeadLetterCipher>,
    failures: AtomicU64,
}

impl EncryptingDeadLetterSink {
    pub fn new(inner: Arc<dyn DeadLetterSink>, encryptor: Arc<PayloadEncryptor>) -> Self {
        Self::with_cipher(inner, encryptor)
    }

    pub fn with_cipher(
        inner: Arc<dyn DeadLetterSink>,
        encryptor: Arc<dyn DeadLetterCipher>,
    ) -> Self {
        Self {
            inner,
            encryptor,
            failures: AtomicU64::new(0),
        }
    }

    fn failure(&self, operation: &str) -> DeadLetterError {
        self.failures.fetch_add(1, Ordering::Relaxed);
        // Cipher errors may contain sensitive material. Record only the operation.
        tracing::error!(operation, "dead-letter encryption boundary failed");
        DeadLetterError(format!("{operation} failed"))
    }
}

#[async_trait]
impl DeadLetterSink for EncryptingDeadLetterSink {
    async fn push(
        &self,
        mut action: Action,
        error: String,
        attempts: u32,
    ) -> Result<(), DeadLetterError> {
        let encrypted = self
            .encryptor
            .encrypt(&action.payload)
            .map_err(|_| self.failure("encryption"))?;
        action.payload = serde_json::Value::String(encrypted);
        self.inner
            .push(action, error, attempts)
            .await
            .map_err(|_| self.failure("storage"))
    }

    fn failure_count(&self) -> u64 {
        self.failures.load(Ordering::Relaxed) + self.inner.failure_count()
    }

    async fn drain(&self) -> Vec<DeadLetterEntry> {
        let mut readable = Vec::new();
        for mut entry in self.inner.drain().await {
            if let serde_json::Value::String(ref encrypted) = entry.action.payload
                && acteon_crypto::is_encrypted(encrypted)
            {
                if let Ok(payload) = self.encryptor.decrypt(encrypted) {
                    entry.action.payload = payload;
                } else {
                    self.failure("decryption");
                    // Bypass this wrapper: the entry is already encrypted.
                    if self
                        .inner
                        .push(entry.action, entry.error, entry.attempts)
                        .await
                        .is_err()
                    {
                        self.failure("ciphertext retention");
                    }
                    continue;
                }
            }
            readable.push(entry);
        }
        readable
    }

    async fn len(&self) -> usize {
        self.inner.len().await
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
        sink.push(action, "test error".into(), 3).await.unwrap();

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
        sink.push(action, "err".into(), 1).await.unwrap();

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
            .await
            .unwrap();

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

        sink.push(action, "permanent failure".into(), 5)
            .await
            .unwrap();

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
            .await
            .unwrap();

        // Drain through encrypting wrapper.
        let sink = EncryptingDeadLetterSink::new(Arc::clone(&inner), enc);
        let entries = sink.drain().await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action.payload, payload);
    }
    struct FailingCipher;
    impl DeadLetterCipher for FailingCipher {
        fn encrypt(&self, _: &serde_json::Value) -> Result<String, CryptoError> {
            Err(CryptoError::EncryptionFailed("injected outage".into()))
        }
        fn decrypt(&self, _: &str) -> Result<serde_json::Value, CryptoError> {
            Err(CryptoError::DecryptionFailed)
        }
    }

    #[tokio::test]
    async fn encryption_failure_never_persists_plaintext() {
        let inner = Arc::new(DeadLetterQueue::new());
        let sink = EncryptingDeadLetterSink::with_cipher(inner.clone(), Arc::new(FailingCipher));
        assert!(
            sink.push(
                test_action(serde_json::json!({"secret":"never-store"})),
                "err".into(),
                1
            )
            .await
            .is_err()
        );
        assert!(inner.is_empty());
        assert_eq!(sink.failure_count(), 1);
    }

    #[tokio::test]
    async fn decryption_failure_retains_ciphertext_without_redelivery() {
        let inner = Arc::new(DeadLetterQueue::new());
        let encrypted = test_encryptor()
            .encrypt_json(&serde_json::json!({"secret":"x"}))
            .unwrap();
        inner.push(
            test_action(serde_json::Value::String(encrypted.clone())),
            "err".into(),
            1,
        );
        let sink = EncryptingDeadLetterSink::with_cipher(inner.clone(), Arc::new(FailingCipher));
        assert!(sink.drain().await.is_empty());
        assert_eq!(sink.failure_count(), 1);
        assert_eq!(inner.drain()[0].action.payload, encrypted);
    }
}
