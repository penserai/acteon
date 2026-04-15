//! Ed25519 action signing and verification.
//!
//! Provides [`ActionSigningKey`] for signing canonical action bytes and
//! [`Keyring`] for verifying signatures against a set of named public
//! keys. Key material is zeroized on drop and redacted in `Debug`
//! output, following the same hygiene pattern as
//! [`MasterKey`](crate::MasterKey).

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

    /// Derive the corresponding public verifying key with the default
    /// `kid` ([`DEFAULT_KID`]). For an explicit `kid`, build the
    /// verifying key directly via [`ActionVerifyingKey::from_bytes_with_kid`].
    #[must_use]
    pub fn verifying_key(&self) -> ActionVerifyingKey {
        ActionVerifyingKey {
            inner: self.inner.verifying_key(),
            signer_id: self.signer_id.clone(),
            kid: DEFAULT_KID.to_owned(),
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

/// Default key identifier stamped on legacy single-key entries that
/// don't specify their own `kid`. Stable so existing TOML configs
/// keep working after the rotation work.
pub const DEFAULT_KID: &str = "k0";

/// An Ed25519 public verifying key associated with a `signer_id` and
/// an opaque `kid` (key identifier).
///
/// The same `signer_id` may have multiple `ActionVerifyingKey`s
/// registered under different `kid`s during a key rotation window.
/// See [`Keyring`] for the multi-key lookup story.
#[derive(Clone)]
pub struct ActionVerifyingKey {
    inner: ed25519_dalek::VerifyingKey,
    signer_id: String,
    kid: String,
}

impl ActionVerifyingKey {
    /// Create from raw 32-byte public key material with the default
    /// `kid` ([`DEFAULT_KID`]).
    pub fn from_bytes(bytes: [u8; 32], signer_id: impl Into<String>) -> Result<Self, CryptoError> {
        Self::from_bytes_with_kid(bytes, signer_id, DEFAULT_KID)
    }

    /// Create from raw 32-byte public key material with an explicit `kid`.
    pub fn from_bytes_with_kid(
        bytes: [u8; 32],
        signer_id: impl Into<String>,
        kid: impl Into<String>,
    ) -> Result<Self, CryptoError> {
        let inner = ed25519_dalek::VerifyingKey::from_bytes(&bytes)
            .map_err(|e| CryptoError::InvalidKey(e.to_string()))?;
        Ok(Self {
            inner,
            signer_id: signer_id.into(),
            kid: kid.into(),
        })
    }

    /// The `signer_id` this key is associated with.
    #[must_use]
    pub fn signer_id(&self) -> &str {
        &self.signer_id
    }

    /// The `kid` (key identifier) this key carries. Defaults to
    /// [`DEFAULT_KID`] when not set explicitly.
    #[must_use]
    pub fn kid(&self) -> &str {
        &self.kid
    }

    /// Override the `kid` on an existing key. Used when migrating a
    /// legacy entry into a rotation-aware keyring.
    #[must_use]
    pub fn with_kid(mut self, kid: impl Into<String>) -> Self {
        self.kid = kid.into();
        self
    }

    /// Raw 32-byte public key material — used when serializing the
    /// active verifier set into the JWKS-style discovery endpoint.
    #[must_use]
    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.inner.to_bytes()
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
            .field("kid", &self.kid)
            .finish_non_exhaustive()
    }
}

/// A set of named verifying keys for multi-signer lookups, with
/// support for **multiple keys per signer** so an operator can stage
/// a key rotation window without coordinated downtime.
///
/// During a rotation:
/// 1. Operator generates a new key and `insert`s it under the same
///    `signer_id` with a new `kid`. Both old and new are now active.
/// 2. The signer (a service, CI bot, etc.) starts producing
///    signatures stamped with the new `kid`.
/// 3. After the in-flight window elapses, the operator removes the
///    old key via [`Keyring::remove`].
///
/// Verification:
/// - When the action carries a `kid`, [`Keyring::verify_with_kid`]
///   selects the exact key to check against. Any `kid` mismatch
///   surfaces as [`CryptoError::UnknownSigner`].
/// - When the action does not carry a `kid` (legacy clients),
///   [`Keyring::verify`] tries every key registered under the
///   signer and accepts the first one that validates the signature.
#[derive(Clone, Debug, Default)]
pub struct Keyring {
    /// `(signer_id, kid)` → key. Allows multiple keys per signer.
    by_kid: HashMap<(String, String), ActionVerifyingKey>,
}

impl Keyring {
    /// Create an empty keyring.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a verifying key. Replaces any existing key with the
    /// same `(signer_id, kid)` pair, but leaves keys for other
    /// `kid`s under the same signer untouched — the multi-key story
    /// for rotation.
    pub fn insert(&mut self, key: ActionVerifyingKey) {
        self.by_kid
            .insert((key.signer_id.clone(), key.kid.clone()), key);
    }

    /// Remove a specific `(signer_id, kid)` entry. Returns true when
    /// the key existed. Use this to retire an old key after a
    /// rotation window has elapsed.
    pub fn remove(&mut self, signer_id: &str, kid: &str) -> bool {
        self.by_kid
            .remove(&(signer_id.to_owned(), kid.to_owned()))
            .is_some()
    }

    /// Total number of keys in the keyring (sum across all signers).
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_kid.len()
    }

    /// Whether the keyring is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_kid.is_empty()
    }

    /// Check whether *any* key for `signer_id` is present in the
    /// keyring (regardless of `kid`).
    #[must_use]
    pub fn contains(&self, signer_id: &str) -> bool {
        self.by_kid.keys().any(|(s, _)| s == signer_id)
    }

    /// Check whether a specific `(signer_id, kid)` pair is registered.
    #[must_use]
    pub fn contains_kid(&self, signer_id: &str, kid: &str) -> bool {
        self.by_kid
            .contains_key(&(signer_id.to_owned(), kid.to_owned()))
    }

    /// Iterate every `(signer_id, kid, ActionVerifyingKey)` triple
    /// in the keyring. Used by the JWKS-style discovery endpoint.
    pub fn iter_keys(&self) -> impl Iterator<Item = &ActionVerifyingKey> {
        self.by_kid.values()
    }

    /// Verify a signature against any key registered under
    /// `signer_id`. Used for legacy clients that don't stamp a
    /// `kid` on signed actions — the verifier tries every active
    /// key for that signer and accepts the first match.
    ///
    /// During a rotation window with both old and new keys active,
    /// this method works for **all** in-flight signatures regardless
    /// of which key produced them. Once you migrate clients to send
    /// `kid`, [`Keyring::verify_with_kid`] is preferred because it
    /// fails fast on key mismatch.
    pub fn verify(
        &self,
        signer_id: &str,
        signature_b64: &str,
        message: &[u8],
    ) -> Result<(), CryptoError> {
        let mut any_key_for_signer = false;
        for ((s, _), key) in &self.by_kid {
            if s == signer_id {
                any_key_for_signer = true;
                if key.verify(signature_b64, message).is_ok() {
                    return Ok(());
                }
            }
        }
        if any_key_for_signer {
            Err(CryptoError::SignatureInvalid)
        } else {
            Err(CryptoError::UnknownSigner(signer_id.to_owned()))
        }
    }

    /// Verify a signature against the specific `(signer_id, kid)`
    /// pair. Use this when the action carries a `kid` so that a
    /// stale or never-issued key is rejected rather than silently
    /// trying every active key for the signer.
    pub fn verify_with_kid(
        &self,
        signer_id: &str,
        kid: &str,
        signature_b64: &str,
        message: &[u8],
    ) -> Result<(), CryptoError> {
        let key = self
            .by_kid
            .get(&(signer_id.to_owned(), kid.to_owned()))
            .ok_or_else(|| CryptoError::UnknownSigner(format!("{signer_id}/{kid}")))?;
        key.verify(signature_b64, message)
    }
}

