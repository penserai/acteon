//! Compliance mode API endpoints.
//!
//! Provides the current compliance status and audit hash chain verification.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::AppState;
use super::schemas::ErrorResponse;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Current compliance status including active mode and feature flags.
#[derive(Debug, Serialize, ToSchema)]
pub struct ComplianceStatusResponse {
    /// Active compliance mode (`"none"`, `"soc2"`, or `"hipaa"`).
    #[schema(example = "soc2")]
    pub mode: String,
    /// Whether synchronous audit writes are enabled.
    pub sync_audit_writes: bool,
    /// Whether audit records are immutable (deletions blocked).
    pub immutable_audit: bool,
    /// Whether `SHA-256` hash chaining is enabled.
    pub hash_chain: bool,
}

/// Request body for verifying an audit hash chain.
#[derive(Debug, Deserialize, ToSchema)]
pub struct VerifyChainRequest {
    /// Namespace to verify.
    #[schema(example = "notifications")]
    pub namespace: String,
    /// Tenant to verify.
    #[schema(example = "tenant-1")]
    pub tenant: String,
    /// Optional: only verify records dispatched at or after this time.
    #[serde(default)]
    pub from: Option<chrono::DateTime<chrono::Utc>>,
    /// Optional: only verify records dispatched at or before this time.
    #[serde(default)]
    pub to: Option<chrono::DateTime<chrono::Utc>>,
}

/// Result of hash chain verification.
#[derive(Debug, Serialize, ToSchema)]
pub struct VerifyChainResponse {
    /// Whether the chain is valid (no broken links or tampered hashes).
    pub valid: bool,
    /// Number of records checked during verification.
    pub records_checked: u64,
    /// ID of the record where the chain first broke, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_broken_at: Option<String>,
    /// ID of the first record in the verified range.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_record_id: Option<String>,
    /// ID of the last record in the verified range.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_record_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Get the current compliance status.
#[utoipa::path(
    get,
    path = "/v1/compliance/status",
    tag = "compliance",
    responses(
        (status = 200, description = "Current compliance status", body = ComplianceStatusResponse),
    )
)]
pub async fn get_compliance_status(State(state): State<AppState>) -> impl IntoResponse {
    let gateway = state.gateway.read().await;
    let response = match gateway.compliance_config() {
        Some(config) => ComplianceStatusResponse {
            mode: config.mode.to_string(),
            sync_audit_writes: config.sync_audit_writes,
            immutable_audit: config.immutable_audit,
            hash_chain: config.hash_chain,
        },
        None => ComplianceStatusResponse {
            mode: "none".to_string(),
            sync_audit_writes: false,
            immutable_audit: false,
            hash_chain: false,
        },
    };
    Json(response)
}

/// Verify the integrity of the audit hash chain for a namespace/tenant pair.
#[utoipa::path(
    post,
    path = "/v1/audit/verify",
    tag = "compliance",
    request_body = VerifyChainRequest,
    responses(
        (status = 200, description = "Chain verification result", body = VerifyChainResponse),
        (status = 400, description = "Hash chaining is not enabled", body = ErrorResponse),
        (status = 500, description = "Verification failed", body = ErrorResponse),
    )
)]
pub async fn verify_audit_chain(
    State(state): State<AppState>,
    Json(req): Json<VerifyChainRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    let gateway = state.gateway.read().await;
    let result = gateway
        .verify_audit_chain(&req.namespace, &req.tenant, req.from, req.to)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("verification failed: {e}"),
                }),
            )
        })?;

    match result {
        Some(verification) => Ok(Json(VerifyChainResponse {
            valid: verification.valid,
            records_checked: verification.records_checked,
            first_broken_at: verification.first_broken_at,
            first_record_id: verification.first_record_id,
            last_record_id: verification.last_record_id,
        })),
        None => Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "hash chaining is not enabled".to_string(),
            }),
        )),
    }
}
