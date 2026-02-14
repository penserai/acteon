//! Shared AES-256-GCM encryption utilities for Acteon config secrets.
//!
//! Values are stored in the format:
//! `ENC[AES256-GCM,kid:<id>,data:<b64>,iv:<b64>,tag:<b64>]`
//!
//! The `kid` (key identifier) field is optional for backward compatibility.
//! Legacy envelopes without `kid` are treated as `kid="k0"`.
//!
//! Decrypted values are returned as [`SecretString`] to prevent accidental
//! logging. The [`MasterKey`] wrapper zeroizes key material on drop.

use std::fmt;
use std::sync::LazyLock;

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use regex::Regex;
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop};

// Re-export for consumers so they don't need a direct `secrecy` dependency.
pub use secrecy::{ExposeSecret, Secret, SecretString};

/// Compiled regex for parsing `ENC[AES256-GCM,...]` envelopes.
///
/// Supports two formats:
/// - With `kid`: `ENC[AES256-GCM,kid:<id>,data:<b64>,iv:<b64>,tag:<b64>]`
/// - Without `kid` (legacy): `ENC[AES256-GCM,data:<b64>,iv:<b64>,tag:<b64>]`
///
/// Captures four groups: kid (optional), data, iv, and tag.
static ENC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^ENC\[AES256-GCM,(?:kid:([A-Za-z0-9_-]+),)?data:([A-Za-z0-9+/=]+),iv:([A-Za-z0-9+/=]+),tag:([A-Za-z0-9+/=]+)\]$",
    )
    .expect("ENC regex is valid")
});

/// A 32-byte AES-256 master key that is zeroized when dropped.
///
/// Prevents key material from lingering in memory after the key is no longer
/// needed. The [`Debug`] implementation is redacted to avoid accidental logging.
///
/// Raw bytes are not accessible outside this crate — all cryptographic
/// operations go through the functions in this module.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct MasterKey([u8; 32]);

impl MasterKey {
    /// Access the raw key bytes (crate-internal only).
    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MasterKey([REDACTED])")
    }
}

/// Errors that can occur during encryption/decryption operations.
#[derive(Debug, Error)]
pub enum CryptoError {
    /// The provided master key is not valid (wrong length or encoding).
    #[error("invalid master key: {0}")]
    InvalidKey(String),

    /// The encrypted value format is malformed.
    #[error("invalid encrypted value: {0}")]
    InvalidFormat(String),

    /// Decryption failed — wrong key or corrupted data.
    #[error("decryption failed (wrong key or corrupted data)")]
    DecryptionFailed,

    /// Encryption failed.
    #[error("encryption failed: {0}")]
    EncryptionFailed(String),
}

/// Parse a 32-byte master key from hex or base64.
///
/// Accepts either 64 hex characters or a base64 string that decodes to exactly
/// 32 bytes. The returned [`MasterKey`] is zeroized on drop.
pub fn parse_master_key(raw: &str) -> Result<MasterKey, CryptoError> {
    let trimmed = raw.trim();
    // Try hex first (64 hex chars = 32 bytes).
    if trimmed.len() == 64
        && let Ok(bytes) = hex::decode(trimmed)
        && bytes.len() == 32
    {
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        return Ok(MasterKey(key));
    }
    // Try base64.
    if let Ok(bytes) = B64.decode(trimmed)
        && bytes.len() == 32
    {
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        return Ok(MasterKey(key));
    }
    Err(CryptoError::InvalidKey(
        "must be 32 bytes encoded as 64 hex chars or base64".to_owned(),
    ))
}

/// Returns `true` if `value` looks like an encrypted `ENC[AES256-GCM,...]` marker.
#[must_use]
pub fn is_encrypted(value: &str) -> bool {
    ENC_RE.is_match(value.trim())
}

