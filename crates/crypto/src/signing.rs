//! Ed25519 action signing and verification.
//!
//! Provides [`ActionSigner`] for signing canonical action bytes and
//! [`Keyring`] for verifying signatures against a set of named public
//! keys. Key material is zeroized on drop and redacted in `Debug`
//! output, following the same hygiene pattern as [`super::MasterKey`].

use std::collections::HashMap;
use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signature, Signer, Verifier};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::CryptoError;

/// An Ed25519 signing key that is zeroized when dropped.
///
/// Wraps [`ed25519_dalek::SigningKey`]. The `Debug` implementation is
/// redacted to prevent accidental logging of secret key material.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct ActionSigningKey {
    #[zeroize(skip)] // ed25519_dalek::SigningKey implements Zeroize internally
    inner: ed25519_dalek::SigningKey,
    signer_id: String,
}

impl ActionSigningKey {
    /// Create a signing key from raw 32-byte seed material.
    pub fn from_bytes(bytes: [u8; 32], signer_id: impl Into<String>) -> Self {
        Self {
            inner: ed25519_dalek::SigningKey::from_bytes(&bytes),
            signer_id: signer_id.into(),
        }
    }

    /// The `signer_id` that will be stamped on signed actions.
    #[must_use]
    pub fn signer_id(&self) -> &str {
        &self.signer_id
    }

    /// Derive the corresponding public verifying key.
    #[must_use]
    pub fn verifying_key(&self) -> ActionVerifyingKey {
        ActionVerifyingKey {
            inner: self.inner.verifying_key(),
            signer_id: self.signer_id.clone(),
        }
    }

    /// Sign arbitrary bytes and return the signature as base64.
    #[must_use]
    pub fn sign(&self, message: &[u8]) -> String {
        let sig = self.inner.sign(message);
        B64.encode(sig.to_bytes())
    }
}

impl fmt::Debug for ActionSigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ActionSigningKey")
            .field("signer_id", &self.signer_id)
            .field("key", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

/// An Ed25519 public verifying key associated with a `signer_id`.
#[derive(Clone)]
pub struct ActionVerifyingKey {
    inner: ed25519_dalek::VerifyingKey,
    signer_id: String,
}

impl ActionVerifyingKey {
    /// Create from raw 32-byte public key material.
    pub fn from_bytes(bytes: [u8; 32], signer_id: impl Into<String>) -> Result<Self, CryptoError> {
        let inner = ed25519_dalek::VerifyingKey::from_bytes(&bytes)
            .map_err(|e| CryptoError::InvalidKey(e.to_string()))?;
        Ok(Self {
            inner,
            signer_id: signer_id.into(),
        })
    }

    /// The `signer_id` this key is associated with.
    #[must_use]
    pub fn signer_id(&self) -> &str {
        &self.signer_id
    }

    /// Verify a base64-encoded signature against a message.
    pub fn verify(&self, signature_b64: &str, message: &[u8]) -> Result<(), CryptoError> {
        let sig_bytes = B64
            .decode(signature_b64)
            .map_err(|_| CryptoError::SignatureInvalid)?;
        let sig = Signature::from_slice(&sig_bytes).map_err(|_| CryptoError::SignatureInvalid)?;
        self.inner
            .verify(message, &sig)
            .map_err(|_| CryptoError::SignatureInvalid)
    }
}

impl fmt::Debug for ActionVerifyingKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ActionVerifyingKey")
            .field("signer_id", &self.signer_id)
            .finish_non_exhaustive()
    }
}

/// A set of named verifying keys for multi-signer lookups.
#[derive(Clone, Debug, Default)]
pub struct Keyring {
    keys: HashMap<String, ActionVerifyingKey>,
}

impl Keyring {
    /// Create an empty keyring.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a verifying key. Replaces any existing key with the
    /// same `signer_id`.
    pub fn insert(&mut self, key: ActionVerifyingKey) {
        self.keys.insert(key.signer_id.clone(), key);
    }

    /// Number of keys in the keyring.
    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether the keyring is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Verify a signature against the key matching `signer_id`.
    pub fn verify(
        &self,
        signer_id: &str,
        signature_b64: &str,
        message: &[u8],
    ) -> Result<(), CryptoError> {
        let key = self
            .keys
            .get(signer_id)
            .ok_or_else(|| CryptoError::UnknownSigner(signer_id.to_owned()))?;
        key.verify(signature_b64, message)
    }
}

/// Generate a fresh Ed25519 keypair.
#[must_use]
pub fn generate_keypair(signer_id: impl Into<String>) -> (ActionSigningKey, ActionVerifyingKey) {
    let id = signer_id.into();
    let sk = ed25519_dalek::SigningKey::generate(&mut rand_core::OsRng);
    let vk = sk.verifying_key();
    (
        ActionSigningKey {
            inner: sk,
            signer_id: id.clone(),
        },
        ActionVerifyingKey {
            inner: vk,
            signer_id: id,
        },
    )
}

