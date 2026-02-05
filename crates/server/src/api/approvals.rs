//! Approval API endpoints.
//!
//! Provides endpoints for the human-in-the-loop approval workflow.
//! Public endpoints (approve/reject/status) are authenticated by HMAC signature
//! in the query string. The list endpoint requires standard authentication.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use acteon_gateway::GatewayError;

use crate::error::ServerError;

use super::AppState;
use super::schemas::ErrorResponse;

/// Response returned when an approval is executed or rejected.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ApprovalActionResponse {
    /// The approval ID.
    #[schema(example = "550e8400-e29b-41d4-a716-446655440000")]
    pub id: String,
    /// The resulting status ("approved" or "rejected").
    #[schema(example = "approved")]
    pub status: String,
    /// The outcome of the original action (only present when approved).
    pub outcome: Option<serde_json::Value>,
}

/// Public-facing approval status response (no payload exposed).
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ApprovalStatusResponse {
    /// The approval token.
    #[schema(example = "550e8400-e29b-41d4-a716-446655440000")]
    pub token: String,
    /// Current status: "pending", "approved", or "rejected".
    #[schema(example = "pending")]
    pub status: String,
    /// Rule that triggered the approval.
    #[schema(example = "approve-large-refunds")]
    pub rule: String,
    /// When the approval was created.
    #[schema(example = "2024-01-15T10:30:00Z")]
    pub created_at: String,
    /// When the approval expires.
    #[schema(example = "2024-01-16T10:30:00Z")]
    pub expires_at: String,
    /// When a decision was made (if any).
    pub decided_at: Option<String>,
    /// Optional message.
    pub message: Option<String>,
}

/// Query parameters for listing pending approvals.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ApprovalQueryParams {
    /// Filter by namespace.
    pub namespace: String,
    /// Filter by tenant.
    pub tenant: String,
}

/// Query parameters for HMAC-authenticated approval endpoints.
#[derive(Debug, Deserialize)]
pub struct SigQuery {
    /// HMAC-SHA256 signature.
    pub sig: String,
    /// Expiration timestamp (unix seconds) bound into the signature.
    pub expires_at: i64,
    /// Key ID identifying which HMAC key was used to produce the signature.
    pub kid: Option<String>,
}

/// Response for listing pending approvals.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ListApprovalsResponse {
    /// List of pending approvals.
    pub approvals: Vec<ApprovalStatusResponse>,
    /// Total number of approvals returned.
    #[schema(example = 3)]
    pub count: usize,
}

/// Path parameters for approval endpoints.
#[derive(Debug, Deserialize)]
pub struct ApprovalPath {
    pub namespace: String,
    pub tenant: String,
    pub id: String,
}

/// `POST /v1/approvals/{namespace}/{tenant}/{id}/approve` -- approve a pending action (HMAC-authenticated).
#[utoipa::path(
    post,
    path = "/v1/approvals/{namespace}/{tenant}/{id}/approve",
    tag = "Approvals",
    summary = "Approve a pending action",
    description = "Approves a pending action identified by namespace, tenant, and ID. Authenticated by HMAC signature in query parameter.",
    params(
        ("namespace" = String, Path, description = "Approval namespace"),
        ("tenant" = String, Path, description = "Approval tenant"),
        ("id" = String, Path, description = "Approval ID"),
        ("sig" = String, Query, description = "HMAC-SHA256 signature"),
        ("expires_at" = i64, Query, description = "Expiration timestamp (unix seconds) bound into the signature"),
        ("kid" = Option<String>, Query, description = "Key ID identifying which HMAC key was used"),
    ),
    responses(
        (status = 200, description = "Action approved and executed", body = ApprovalActionResponse),
        (status = 404, description = "Approval not found or expired", body = ErrorResponse),
        (status = 410, description = "Approval already decided", body = ErrorResponse),
    )
)]
pub async fn approve(
    State(state): State<AppState>,
    Path(path): Path<ApprovalPath>,
    Query(sig_query): Query<SigQuery>,
) -> Result<impl IntoResponse, ServerError> {
    let gw = state.gateway.read().await;

    match gw
        .execute_approval(
            &path.namespace,
            &path.tenant,
            &path.id,
            &sig_query.sig,
            sig_query.expires_at,
            sig_query.kid.as_deref(),
        )
        .await
    {
        Ok(outcome) => {
            let outcome_json = serde_json::to_value(&outcome).ok();
            let response = ApprovalActionResponse {
                id: path.id,
                status: "approved".into(),
                outcome: outcome_json,
            };
            Ok((StatusCode::OK, Json(serde_json::json!(response))))
        }
        Err(GatewayError::ApprovalNotFound) => Ok((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                error: "approval not found or expired".into(),
            })),
        )),
        Err(GatewayError::ApprovalAlreadyDecided(status)) => Ok((
            StatusCode::GONE,
            Json(serde_json::json!(ErrorResponse {
                error: format!("approval already decided: {status}"),
            })),
        )),
        Err(e) => Ok((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse {
                error: e.to_string(),
            })),
        )),
    }
}

