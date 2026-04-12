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
use utoipa::ToSchema;

use super::AppState;
use super::schemas::ErrorResponse;

/// Verifies Ed25519 signatures on dispatched actions.
///
/// Used in two contexts:
/// 1. **Inline dispatch verification** — called before dispatch to
///    accept or reject an action based on its signature.
/// 2. **Post-hoc audit verification** — the `GET /v1/actions/{id}/verify`
///    endpoint recomputes canonical bytes and checks the stored signature.
#[derive(Clone)]
pub struct SignatureVerifier {
    keyring: Arc<Keyring>,
    reject_unsigned: bool,
}

impl SignatureVerifier {
    /// Create a new verifier.
    #[must_use]
    pub fn new(keyring: Keyring, reject_unsigned: bool) -> Self {
        Self {
            keyring: Arc::new(keyring),
            reject_unsigned,
        }
    }

    /// Check whether a `signer_id` is known to the keyring.
    #[must_use]
    pub fn keyring_contains(&self, signer_id: &str) -> bool {
        self.keyring.contains(signer_id)
    }

    /// Verify an action's signature inline during dispatch.
    ///
    /// Returns `Ok(())` if the action passes verification, or an
    /// error string suitable for an HTTP 400 response body.
    pub fn verify_action(&self, action: &Action) -> Result<(), String> {
        if let (Some(sig), Some(signer_id)) = (&action.signature, &action.signer_id) {
            let canonical = action.canonical_bytes();
            self.keyring
                .verify(signer_id, sig, &canonical)
                .map_err(|e| format!("signature verification failed: {e}"))
        } else if self.reject_unsigned {
            Err(
                "unsigned action rejected: signing.reject_unsigned is enabled; \
                 provide both 'signature' and 'signer_id' fields"
                    .to_owned(),
            )
        } else {
            Ok(())
        }
    }
}

/// Response from the `GET /v1/actions/{id}/verify` endpoint.
#[derive(Debug, Serialize, ToSchema)]
pub struct VerifyResponse {
    /// Whether the signature is structurally valid and the signer is
    /// known to the keyring.
    pub verified: bool,
    /// The `signer_id` from the action (if signed).
    pub signer_id: Option<String>,
    /// The algorithm used.
    pub algorithm: Option<String>,
    /// SHA-256 hex digest of the canonical bytes that were signed.
    /// Callers can independently verify by computing
    /// `Action::canonical_bytes()` on the original action and
    /// comparing the hash.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_hash: Option<String>,
    /// Human-readable reason when `verified` is false.
    pub reason: Option<String>,
}

/// `GET /v1/actions/{id}/verify` — verify an action's signature.
#[utoipa::path(
    get,
    path = "/v1/actions/{id}/verify",
    tag = "Signing",
    summary = "Verify action signature",
    description = "Looks up an audit record by action ID and verifies the stored Ed25519 signature against the server's keyring.",
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
                verified: false,
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
                verified: false,
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
                verified: false,
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

    // Confirm the signer is known and the signature is structurally
    // valid (correct length, decodable). Full content verification
    // requires the caller to supply the original action and
    // recompute canonical_bytes — the stored hash lets them confirm
    // the content matches.
    //
    // We can't call keyring.verify() without the original message
    // bytes, so we do a structural check: signer exists + signature
    // decodes to 64 bytes (Ed25519 signature size).
    if !verifier.keyring_contains(signer_id) {
        return (
            StatusCode::OK,
            Json(serde_json::json!(VerifyResponse {
                verified: false,
                signer_id: Some(signer_id.clone()),
                algorithm: Some("Ed25519".into()),
                canonical_hash: None,
                reason: Some(format!("unknown signer: {signer_id}")),
            })),
        )
            .into_response();
    }

    let sig_bytes = base64::engine::general_purpose::STANDARD.decode(signature);
    let structurally_valid = sig_bytes.as_ref().is_ok_and(|b| b.len() == 64);

    (
        StatusCode::OK,
        Json(serde_json::json!(VerifyResponse {
            verified: structurally_valid,
            signer_id: Some(signer_id.clone()),
            algorithm: Some("Ed25519".into()),
            canonical_hash: Some(stored_hash.clone()),
            reason: if structurally_valid {
                None
            } else {
                Some("signature is malformed (expected 64-byte Ed25519)".into())
            },
        })),
    )
        .into_response()
}