/// If `value` starts with `ENC[AES256-GCM,...]`, decrypt it using the master
/// key. Otherwise return the value unchanged (pass-through).
///
/// Returns a [`SecretString`] to prevent accidental logging of decrypted
/// secrets.
pub fn decrypt_value(value: &str, master_key: &MasterKey) -> Result<SecretString, CryptoError> {
    let trimmed = value.trim();

    let Some(caps) = ENC_RE.captures(trimmed) else {
        // Not an ENC[...] envelope — pass through unchanged.
        return Ok(SecretString::new(value.to_owned()));
    };

    // Group 1 = kid (optional), Group 2 = data, Group 3 = iv, Group 4 = tag
    let data = B64
        .decode(&caps[2])
        .map_err(|e| CryptoError::InvalidFormat(format!("invalid base64 in data: {e}")))?;
    let iv = B64
        .decode(&caps[3])
        .map_err(|e| CryptoError::InvalidFormat(format!("invalid base64 in iv: {e}")))?;
    let tag = B64
        .decode(&caps[4])
        .map_err(|e| CryptoError::InvalidFormat(format!("invalid base64 in tag: {e}")))?;

    if iv.len() != 12 {
        return Err(CryptoError::InvalidFormat(format!(
            "IV must be 12 bytes, got {}",
            iv.len()
        )));
    }
    if tag.len() != 16 {
        return Err(CryptoError::InvalidFormat(format!(
            "tag must be 16 bytes, got {}",
            tag.len()
        )));
    }

    // AES-GCM ciphertext = data || tag
    let mut ciphertext = data;
    ciphertext.extend_from_slice(&tag);

    let cipher = Aes256Gcm::new_from_slice(master_key.as_bytes())
        .map_err(|e| CryptoError::InvalidKey(format!("invalid AES key: {e}")))?;
    let nonce = Nonce::from_slice(&iv);

    let plaintext = cipher
        .decrypt(nonce, ciphertext.as_ref())
        .map_err(|_| CryptoError::DecryptionFailed)?;

    let s = String::from_utf8(plaintext)
        .map_err(|e| CryptoError::InvalidFormat(format!("decrypted value is not UTF-8: {e}")))?;

    Ok(SecretString::new(s))
}

/// Encrypt a plaintext string, producing an `ENC[AES256-GCM,...]` marker.
///
/// The returned string is the encrypted envelope (not secret itself — it is
/// meant to be stored in configuration files).
pub fn encrypt_value(plaintext: &str, master_key: &MasterKey) -> Result<String, CryptoError> {
    encrypt_value_with_kid(plaintext, master_key, None)
}

/// Encrypt a plaintext string with an optional key identifier (`kid`).
///
/// When `kid` is `Some`, the envelope includes a `kid:<id>,` field:
/// `ENC[AES256-GCM,kid:<id>,data:<b64>,iv:<b64>,tag:<b64>]`
pub fn encrypt_value_with_kid(
    plaintext: &str,
    master_key: &MasterKey,
    kid: Option<&str>,
) -> Result<String, CryptoError> {
    use aes_gcm::AeadCore;

    let cipher = Aes256Gcm::new_from_slice(master_key.as_bytes())
        .map_err(|e| CryptoError::InvalidKey(format!("invalid AES key: {e}")))?;
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| CryptoError::EncryptionFailed(e.to_string()))?;

    // AES-GCM output = ciphertext_data || 16-byte tag
    let (data, tag) = ciphertext.split_at(ciphertext.len() - 16);

    let kid_part = match kid {
        Some(id) => format!("kid:{id},"),
        None => String::new(),
    };

    Ok(format!(
        "ENC[AES256-GCM,{kid_part}data:{},iv:{},tag:{}]",
        B64.encode(data),
        B64.encode(nonce.as_slice()),
        B64.encode(tag),
    ))
}

/// Extract the key identifier (`kid`) from an `ENC[...]` envelope.
///
/// Returns `None` if the value is not encrypted or has no `kid` field (legacy).
#[must_use]
pub fn extract_kid(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let caps = ENC_RE.captures(trimmed)?;
    caps.get(1).map(|m| m.as_str().to_owned())
}

/// A named key entry for the [`PayloadEncryptor`].
pub struct PayloadKeyEntry {
    /// Key identifier embedded in the `ENC[...]` envelope.
    pub kid: String,
    /// The AES-256 key material.
    pub key: MasterKey,
}

/// Encrypts and decrypts action payloads at rest.
///
/// Supports multiple keys for key rotation. The first key in the set is used
/// for encryption; all keys are tried for decryption (matched by `kid` in the
/// envelope, with fallback to trying all keys for legacy envelopes without a
/// `kid`).
///
/// Plaintext (non-`ENC[...]`) values pass through `decrypt_*` methods
/// unchanged, ensuring backward compatibility with data written before
/// encryption was enabled.
pub struct PayloadEncryptor {
    keys: Vec<PayloadKeyEntry>,
}