/// `POST /v1/approvals/{namespace}/{tenant}/{id}/reject` -- reject a pending action (HMAC-authenticated).
#[utoipa::path(
    post,
    path = "/v1/approvals/{namespace}/{tenant}/{id}/reject",
    tag = "Approvals",
    summary = "Reject a pending action",
    description = "Rejects a pending action identified by namespace, tenant, and ID. Authenticated by HMAC signature in query parameter.",
    params(
        ("namespace" = String, Path, description = "Approval namespace"),
        ("tenant" = String, Path, description = "Approval tenant"),
        ("id" = String, Path, description = "Approval ID"),
        ("sig" = String, Query, description = "HMAC-SHA256 signature"),
        ("expires_at" = i64, Query, description = "Expiration timestamp (unix seconds) bound into the signature"),
        ("kid" = Option<String>, Query, description = "Key ID identifying which HMAC key was used"),
    ),
    responses(
        (status = 200, description = "Action rejected", body = ApprovalActionResponse),
        (status = 404, description = "Approval not found or expired", body = ErrorResponse),
        (status = 410, description = "Approval already decided", body = ErrorResponse),
    )
)]
pub async fn reject(
    State(state): State<AppState>,
    Path(path): Path<ApprovalPath>,
    Query(sig_query): Query<SigQuery>,
) -> Result<impl IntoResponse, ServerError> {
    let gw = state.gateway.read().await;

    match gw
        .reject_approval(
            &path.namespace,
            &path.tenant,
            &path.id,
            &sig_query.sig,
            sig_query.expires_at,
            sig_query.kid.as_deref(),
        )
        .await
    {
        Ok(()) => {
            let response = ApprovalActionResponse {
                id: path.id,
                status: "rejected".into(),
                outcome: None,
            };
            Ok((StatusCode::OK, Json(serde_json::json!(response))))
        }
        Err(GatewayError::ApprovalNotFound) => Ok((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                error: "approval not found or expired".into(),
            })),
        )),
        Err(GatewayError::ApprovalAlreadyDecided(status)) => Ok((
            StatusCode::GONE,
            Json(serde_json::json!(ErrorResponse {
                error: format!("approval already decided: {status}"),
            })),
        )),
        Err(e) => Ok((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse {
                error: e.to_string(),
            })),
        )),
    }
}

/// `GET /v1/approvals/{namespace}/{tenant}/{id}` -- get approval status (HMAC-authenticated).
#[utoipa::path(
    get,
    path = "/v1/approvals/{namespace}/{tenant}/{id}",
    tag = "Approvals",
    summary = "Get approval status",
    description = "Retrieves the status of an approval by namespace, tenant, and ID. Does not expose the original action payload. Authenticated by HMAC signature in query parameter.",
    params(
        ("namespace" = String, Path, description = "Approval namespace"),
        ("tenant" = String, Path, description = "Approval tenant"),
        ("id" = String, Path, description = "Approval ID"),
        ("sig" = String, Query, description = "HMAC-SHA256 signature"),
        ("expires_at" = i64, Query, description = "Expiration timestamp (unix seconds) bound into the signature"),
        ("kid" = Option<String>, Query, description = "Key ID identifying which HMAC key was used"),
    ),
    responses(
        (status = 200, description = "Approval status retrieved", body = ApprovalStatusResponse),
        (status = 404, description = "Approval not found or expired", body = ErrorResponse),
    )
)]
pub async fn get_approval(
    State(state): State<AppState>,
    Path(path): Path<ApprovalPath>,
    Query(sig_query): Query<SigQuery>,
) -> Result<impl IntoResponse, ServerError> {
    let gw = state.gateway.read().await;

    match gw
        .get_approval_status(
            &path.namespace,
            &path.tenant,
            &path.id,
            &sig_query.sig,
            sig_query.expires_at,
            sig_query.kid.as_deref(),
        )
        .await
    {
        Ok(Some(status)) => {
            let response = ApprovalStatusResponse {
                token: status.token,
                status: status.status,
                rule: status.rule,
                created_at: status.created_at.to_rfc3339(),
                expires_at: status.expires_at.to_rfc3339(),
                decided_at: status.decided_at.map(|dt| dt.to_rfc3339()),
                message: status.message,
            };
            Ok((StatusCode::OK, Json(serde_json::json!(response))))
        }
        Ok(None) => Ok((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                error: "approval not found or expired".into(),
            })),
        )),
        Err(e) => Ok((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse {
                error: e.to_string(),
            })),
        )),
    }
}

/// `GET /v1/approvals` -- list pending approvals (protected, requires auth).
#[utoipa::path(
    get,
    path = "/v1/approvals",
    tag = "Approvals",
    summary = "List pending approvals",
    description = "Lists pending approvals filtered by namespace and tenant. Requires authentication.",
    params(
        ("namespace" = String, Query, description = "Approval namespace"),
        ("tenant" = String, Query, description = "Approval tenant"),
    ),
    responses(
        (status = 200, description = "List of pending approvals", body = ListApprovalsResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse),
    )
)]
pub async fn list_approvals(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<crate::auth::identity::CallerIdentity>,
    Query(params): Query<ApprovalQueryParams>,
) -> Result<impl IntoResponse, ServerError> {
    use crate::auth::role::Permission;

    if !identity.role.has_permission(Permission::AuditRead) {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!(ErrorResponse {
                error: "insufficient permissions".into(),
            })),
        ));
    }

    let gw = state.gateway.read().await;

    match gw
        .list_pending_approvals(&params.namespace, &params.tenant)
        .await
    {
        Ok(statuses) => {
            let approvals: Vec<ApprovalStatusResponse> = statuses
                .into_iter()
                .map(|s| ApprovalStatusResponse {
                    token: s.token,
                    status: s.status,
                    rule: s.rule,
                    created_at: s.created_at.to_rfc3339(),
                    expires_at: s.expires_at.to_rfc3339(),
                    decided_at: s.decided_at.map(|dt| dt.to_rfc3339()),
                    message: s.message,
                })
                .collect();
            let response = ListApprovalsResponse {
                count: approvals.len(),
                approvals,
            };
            Ok((StatusCode::OK, Json(serde_json::json!(response))))
        }
        Err(e) => Ok((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse {
                error: e.to_string(),
            })),
        )),
    }
}
