//! Action signature verification.
//!
//! Provides [`SignatureVerifier`] for verifying Ed25519 signatures on
//! incoming actions and a `GET /v1/actions/{id}/verify` endpoint for
//! post-hoc cryptographic proof of action origin.

use std::sync::Arc;

use acteon_core::Action;
use acteon_crypto::signing::Keyring;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use base64::Engine;
use serde::Serialize;
use tracing::warn;
use utoipa::ToSchema;

use super::AppState;
use super::schemas::ErrorResponse;

/// Per-signer scope restrictions.
#[derive(Clone, Debug)]
pub struct SignerScope {
    /// Allowed tenants. `["*"]` means all.
    pub tenants: Vec<String>,
    /// Allowed namespaces. `["*"]` means all.
    pub namespaces: Vec<String>,
}

impl SignerScope {
    fn allows(&self, tenant: &str, namespace: &str) -> bool {
        let tenant_ok = self.tenants.iter().any(|t| t == "*" || t == tenant);
        let ns_ok = self.namespaces.iter().any(|n| n == "*" || n == namespace);
        tenant_ok && ns_ok
    }
}

/// Verifies Ed25519 signatures on dispatched actions.
///
/// Performs three checks on signed actions:
/// 1. **Signature validity** — the Ed25519 signature verifies against
///    the keyring's public key for the given `signer_id`.
/// 2. **Scope enforcement** — the signer is authorized for the
///    action's `(tenant, namespace)` pair. Prevents a compromised
///    key for one tenant from signing actions for another.
/// 3. **Replay rejection** (when `reject_replay` is enabled) — the
///    action ID must not have been seen before (checked externally
///    by the dispatch handler against the state store).
#[derive(Clone)]
pub struct SignatureVerifier {
    keyring: Arc<Keyring>,
    reject_unsigned: bool,
    /// Per-`(signer_id, kid)` scope restrictions. Keyed by key, not by signer,
    /// so a rotation that introduces a second `kid` for the same `signer_id`
    /// keeps each key's configured scope instead of the last-loaded one
    /// overwriting the rest. Missing entries default to allow-all (the server
    /// key is inserted without scope limits).
    scopes: std::collections::HashMap<(String, String), SignerScope>,
}

impl SignatureVerifier {
    /// Create a new verifier.
    #[must_use]
    pub fn new(keyring: Keyring, reject_unsigned: bool) -> Self {
        Self {
            keyring: Arc::new(keyring),
            reject_unsigned,
            scopes: std::collections::HashMap::new(),
        }
    }

    /// Register a scope restriction for a specific `(signer_id, kid)` key.
    pub fn add_scope(
        &mut self,
        signer_id: impl Into<String>,
        kid: impl Into<String>,
        scope: SignerScope,
    ) {
        self.scopes.insert((signer_id.into(), kid.into()), scope);
    }

    /// Number of keys in the keyring.
    #[must_use]
    pub fn keyring_len(&self) -> usize {
        self.keyring.len()
    }

    /// Check whether a `signer_id` is known to the keyring.
    #[must_use]
    pub fn keyring_contains(&self, signer_id: &str) -> bool {
        self.keyring.contains(signer_id)
    }

    /// Iterate every key in the keyring. Used by the JWKS-style
    /// discovery endpoint to publish the active verifier set.
    pub fn iter_keys(&self) -> impl Iterator<Item = &acteon_crypto::signing::ActionVerifyingKey> {
        self.keyring.iter_keys()
    }

    /// Look up the scope restrictions registered for a specific
    /// `(signer_id, kid)` key, if any. Used by the JWKS-style discovery
    /// endpoint so clients can see which `(tenant, namespace)` pairs each
    /// key is authorized for.
    pub fn scope_for(&self, signer_id: &str, kid: &str) -> Option<&SignerScope> {
        self.scopes.get(&(signer_id.to_owned(), kid.to_owned()))
    }