impl PayloadEncryptor {
    /// Create a new encryptor from a single [`MasterKey`].
    ///
    /// The key is assigned `kid="k0"` for backward compatibility.
    pub fn new(key: MasterKey) -> Self {
        Self {
            keys: vec![PayloadKeyEntry {
                kid: "k0".to_owned(),
                key,
            }],
        }
    }

    /// Create a new encryptor from multiple named keys.
    ///
    /// The first key (`keys[0]`) is used for encryption. All keys are
    /// available for decryption. Panics if `keys` is empty.
    pub fn with_keys(keys: Vec<PayloadKeyEntry>) -> Self {
        assert!(
            !keys.is_empty(),
            "PayloadEncryptor requires at least one key"
        );
        Self { keys }
    }

    /// Return the current encryption key's `kid`.
    #[must_use]
    pub fn current_kid(&self) -> &str {
        &self.keys[0].kid
    }

    /// Encrypt a [`serde_json::Value`], returning an `ENC[...]` envelope string.
    ///
    /// The value is serialized to a JSON string before encryption.
    pub fn encrypt_json(&self, value: &serde_json::Value) -> Result<String, CryptoError> {
        let plain = serde_json::to_string(value).map_err(|e| {
            CryptoError::EncryptionFailed(format!("JSON serialization failed: {e}"))
        })?;
        let entry = &self.keys[0];
        encrypt_value_with_kid(&plain, &entry.key, Some(&entry.kid))
    }

    /// Decrypt a string that may be an `ENC[...]` envelope back into a
    /// [`serde_json::Value`].
    ///
    /// If the input is a plain (non-encrypted) string, it is parsed as JSON
    /// directly, providing backward compatibility with pre-encryption data.
    pub fn decrypt_json(&self, value: &str) -> Result<serde_json::Value, CryptoError> {
        let plain = self.decrypt_raw(value)?;
        serde_json::from_str(plain.expose_secret())
            .map_err(|e| CryptoError::InvalidFormat(format!("JSON parse failed: {e}")))
    }

    /// Encrypt a plaintext string, returning an `ENC[...]` envelope.
    pub fn encrypt_str(&self, value: &str) -> Result<String, CryptoError> {
        let entry = &self.keys[0];
        encrypt_value_with_kid(value, &entry.key, Some(&entry.kid))
    }

    /// Decrypt a string that may be an `ENC[...]` envelope back to plaintext.
    ///
    /// Non-encrypted strings pass through unchanged.
    pub fn decrypt_str(&self, value: &str) -> Result<String, CryptoError> {
        Ok(self.decrypt_raw(value)?.expose_secret().clone())
    }

    /// Core multi-key decryption logic.
    ///
    /// 1. If the value is not an `ENC[...]` envelope, pass through.
    /// 2. Extract `kid` from envelope — if present, look up key by `kid`.
    /// 3. If `kid` not found or missing (legacy), try all keys in order.
    fn decrypt_raw(&self, value: &str) -> Result<SecretString, CryptoError> {
        let trimmed = value.trim();

        if !ENC_RE.is_match(trimmed) {
            return Ok(SecretString::new(value.to_owned()));
        }

        // Try matching kid first for direct lookup.
        let kid = extract_kid(trimmed);

        if let Some(ref kid_str) = kid
            && let Some(entry) = self.keys.iter().find(|e| e.kid == *kid_str)
        {
            return decrypt_value(trimmed, &entry.key);
        }

        // Fallback: try all keys in order (handles legacy no-kid envelopes
        // and cases where kid is present but not in our set).
        let mut last_err = CryptoError::DecryptionFailed;
        for entry in &self.keys {
            match decrypt_value(trimmed, &entry.key) {
                Ok(plaintext) => return Ok(plaintext),
                Err(e) => last_err = e,
            }
        }
        Err(last_err)
    }
}

