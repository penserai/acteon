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
    /// Whether the signature is valid.
    pub verified: bool,
    /// The `signer_id` from the action (if signed).
    pub signer_id: Option<String>,
    /// The algorithm used.
    pub algorithm: Option<String>,
    /// Human-readable reason when `verified` is false.
    pub reason: Option<String>,
}

/// `GET /v1/actions/{id}/verify` — verify an action's signature.
#[utoipa::path(
    get,
    path = "/v1/actions/{id}/verify",
    tag = "Signing",
    summary = "Verify action signature",
    description = "Looks up an audit record by action ID, recomputes the canonical bytes, and verifies the stored Ed25519 signature against the server's keyring. Returns a JSON object with `verified`, `signer_id`, `algorithm`, and an optional `reason` when verification fails.",
    params(("id" = String, Path, description = "Action ID")),
    responses(
        (status = 200, description = "Verification result", body = VerifyResponse),
        (status = 404, description = "Action not found", body = ErrorResponse),
    )
)]
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
                reason: Some("signing is not enabled on this server".into()),
            })),
        )
            .into_response();
    };

    // Reconstruct the canonical bytes from the audit record's stored
    // payload + fields. We build a minimal Action for canonicalization.
    let action = Action::new(
        record.namespace.as_str(),
        record.tenant.as_str(),
        record.provider.as_str(),
        &record.action_type,
        record.action_payload.clone().unwrap_or_default(),
    );
    // Note: this reconstructed action won't have all original fields
    // (metadata, dedup_key, etc.) so full verification requires the
    // original action to be stored. For v1, we verify what we can.
    let canonical = action.canonical_bytes();

    match verifier.keyring.verify(signer_id, signature, &canonical) {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!(VerifyResponse {
                verified: true,
                signer_id: Some(signer_id.clone()),
                algorithm: Some("Ed25519".into()),
                reason: None,
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::OK,
            Json(serde_json::json!(VerifyResponse {
                verified: false,
                signer_id: Some(signer_id.clone()),
                algorithm: Some("Ed25519".into()),
                reason: Some(format!("{e}")),
            })),
        )
            .into_response(),
    }
}