    /// Verify an action's signature inline during dispatch.
    ///
    /// Returns a [`VerifyOutcome`] describing which branch was
    /// taken. Callers bump the corresponding gateway metric via
    /// [`VerifyOutcome::record_metric`] and translate the outcome
    /// into an HTTP 400 response via [`VerifyOutcome::error_message`]
    /// when appropriate.
    ///
    /// When the action carries a `kid`, the verifier looks up the
    /// exact `(signer_id, kid)` pair — fail-fast on stale or
    /// never-issued keys. When no `kid` is present, the verifier
    /// tries every active key registered under `signer_id` and
    /// accepts the first match (legacy single-key behavior, plus
    /// the rotation overlap window where neither client nor server
    /// has fully migrated to `kid`).
    pub fn verify_action(&self, action: &Action) -> VerifyOutcome {
        let (Some(sig), Some(signer_id)) = (&action.signature, &action.signer_id) else {
            return if self.reject_unsigned {
                VerifyOutcome::UnsignedRejected
            } else {
                VerifyOutcome::UnsignedAllowed
            };
        };

        // 1. Cryptographic verification.
        let canonical = action.canonical_bytes();
        let crypto_result = if let Some(ref kid) = action.kid {
            self.keyring
                .verify_with_kid(signer_id, kid, sig, &canonical)
        } else {
            self.keyring.verify(signer_id, sig, &canonical)
        };

        if let Err(e) = crypto_result {
            return match e {
                acteon_crypto::CryptoError::UnknownSigner(_) => VerifyOutcome::UnknownSigner {
                    signer_id: signer_id.clone(),
                    kid: action.kid.clone(),
                },
                acteon_crypto::CryptoError::SignatureInvalid => VerifyOutcome::InvalidSignature {
                    signer_id: signer_id.clone(),
                    kid: action.kid.clone(),
                },
                // Any other CryptoError variant (InvalidKey, InvalidFormat,
                // DecryptionFailed, EncryptionFailed) is an internal fault
                // — those error types are for the encryption path, not
                // signature verification, and shouldn't reach this arm in
                // normal operation. Surface as a 500 so operators notice
                // instead of mislabeling them as "invalid signature".
                other => VerifyOutcome::InternalError {
                    message: other.to_string(),
                },
            };
        }

        // 2. Scope enforcement, per key (signer_id, kid).
        let scope_denied = if let Some(ref kid) = action.kid {
            // The signature was verified against exactly this key, so enforce
            // exactly this key's scope (allow-all when none is configured).
            self.scopes
                .get(&(signer_id.clone(), kid.clone()))
                .is_some_and(|scope| !scope.allows(&action.tenant, &action.namespace))
        } else {
            // Legacy kid-less action: `keyring.verify` accepted it against one
            // of the signer's keys but does not report which, so fail closed —
            // the action must satisfy EVERY scope configured for this signer.
            // (When all of a signer's keys share one scope, this is identical
            // to the per-key check; it only tightens the ambiguous case.)
            self.scopes
                .iter()
                .filter(|((s, _), _)| s == signer_id)
                .any(|(_, scope)| !scope.allows(&action.tenant, &action.namespace))
        };
        if scope_denied {
            return VerifyOutcome::ScopeDenied {
                signer_id: signer_id.clone(),
                tenant: action.tenant.to_string(),
                namespace: action.namespace.to_string(),
            };
        }

        VerifyOutcome::Verified {
            signer_id: signer_id.clone(),
            kid: action.kid.clone(),
        }
    }
}