impl fmt::Debug for PayloadEncryptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PayloadEncryptor([REDACTED])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> MasterKey {
        parse_master_key(&"42".repeat(32)).unwrap()
    }

    fn test_key_2() -> MasterKey {
        parse_master_key(&"ab".repeat(32)).unwrap()
    }

    #[test]
    fn roundtrip_encrypt_decrypt() {
        let key = test_key();
        let plaintext = "my-secret-value";
        let encrypted = encrypt_value(plaintext, &key).unwrap();
        assert!(encrypted.starts_with("ENC[AES256-GCM,"));
        let decrypted = decrypt_value(&encrypted, &key).unwrap();
        assert_eq!(decrypted.expose_secret(), plaintext);
    }

    #[test]
    fn passthrough_plain_value() {
        let key = test_key();
        let plain = "not-encrypted";
        let result = decrypt_value(plain, &key).unwrap();
        assert_eq!(result.expose_secret(), plain);
    }

    #[test]
    fn parse_hex_key() {
        let hex_key = "aa".repeat(32);
        let key = parse_master_key(&hex_key).unwrap();
        assert_eq!(key.as_bytes(), &[0xaa; 32]);
    }

    #[test]
    fn parse_base64_key() {
        let raw = [0xbbu8; 32];
        let b64 = B64.encode(raw);
        let key = parse_master_key(&b64).unwrap();
        assert_eq!(key.as_bytes(), &[0xbb; 32]);
    }

    #[test]
    fn is_encrypted_detects_enc_prefix() {
        assert!(is_encrypted("ENC[AES256-GCM,data:abc,iv:def,tag:ghi]"));
        assert!(is_encrypted(
            "ENC[AES256-GCM,kid:k1,data:abc,iv:def,tag:ghi]"
        ));
        assert!(!is_encrypted("plain-text-value"));
        assert!(!is_encrypted("ENC[AES256-GCM,incomplete"));
    }

    #[test]
    fn decrypt_invalid_format_missing_data() {
        let key = test_key();
        // Missing data field — regex won't match, so it'll pass through.
        // Use a value that looks like ENC but has bad structure:
        let bad = "ENC[AES256-GCM,data:AAAA,iv:AAAA,tag:AAAA]";
        // The regex matches, but the decoded iv/tag have wrong lengths.
        let err = decrypt_value(bad, &key).unwrap_err();
        assert!(matches!(err, CryptoError::InvalidFormat(_)));
    }

    #[test]
    fn parse_master_key_rejects_short() {
        let err = parse_master_key("too-short").unwrap_err();
        assert!(matches!(err, CryptoError::InvalidKey(_)));
    }

    #[test]
    fn master_key_debug_is_redacted() {
        let key = test_key();
        let debug = format!("{key:?}");
        assert_eq!(debug, "MasterKey([REDACTED])");
        assert!(!debug.contains("42"));
    }

    #[test]
    fn malformed_enc_passes_through() {
        let key = test_key();
        // Looks like ENC but doesn't match the regex — treated as plain value.
        let malformed = "ENC[AES256-GCM,garbage]";
        let result = decrypt_value(malformed, &key).unwrap();
        assert_eq!(result.expose_secret(), malformed);
    }

    // -----------------------------------------------------------------------
    // encrypt_value_with_kid tests
    // -----------------------------------------------------------------------

    #[test]
    fn encrypt_with_kid_embeds_kid_in_envelope() {
        let key = test_key();
        let encrypted = encrypt_value_with_kid("hello", &key, Some("k1")).unwrap();
        assert!(encrypted.contains("kid:k1,"), "should contain kid:k1");
        assert!(encrypted.starts_with("ENC[AES256-GCM,kid:k1,data:"));
        // Should still decrypt with the same key.
        let decrypted = decrypt_value(&encrypted, &key).unwrap();
        assert_eq!(decrypted.expose_secret(), "hello");
    }

    #[test]
    fn encrypt_without_kid_produces_legacy_format() {
        let key = test_key();
        let encrypted = encrypt_value_with_kid("hello", &key, None).unwrap();
        assert!(!encrypted.contains("kid:"), "should not contain kid");
        assert!(encrypted.starts_with("ENC[AES256-GCM,data:"));
        let decrypted = decrypt_value(&encrypted, &key).unwrap();
        assert_eq!(decrypted.expose_secret(), "hello");
    }

    #[test]
    fn extract_kid_returns_kid_when_present() {
        let key = test_key();
        let encrypted = encrypt_value_with_kid("data", &key, Some("rotation-2")).unwrap();
        assert_eq!(extract_kid(&encrypted), Some("rotation-2".to_owned()));
    }

    #[test]
    fn extract_kid_returns_none_for_legacy() {
        let key = test_key();
        let encrypted = encrypt_value("data", &key).unwrap();
        assert_eq!(extract_kid(&encrypted), None);
    }

    #[test]
    fn extract_kid_returns_none_for_plaintext() {
        assert_eq!(extract_kid("not-encrypted"), None);
    }

    // -----------------------------------------------------------------------
    // PayloadEncryptor tests
    // -----------------------------------------------------------------------

    #[test]
    fn payload_encryptor_roundtrip_json_object() {
        let enc = PayloadEncryptor::new(test_key());
        let value = serde_json::json!({"user": "alice", "amount": 42});
        let encrypted = enc.encrypt_json(&value).unwrap();
        assert!(encrypted.starts_with("ENC[AES256-GCM,"));
        let decrypted = enc.decrypt_json(&encrypted).unwrap();
        assert_eq!(decrypted, value);
    }

    #[test]
    fn payload_encryptor_roundtrip_json_array() {
        let enc = PayloadEncryptor::new(test_key());
        let value = serde_json::json!([1, "two", null, true]);
        let encrypted = enc.encrypt_json(&value).unwrap();
        let decrypted = enc.decrypt_json(&encrypted).unwrap();
        assert_eq!(decrypted, value);
    }

    #[test]
    fn payload_encryptor_roundtrip_json_null() {
        let enc = PayloadEncryptor::new(test_key());
        let value = serde_json::Value::Null;
        let encrypted = enc.encrypt_json(&value).unwrap();
        let decrypted = enc.decrypt_json(&encrypted).unwrap();
        assert_eq!(decrypted, value);
    }

    #[test]
    fn payload_encryptor_roundtrip_nested() {
        let enc = PayloadEncryptor::new(test_key());
        let value = serde_json::json!({
            "action": {"id": "a1", "payload": {"key": "secret"}},
            "scheduled_for": "2026-03-01T00:00:00Z"
        });
        let encrypted = enc.encrypt_json(&value).unwrap();
        let decrypted = enc.decrypt_json(&encrypted).unwrap();
        assert_eq!(decrypted, value);
    }

    #[test]
    fn payload_encryptor_decrypt_plain_json_passthrough() {
        let enc = PayloadEncryptor::new(test_key());
        let plain = r#"{"user":"alice","amount":42}"#;
        let decrypted = enc.decrypt_json(plain).unwrap();
        assert_eq!(
            decrypted,
            serde_json::json!({"user": "alice", "amount": 42})
        );
    }

    #[test]
    fn payload_encryptor_roundtrip_str() {
        let enc = PayloadEncryptor::new(test_key());
        let plain = r#"{"action":"test"}"#;
        let encrypted = enc.encrypt_str(plain).unwrap();
        assert!(encrypted.starts_with("ENC[AES256-GCM,"));
        let decrypted = enc.decrypt_str(&encrypted).unwrap();
        assert_eq!(decrypted, plain);
    }

    #[test]
    fn payload_encryptor_decrypt_str_passthrough() {
        let enc = PayloadEncryptor::new(test_key());
        let plain = "not-encrypted-at-all";
        let decrypted = enc.decrypt_str(plain).unwrap();
        assert_eq!(decrypted, plain);
    }

    #[test]
    fn payload_encryptor_debug_is_redacted() {
        let enc = PayloadEncryptor::new(test_key());
        let debug = format!("{enc:?}");
        assert_eq!(debug, "PayloadEncryptor([REDACTED])");
    }

    // -----------------------------------------------------------------------
    // Multi-key / key rotation tests
    // -----------------------------------------------------------------------

    #[test]
    fn single_key_new_embeds_k0_kid() {
        let enc = PayloadEncryptor::new(test_key());
        assert_eq!(enc.current_kid(), "k0");
        let encrypted = enc.encrypt_str("test").unwrap();
        assert_eq!(extract_kid(&encrypted), Some("k0".to_owned()));
    }

    #[test]
    fn multi_key_encrypts_with_first_key() {
        let enc = PayloadEncryptor::with_keys(vec![
            PayloadKeyEntry {
                kid: "k2".to_owned(),
                key: test_key_2(),
            },
            PayloadKeyEntry {
                kid: "k1".to_owned(),
                key: test_key(),
            },
        ]);

        assert_eq!(enc.current_kid(), "k2");
        let encrypted = enc.encrypt_str("secret").unwrap();
        assert_eq!(extract_kid(&encrypted), Some("k2".to_owned()));

        // Roundtrip works.
        let decrypted = enc.decrypt_str(&encrypted).unwrap();
        assert_eq!(decrypted, "secret");
    }

    #[test]
    fn multi_key_decrypts_old_key() {
        // Simulate key rotation: old data encrypted with k1, new encryptor has k2 (primary) + k1 (old).
        let old_enc = PayloadEncryptor::with_keys(vec![PayloadKeyEntry {
            kid: "k1".to_owned(),
            key: test_key(),
        }]);
        let encrypted_with_old = old_enc.encrypt_str("old-secret").unwrap();
        assert_eq!(extract_kid(&encrypted_with_old), Some("k1".to_owned()));

        // New encryptor with k2 as primary, k1 as secondary.
        let new_enc = PayloadEncryptor::with_keys(vec![
            PayloadKeyEntry {
                kid: "k2".to_owned(),
                key: test_key_2(),
            },
            PayloadKeyEntry {
                kid: "k1".to_owned(),
                key: test_key(),
            },
        ]);

        // Can decrypt old data.
        let decrypted = new_enc.decrypt_str(&encrypted_with_old).unwrap();
        assert_eq!(decrypted, "old-secret");

        // New encryptions use k2.
        let new_encrypted = new_enc.encrypt_str("new-secret").unwrap();
        assert_eq!(extract_kid(&new_encrypted), Some("k2".to_owned()));
        let new_decrypted = new_enc.decrypt_str(&new_encrypted).unwrap();
        assert_eq!(new_decrypted, "new-secret");
    }

    #[test]
    fn multi_key_decrypts_legacy_no_kid_envelope() {
        // Legacy envelope encrypted with encrypt_value (no kid).
        let key = test_key();
        let legacy = encrypt_value("legacy-data", &key).unwrap();
        assert!(extract_kid(&legacy).is_none());

        // Multi-key encryptor that includes the same key.
        let enc = PayloadEncryptor::with_keys(vec![
            PayloadKeyEntry {
                kid: "k2".to_owned(),
                key: test_key_2(),
            },
            PayloadKeyEntry {
                kid: "k0".to_owned(),
                key: test_key(),
            },
        ]);

        // Should fallback-try all keys and succeed.
        let decrypted = enc.decrypt_str(&legacy).unwrap();
        assert_eq!(decrypted, "legacy-data");
    }

    #[test]
    fn multi_key_fails_when_no_key_matches() {
        let enc = PayloadEncryptor::with_keys(vec![PayloadKeyEntry {
            kid: "k1".to_owned(),
            key: test_key(),
        }]);

        // Encrypt with a different key that the encryptor doesn't have.
        let other_key = test_key_2();
        let encrypted = encrypt_value_with_kid("secret", &other_key, Some("k99")).unwrap();

        let err = enc.decrypt_str(&encrypted).unwrap_err();
        assert!(
            matches!(err, CryptoError::DecryptionFailed),
            "should fail when no key matches"
        );
    }

    #[test]
    fn multi_key_json_roundtrip() {
        let enc = PayloadEncryptor::with_keys(vec![
            PayloadKeyEntry {
                kid: "primary".to_owned(),
                key: test_key(),
            },
            PayloadKeyEntry {
                kid: "secondary".to_owned(),
                key: test_key_2(),
            },
        ]);

        let value = serde_json::json!({"key": "value", "nested": [1, 2, 3]});
        let encrypted = enc.encrypt_json(&value).unwrap();
        assert_eq!(extract_kid(&encrypted), Some("primary".to_owned()));
        let decrypted = enc.decrypt_json(&encrypted).unwrap();
        assert_eq!(decrypted, value);
    }

    #[test]
    #[should_panic(expected = "PayloadEncryptor requires at least one key")]
    fn with_keys_panics_on_empty() {
        PayloadEncryptor::with_keys(vec![]);
    }
}