/// Generate a fresh Ed25519 keypair with the default `kid`
/// ([`DEFAULT_KID`]).
#[must_use]
pub fn generate_keypair(signer_id: impl Into<String>) -> (ActionSigningKey, ActionVerifyingKey) {
    generate_keypair_with_kid(signer_id, DEFAULT_KID)
}

/// Generate a fresh Ed25519 keypair with an explicit `kid` — used
/// by rotation tooling to introduce a new key alongside an existing
/// one for the same signer.
#[must_use]
pub fn generate_keypair_with_kid(
    signer_id: impl Into<String>,
    kid: impl Into<String>,
) -> (ActionSigningKey, ActionVerifyingKey) {
    let id = signer_id.into();
    let kid = kid.into();
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
            kid,
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

/// Parse a 32-byte verifying (public) key from hex or base64 with the
/// default [`DEFAULT_KID`].
pub fn parse_verifying_key(
    raw: &str,
    signer_id: impl Into<String>,
) -> Result<ActionVerifyingKey, CryptoError> {
    parse_verifying_key_with_kid(raw, signer_id, DEFAULT_KID)
}

/// Parse a 32-byte verifying (public) key from hex or base64 with an
/// explicit `kid`. Used when loading rotation-aware keyring entries
/// from configuration.
pub fn parse_verifying_key_with_kid(
    raw: &str,
    signer_id: impl Into<String>,
    kid: impl Into<String>,
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
    ActionVerifyingKey::from_bytes_with_kid(arr, signer_id, kid)
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

    // =====================================================================
    // Multi-key keyring (rotation) tests
    // =====================================================================

    #[test]
    fn keyring_default_kid_is_k0() {
        let (_, vk) = generate_keypair("legacy-signer");
        assert_eq!(vk.kid(), DEFAULT_KID);
        assert_eq!(vk.kid(), "k0");
    }

    #[test]
    fn keyring_supports_multiple_kids_per_signer() {
        // Stage a rotation: same signer_id, two different kids.
        let (sk_old, vk_old) = generate_keypair_with_kid("ci-bot", "k1");
        let (sk_new, vk_new) = generate_keypair_with_kid("ci-bot", "k2");
        let mut keyring = Keyring::new();
        keyring.insert(vk_old);
        keyring.insert(vk_new);

        assert_eq!(keyring.len(), 2);
        assert!(keyring.contains("ci-bot"));
        assert!(keyring.contains_kid("ci-bot", "k1"));
        assert!(keyring.contains_kid("ci-bot", "k2"));

        // verify_with_kid must select the exact key.
        let msg = b"canonical action bytes";
        let sig_old = sk_old.sign(msg);
        let sig_new = sk_new.sign(msg);

        keyring
            .verify_with_kid("ci-bot", "k1", &sig_old, msg)
            .expect("k1 signature verifies under k1");
        keyring
            .verify_with_kid("ci-bot", "k2", &sig_new, msg)
            .expect("k2 signature verifies under k2");

        // Cross-check: k1 signature against k2 must fail.
        assert!(
            keyring
                .verify_with_kid("ci-bot", "k2", &sig_old, msg)
                .is_err()
        );
    }

    #[test]
    fn keyring_legacy_verify_tries_all_kids() {
        // A legacy client doesn't send `kid` — the verifier should
        // try every key for the signer and accept the first match.
        let (sk_old, vk_old) = generate_keypair_with_kid("ci-bot", "k1");
        let (_sk_new, vk_new) = generate_keypair_with_kid("ci-bot", "k2");
        let mut keyring = Keyring::new();
        keyring.insert(vk_old);
        keyring.insert(vk_new);

        let msg = b"canonical action bytes";
        let sig = sk_old.sign(msg);

        // Legacy verify (no kid) finds k1 even with k2 also registered.
        keyring
            .verify("ci-bot", &sig, msg)
            .expect("legacy verify finds the matching kid");
    }

    #[test]
    fn keyring_remove_retires_a_specific_kid() {
        let (_, vk_old) = generate_keypair_with_kid("ci-bot", "k1");
        let (sk_new, vk_new) = generate_keypair_with_kid("ci-bot", "k2");
        let mut keyring = Keyring::new();
        keyring.insert(vk_old);
        keyring.insert(vk_new);

        assert!(keyring.remove("ci-bot", "k1"));
        assert!(!keyring.contains_kid("ci-bot", "k1"));
        assert!(keyring.contains_kid("ci-bot", "k2"));
        assert!(keyring.contains("ci-bot"));

        // Retired kid no longer verifies.
        let msg = b"after rotation";
        let sig_new = sk_new.sign(msg);
        keyring
            .verify_with_kid("ci-bot", "k2", &sig_new, msg)
            .expect("active key still verifies");
        assert!(
            keyring
                .verify_with_kid("ci-bot", "k1", &sig_new, msg)
                .is_err()
        );
    }

    #[test]
    fn keyring_unknown_kid_is_distinct_from_unknown_signer() {
        let (_, vk) = generate_keypair_with_kid("ci-bot", "k1");
        let mut keyring = Keyring::new();
        keyring.insert(vk);

        // Wrong kid for known signer → UnknownSigner with composite name.
        let result = keyring.verify_with_kid("ci-bot", "k2", "AAAA", b"msg");
        assert!(matches!(result, Err(CryptoError::UnknownSigner(_))));

        // Unknown signer entirely → UnknownSigner with the signer name.
        let result = keyring.verify_with_kid("nobody", "k1", "AAAA", b"msg");
        assert!(matches!(result, Err(CryptoError::UnknownSigner(_))));
    }

    #[test]
    fn keyring_iter_keys_returns_every_entry() {
        let (_, vk1) = generate_keypair_with_kid("ci-bot", "k1");
        let (_, vk2) = generate_keypair_with_kid("ci-bot", "k2");
        let (_, vk3) = generate_keypair_with_kid("deploy-bot", "k1");
        let mut keyring = Keyring::new();
        keyring.insert(vk1);
        keyring.insert(vk2);
        keyring.insert(vk3);

        let collected: Vec<(&str, &str)> = keyring
            .iter_keys()
            .map(|k| (k.signer_id(), k.kid()))
            .collect();
        assert_eq!(collected.len(), 3);
        assert!(collected.iter().any(|(s, k)| *s == "ci-bot" && *k == "k1"));
        assert!(collected.iter().any(|(s, k)| *s == "ci-bot" && *k == "k2"));
        assert!(
            collected
                .iter()
                .any(|(s, k)| *s == "deploy-bot" && *k == "k1")
        );
    }

    #[test]
    fn keyring_legacy_signature_with_no_matching_key_is_invalid() {
        // Signer is known but no key validates: SignatureInvalid (not UnknownSigner).
        let (_, vk) = generate_keypair_with_kid("ci-bot", "k1");
        let mut keyring = Keyring::new();
        keyring.insert(vk);
        let result = keyring.verify(
            "ci-bot",
            "MTIzNDU2Nzg5MGFiY2RlZjEyMzQ1Njc4OTBhYmNkZWYxMjM0NTY3ODkwYWJjZGVmMTIzNDU2Nzg5MGFiY2RlZjEyMzQ=",
            b"msg",
        );
        assert!(matches!(result, Err(CryptoError::SignatureInvalid)));
    }
}