/// Outcome of verifying an action's signature.
///
/// Lets the dispatch handler distinguish between the branches the
/// verifier took so it can bump the right metric and — for rejection
/// paths — emit a targeted HTTP 400 error message.
#[derive(Debug, Clone)]
pub enum VerifyOutcome {
    /// Action carries no signature and `reject_unsigned` is off —
    /// dispatch proceeds without signing validation.
    UnsignedAllowed,
    /// Action carries a signature that is cryptographically valid
    /// and passes scope enforcement.
    Verified {
        signer_id: String,
        kid: Option<String>,
    },
    /// Action carries no signature and `reject_unsigned` is on.
    UnsignedRejected,
    /// The `signer_id` (or `(signer_id, kid)` pair during a rotation
    /// window) is not registered in the keyring.
    UnknownSigner {
        signer_id: String,
        kid: Option<String>,
    },
    /// Cryptographic verification failed — the signature does not
    /// match the canonical bytes under the registered public key.
    InvalidSignature {
        signer_id: String,
        kid: Option<String>,
    },
    /// Signature is cryptographically valid but the signer is not
    /// authorized for the action's `(tenant, namespace)` pair.
    ScopeDenied {
        signer_id: String,
        tenant: String,
        namespace: String,
    },
    /// An unexpected crypto error was returned during verification.
    /// Indicates a bug or misconfiguration rather than a rejected
    /// signature — the dispatch handler maps this to HTTP 500.
    InternalError { message: String },
}

impl VerifyOutcome {
    /// Whether the outcome allows dispatch to proceed.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::UnsignedAllowed | Self::Verified { .. })
    }

    /// HTTP 400 body message for rejection outcomes, or `None` when
    /// dispatch should proceed. `InternalError` is handled separately
    /// via [`Self::internal_error_message`] since it maps to a 500.
    ///
    /// `UnknownSigner` and `InvalidSignature` return the same wire
    /// message on purpose: distinguishing them would let an attacker
    /// enumerate which `(signer_id, kid)` pairs exist on the server.
    /// The JWKS discovery endpoint already exposes the public
    /// verifier set to anyone who asks, but we don't need to give
    /// away additional signal per dispatch attempt. Operators can
    /// still tell the two apart via the [`record_metric`] counters
    /// and the structured tracing log emitted by [`log_rejection`].
    ///
    /// [`record_metric`]: Self::record_metric
    /// [`log_rejection`]: Self::log_rejection
    #[must_use]
    pub fn error_message(&self) -> Option<String> {
        match self {
            Self::UnsignedAllowed | Self::Verified { .. } | Self::InternalError { .. } => None,
            Self::UnsignedRejected => Some(
                "unsigned action rejected: signing.reject_unsigned is enabled; \
                 provide both 'signature' and 'signer_id' fields"
                    .to_owned(),
            ),
            Self::UnknownSigner { signer_id, .. } | Self::InvalidSignature { signer_id, .. } => {
                Some(format!(
                    "signature verification failed for signer '{signer_id}'"
                ))
            }
            Self::ScopeDenied {
                signer_id,
                tenant,
                namespace,
            } => Some(format!(
                "signer '{signer_id}' is not authorized for tenant={tenant} \
                 namespace={namespace}"
            )),
        }
    }

    /// Emit a structured `tracing::warn` line describing a rejection
    /// outcome. Operators debugging a failed dispatch get the full
    /// variant, `kid`, and scope context in logs — which the
    /// generic HTTP 400 body deliberately omits. No-ops on the
    /// success and `UnsignedAllowed` branches.
    pub fn log_rejection(&self) {
        match self {
            Self::UnsignedAllowed | Self::Verified { .. } => {}
            Self::UnsignedRejected => {
                warn!(
                    outcome = "unsigned_rejected",
                    "signature verification rejected: unsigned action with reject_unsigned enabled"
                );
            }
            Self::UnknownSigner { signer_id, kid } => {
                warn!(
                    outcome = "unknown_signer",
                    signer_id = signer_id.as_str(),
                    kid = kid.as_deref(),
                    "signature verification rejected: signer (or signer/kid pair) not in keyring"
                );
            }
            Self::InvalidSignature { signer_id, kid } => {
                warn!(
                    outcome = "invalid_signature",
                    signer_id = signer_id.as_str(),
                    kid = kid.as_deref(),
                    "signature verification rejected: Ed25519 signature did not validate"
                );
            }
            Self::ScopeDenied {
                signer_id,
                tenant,
                namespace,
            } => {
                warn!(
                    outcome = "scope_denied",
                    signer_id = signer_id.as_str(),
                    tenant = tenant.as_str(),
                    namespace = namespace.as_str(),
                    "signature verification rejected: signer not authorized for tenant/namespace"
                );
            }
            Self::InternalError { message } => {
                warn!(
                    outcome = "internal_error",
                    error = message.as_str(),
                    "signature verification hit an unexpected crypto error"
                );
            }
        }
    }

    /// HTTP 500 body message when an unexpected crypto error surfaced
    /// during verification (none otherwise).
    #[must_use]
    pub fn internal_error_message(&self) -> Option<String> {
        match self {
            Self::InternalError { message } => Some(format!(
                "signature verification failed with an unexpected crypto error: {message}"
            )),
            _ => None,
        }
    }

    /// Bump the gateway counter that corresponds to this outcome.
    pub fn record_metric(&self, metrics: &acteon_gateway::GatewayMetrics) {
        match self {
            Self::UnsignedAllowed => metrics.increment_signing_unsigned_allowed(),
            Self::Verified { .. } => metrics.increment_signing_verified(),
            Self::UnsignedRejected => metrics.increment_signing_unsigned_rejected(),
            Self::UnknownSigner { .. } => metrics.increment_signing_unknown_signer(),
            Self::InvalidSignature { .. } => metrics.increment_signing_invalid(),
            Self::ScopeDenied { .. } => metrics.increment_signing_scope_denied(),
            // InternalError is a bug, not a signing event — don't
            // count it against the verification metrics.
            Self::InternalError { .. } => {}
        }
    }
}