/// Parse a 32-byte signing key from hex or base64.
///
/// Accepts either 64 hex characters or a base64 string that decodes
/// to exactly 32 bytes, mirroring [`super::parse_master_key`].
pub fn parse_signing_key(
    raw: &str,
    signer_id: impl Into<String>,
) -> Result<ActionSigningKey, CryptoError> {
    let bytes = if raw.len() == 64 && raw.chars().all(|c| c.is_ascii_hexdigit()) {
        hex::decode(raw).map_err(|e| CryptoError::InvalidKey(e.to_string()))?
    } else {
        B64.decode(raw)
            .map_err(|e| CryptoError::InvalidKey(e.to_string()))?
    };
    if bytes.len() != 32 {
        return Err(CryptoError::InvalidKey(format!(
            "expected 32 bytes, got {}",
            bytes.len()
        )));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(ActionSigningKey::from_bytes(arr, signer_id))
}

/// Parse a 32-byte verifying (public) key from hex or base64.
pub fn parse_verifying_key(
    raw: &str,
    signer_id: impl Into<String>,
) -> Result<ActionVerifyingKey, CryptoError> {
    let bytes = if raw.len() == 64 && raw.chars().all(|c| c.is_ascii_hexdigit()) {
        hex::decode(raw).map_err(|e| CryptoError::InvalidKey(e.to_string()))?
    } else {
        B64.decode(raw)
            .map_err(|e| CryptoError::InvalidKey(e.to_string()))?
    };
    if bytes.len() != 32 {
        return Err(CryptoError::InvalidKey(format!(
            "expected 32 bytes, got {}",
            bytes.len()
        )));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    ActionVerifyingKey::from_bytes(arr, signer_id)
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify_roundtrip() {
        let (sk, vk) = generate_keypair("test-signer");
        let message = b"hello world";
        let sig = sk.sign(message);
        vk.verify(&sig, message).expect("valid signature");
    }

    #[test]
    fn sign_verify_via_keyring() {
        let (sk, vk) = generate_keypair("ci-bot");
        let mut keyring = Keyring::new();
        keyring.insert(vk);

        let message = b"canonical action bytes";
        let sig = sk.sign(message);
        keyring
            .verify("ci-bot", &sig, message)
            .expect("keyring verification should pass");
    }

    #[test]
    fn bad_signature_rejected() {
        let (_, vk) = generate_keypair("test");
        let result = vk.verify("dGhpcyBpcyBub3QgYSByZWFsIHNpZ25hdHVyZQ==", b"msg");
        assert!(result.is_err());
    }

    #[test]
    fn unknown_signer_rejected() {
        let keyring = Keyring::new();
        let result = keyring.verify("nobody", "AAAA", b"msg");
        assert!(matches!(result, Err(CryptoError::UnknownSigner(_))));
    }

    #[test]
    fn tampered_message_rejected() {
        let (sk, vk) = generate_keypair("test");
        let sig = sk.sign(b"original");
        let result = vk.verify(&sig, b"tampered");
        assert!(matches!(result, Err(CryptoError::SignatureInvalid)));
    }

    #[test]
    fn parse_signing_key_hex() {
        let (sk, _) = generate_keypair("test");
        let hex_str = hex::encode(sk.inner.to_bytes());
        let parsed = parse_signing_key(&hex_str, "test").expect("parse hex key");
        assert_eq!(parsed.signer_id(), "test");

        // Sign with original, verify with parsed-derived pubkey.
        let msg = b"roundtrip";
        let sig = sk.sign(msg);
        parsed.verifying_key().verify(&sig, msg).expect("verify");
    }

    #[test]
    fn parse_signing_key_base64() {
        let (sk, _) = generate_keypair("b64-test");
        let b64_str = B64.encode(sk.inner.to_bytes());
        let parsed = parse_signing_key(&b64_str, "b64-test").expect("parse b64 key");
        assert_eq!(parsed.signer_id(), "b64-test");
    }

    #[test]
    fn parse_verifying_key_hex() {
        let (sk, vk) = generate_keypair("pub-test");
        let hex_str = hex::encode(vk.inner.to_bytes());
        let parsed = parse_verifying_key(&hex_str, "pub-test").expect("parse hex pubkey");

        let msg = b"verify me";
        let sig = sk.sign(msg);
        parsed.verify(&sig, msg).expect("verify");
    }

    #[test]
    fn wrong_length_key_rejected() {
        let result = parse_signing_key("deadbeef", "short");
        assert!(matches!(result, Err(CryptoError::InvalidKey(_))));
    }

    #[test]
    fn debug_redacts_secret_key() {
        let (sk, _) = generate_keypair("debug-test");
        let debug = format!("{sk:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains(&hex::encode(sk.inner.to_bytes())));
    }
}