/// Response from the `GET /v1/actions/{id}/verify` endpoint.
///
/// IMPORTANT: this endpoint does NOT perform a full cryptographic
/// verification. The audit record stores only the SHA-256 `canonical_hash` of
/// the bytes that were signed, not the bytes themselves, so the original
/// Ed25519 signature cannot be re-checked server-side. It reports whether the
/// signer is known and whether the stored signature is well-formed, and
/// returns the `canonical_hash` so a caller holding the original action can
/// verify independently. There is deliberately no single `verified: true`
/// field, which previously over-claimed cryptographic verification.
#[derive(Debug, Serialize, ToSchema)]
pub struct VerifyResponse {
    /// Whether the action's `signer_id` is present in the server keyring.
    pub signer_known: bool,
    /// Whether the stored signature is well-formed — it base64-decodes to a
    /// 64-byte Ed25519 signature. This is a structural check only, NOT a
    /// cryptographic verification (see the type-level note).
    pub signature_well_formed: bool,
    /// The `signer_id` from the action (if signed).
    pub signer_id: Option<String>,
    /// The algorithm used.
    pub algorithm: Option<String>,
    /// SHA-256 hex digest of the canonical bytes that were signed. To fully
    /// verify, recompute `Action::canonical_bytes()` on the original action,
    /// confirm its SHA-256 equals this value, then verify the signature
    /// against those bytes with the signer's public key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_hash: Option<String>,
    /// Human-readable detail: the verification caveat on success, or why the
    /// signer is unknown / signature malformed / record unsigned otherwise.
    pub reason: Option<String>,
}

/// `GET /v1/actions/{id}/verify` — verify an action's signature.
#[utoipa::path(
    get,
    path = "/v1/actions/{id}/verify",
    tag = "Signing",
    summary = "Verify action signature",
    description = "Looks up an audit record by action ID and reports whether its signer is known and the stored Ed25519 signature is well-formed. NOTE: this is not a full cryptographic verification — the audit record stores only the canonical-bytes hash, so re-verification requires the original action (the canonical_hash is returned for that purpose).",
    params(("id" = String, Path, description = "Action ID")),
    responses(
        (status = 200, description = "Verification result", body = VerifyResponse),
        (status = 404, description = "Action not found", body = ErrorResponse),
    )
)]
#[allow(clippy::too_many_lines)]
pub async fn verify_action(
    State(state): State<AppState>,
    Path(action_id): Path<String>,
) -> impl IntoResponse {
    // Look up the audit record by action ID.
    let Some(ref audit) = state.audit else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                error: "audit trail is disabled".into(),
            })),
        )
            .into_response();
    };

    let record = match audit.get_by_action_id(&action_id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!(ErrorResponse {
                    error: format!("action not found: {action_id}"),
                })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse {
                    error: e.to_string(),
                })),
            )
                .into_response();
        }
    };

    // If the action was not signed, report that.
    let (Some(signature), Some(signer_id)) = (&record.signature, &record.signer_id) else {
        return (
            StatusCode::OK,
            Json(serde_json::json!(VerifyResponse {
                signer_known: false,
                signature_well_formed: false,
                signer_id: record.signer_id.clone(),
                algorithm: None,
                canonical_hash: None,
                reason: Some("action was not signed".into()),
            })),
        )
            .into_response();
    };

    // Verify against the keyring.
    let Some(ref verifier) = state.signature_verifier else {
        return (
            StatusCode::OK,
            Json(serde_json::json!(VerifyResponse {
                signer_known: false,
                signature_well_formed: false,
                signer_id: Some(signer_id.clone()),
                algorithm: Some("Ed25519".into()),
                canonical_hash: None,
                reason: Some("signing is not enabled on this server".into()),
            })),
        )
            .into_response();
    };

    // The audit record stores a SHA-256 hash of the canonical bytes
    // that were signed at dispatch time. We cannot reconstruct the
    // original action from the audit record (it doesn't carry all
    // fields — id, created_at, metadata, etc. would differ). Instead,
    // we verify the signature against the stored canonical hash:
    // the signer is in the keyring, the signature length/format is
    // valid, and the canonical content that was signed is pinned by
    // the hash.
    let Some(ref stored_hash) = record.canonical_hash else {
        return (
            StatusCode::OK,
            Json(serde_json::json!(VerifyResponse {
                signer_known: verifier.keyring_contains(signer_id),
                signature_well_formed: false,
                signer_id: Some(signer_id.clone()),
                algorithm: Some("Ed25519".into()),
                canonical_hash: None,
                reason: Some(
                    "audit record has no canonical_hash — action was \
                     dispatched before signing metadata was stored"
                        .into()
                ),
            })),
        )
            .into_response();
    };

    // We cannot cryptographically verify the signature here: that needs the
    // original signed bytes, and the audit record keeps only their hash. So we
    // report the two facts we CAN establish — the signer is known to the
    // keyring, and the stored signature is well-formed (decodes to 64 bytes,
    // the Ed25519 signature size) — and hand back the canonical_hash for the
    // caller to verify independently.
    if !verifier.keyring_contains(signer_id) {
        return (
            StatusCode::OK,
            Json(serde_json::json!(VerifyResponse {
                signer_known: false,
                signature_well_formed: false,
                signer_id: Some(signer_id.clone()),
                algorithm: Some("Ed25519".into()),
                canonical_hash: Some(stored_hash.clone()),
                reason: Some(format!("unknown signer: {signer_id}")),
            })),
        )
            .into_response();
    }

    let sig_bytes = base64::engine::general_purpose::STANDARD.decode(signature);
    let signature_well_formed = sig_bytes.as_ref().is_ok_and(|b| b.len() == 64);

    (
        StatusCode::OK,
        Json(serde_json::json!(VerifyResponse {
            signer_known: true,
            signature_well_formed,
            signer_id: Some(signer_id.clone()),
            algorithm: Some("Ed25519".into()),
            canonical_hash: Some(stored_hash.clone()),
            reason: Some(if signature_well_formed {
                "signer known and signature well-formed; this is NOT a full \
                 cryptographic verification — recompute canonical_bytes() on \
                 the original action and check it against canonical_hash, then \
                 verify the signature against those bytes"
                    .into()
            } else {
                "signature is malformed (expected 64-byte Ed25519)".to_owned()
            }),
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_core::Action;
    use acteon_crypto::signing::{ActionSigningKey, Keyring, generate_keypair_with_kid};

    fn scope(tenants: &[&str]) -> SignerScope {
        SignerScope {
            tenants: tenants.iter().map(|t| (*t).to_owned()).collect(),
            namespaces: vec!["*".to_owned()],
        }
    }

    fn sign(sk: &ActionSigningKey, signer_id: &str, kid: Option<&str>, tenant: &str) -> Action {
        let mut action = Action::new("n", tenant, "email", "send", serde_json::json!({}));
        action.signer_id = Some(signer_id.to_owned());
        action.kid = kid.map(ToOwned::to_owned);
        let canonical = action.canonical_bytes();
        action.signature = Some(sk.sign(&canonical));
        action
    }

    #[test]
    fn scope_is_enforced_per_kid_not_collapsed_on_rotation() {
        // Signer "s1" rotated: k1 is scoped to t1, k2 to t2.
        let (sk1, vk1) = generate_keypair_with_kid("s1", "k1");
        let (sk2, vk2) = generate_keypair_with_kid("s1", "k2");
        let mut keyring = Keyring::new();
        keyring.insert(vk1);
        keyring.insert(vk2);
        let mut verifier = SignatureVerifier::new(keyring, false);
        verifier.add_scope("s1", "k1", scope(&["t1"]));
        verifier.add_scope("s1", "k2", scope(&["t2"]));

        // k1 signing for its own tenant t1 → verified.
        let a = sign(&sk1, "s1", Some("k1"), "t1");
        assert!(matches!(
            verifier.verify_action(&a),
            VerifyOutcome::Verified { .. }
        ));

        // k1 signing for t2 → DENIED. Before keying scopes by kid, the single
        // signer-wide scope was whichever entry loaded last; if that were k2's
        // (allows t2) this k1-signed action would be wrongly accepted.
        let b = sign(&sk1, "s1", Some("k1"), "t2");
        assert!(
            matches!(
                verifier.verify_action(&b),
                VerifyOutcome::ScopeDenied { .. }
            ),
            "a k1-signed action for t2 must be scope-denied"
        );

        // k2 signing for t2 → verified (its own scope).
        let c = sign(&sk2, "s1", Some("k2"), "t2");
        assert!(matches!(
            verifier.verify_action(&c),
            VerifyOutcome::Verified { .. }
        ));
    }

    #[test]
    fn kidless_action_must_satisfy_every_signer_scope() {
        // A legacy action without a kid is accepted by `keyring.verify` against
        // any of the signer's keys without reporting which, so it must satisfy
        // EVERY scope for the signer (fail closed).
        let (sk1, vk1) = generate_keypair_with_kid("s1", "k1");
        let (_sk2, vk2) = generate_keypair_with_kid("s1", "k2");
        let mut keyring = Keyring::new();
        keyring.insert(vk1);
        keyring.insert(vk2);
        let mut verifier = SignatureVerifier::new(keyring, false);
        verifier.add_scope("s1", "k1", scope(&["t1"])); // narrow
        verifier.add_scope("s1", "k2", scope(&["t1", "t2"])); // broad

        // t2 is allowed by k2 but not k1 → fail closed → denied.
        let denied = sign(&sk1, "s1", None, "t2");
        assert!(matches!(
            verifier.verify_action(&denied),
            VerifyOutcome::ScopeDenied { .. }
        ));

        // t1 is allowed by both → verified.
        let ok = sign(&sk1, "s1", None, "t1");
        assert!(matches!(
            verifier.verify_action(&ok),
            VerifyOutcome::Verified { .. }
        ));
    }
}
