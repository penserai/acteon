use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use acteon_core::ChainStatus;

use super::AppState;
use super::schemas::ErrorResponse;

/// Query parameters for listing chain executions.
#[derive(Debug, Deserialize, IntoParams)]
pub struct ChainQueryParams {
    /// Namespace to filter by.
    pub namespace: String,
    /// Tenant to filter by.
    pub tenant: String,
    /// Optional status filter: `"running"`, `"completed"`, `"failed"`, `"cancelled"`, `"timed_out"`.
    pub status: Option<String>,
}

/// Namespace/tenant query for chain detail endpoints.
#[derive(Debug, Deserialize, IntoParams)]
pub struct ChainNamespaceParams {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
}

/// Request body for cancelling a chain.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ChainCancelRequest {
    /// Namespace of the chain.
    pub namespace: String,
    /// Tenant of the chain.
    pub tenant: String,
    /// Optional reason for cancellation.
    #[serde(default)]
    pub reason: Option<String>,
    /// Optional identifier of who cancelled the chain.
    #[serde(default)]
    pub cancelled_by: Option<String>,
}

/// Summary of a chain execution for list responses.
#[derive(Debug, Serialize, ToSchema)]
pub struct ChainSummary {
    /// Unique chain execution ID.
    pub chain_id: String,
    /// Name of the chain configuration.
    pub chain_name: String,
    /// Current status.
    pub status: String,
    /// Current step index (0-based).
    pub current_step: usize,
    /// Total number of steps.
    pub total_steps: usize,
    /// When the chain started.
    pub started_at: DateTime<Utc>,
    /// When the chain was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Response for listing chain executions.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListChainsResponse {
    /// List of chain execution summaries.
    pub chains: Vec<ChainSummary>,
}

/// Detailed status of a single chain step.
#[derive(Debug, Serialize, ToSchema)]
pub struct ChainStepStatus {
    /// Step name.
    pub name: String,
    /// Provider used for this step.
    pub provider: String,
    /// Step status: `"pending"`, `"completed"`, `"failed"`, `"skipped"`.
    pub status: String,
    /// Response body from the provider (if completed).
    #[schema(value_type = Option<Object>)]
    pub response_body: Option<serde_json::Value>,
    /// Error message (if failed).
    pub error: Option<String>,
    /// When this step completed.
    pub completed_at: Option<DateTime<Utc>>,
}

/// Full detail response for a chain execution.
#[derive(Debug, Serialize, ToSchema)]
pub struct ChainDetailResponse {
    /// Unique chain execution ID.
    pub chain_id: String,
    /// Name of the chain configuration.
    pub chain_name: String,
    /// Current status.
    pub status: String,
    /// Current step index (0-based).
    pub current_step: usize,
    /// Total number of steps.
    pub total_steps: usize,
    /// Per-step status details.
    pub steps: Vec<ChainStepStatus>,
    /// When the chain started.
    pub started_at: DateTime<Utc>,
    /// When the chain was last updated.
    pub updated_at: DateTime<Utc>,
    /// When the chain will time out.
    pub expires_at: Option<DateTime<Utc>>,
    /// Reason for cancellation (if cancelled).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel_reason: Option<String>,
    /// Who cancelled the chain (if cancelled).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancelled_by: Option<String>,
}

fn parse_status_filter(s: &str) -> Option<ChainStatus> {
    match s {
        "running" => Some(ChainStatus::Running),
        "completed" => Some(ChainStatus::Completed),
        "failed" => Some(ChainStatus::Failed),
        "cancelled" => Some(ChainStatus::Cancelled),
        "timed_out" => Some(ChainStatus::TimedOut),
        _ => None,
    }
}

fn status_to_string(s: &ChainStatus) -> String {
    match s {
        ChainStatus::Running => "running".into(),
        ChainStatus::Completed => "completed".into(),
        ChainStatus::Failed => "failed".into(),
        ChainStatus::Cancelled => "cancelled".into(),
        ChainStatus::TimedOut => "timed_out".into(),
    }
}

/// `GET /v1/chains` -- list chain executions.
#[utoipa::path(
    get,
    path = "/v1/chains",
    tag = "Chains",
    summary = "List chain executions",
    description = "Returns chain executions filtered by namespace, tenant, and optional status.",
    params(ChainQueryParams),
    responses(
        (status = 200, description = "Chain list", body = ListChainsResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
    )
)]
pub async fn list_chains(
    State(state): State<AppState>,
    Query(params): Query<ChainQueryParams>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let status_filter = params.status.as_deref().and_then(parse_status_filter);

    match gw
        .list_chains(&params.namespace, &params.tenant, status_filter.as_ref())
        .await
    {
        Ok(chains) => {
            let summaries: Vec<ChainSummary> = chains
                .iter()
                .map(|c| ChainSummary {
                    chain_id: c.chain_id.clone(),
                    chain_name: c.chain_name.clone(),
                    status: status_to_string(&c.status),
                    current_step: c.current_step,
                    total_steps: c.total_steps,
                    started_at: c.started_at,
                    updated_at: c.updated_at,
                })
                .collect();
            (
                StatusCode::OK,
                Json(ListChainsResponse { chains: summaries }),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

/// `GET /v1/chains/{chain_id}` -- get chain execution details.
#[utoipa::path(
    get,
    path = "/v1/chains/{chain_id}",
    tag = "Chains",
    summary = "Get chain execution status",
    description = "Returns full details of a chain execution including step results.",
    params(
        ("chain_id" = String, Path, description = "Chain execution ID"),
        ChainNamespaceParams,
    ),
    responses(
        (status = 200, description = "Chain details", body = ChainDetailResponse),
        (status = 404, description = "Chain not found", body = ErrorResponse),
    )
)]
pub async fn get_chain(
    State(state): State<AppState>,
    Path(chain_id): Path<String>,
    Query(params): Query<ChainNamespaceParams>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;

    match gw
        .get_chain_status(&params.namespace, &params.tenant, &chain_id)
        .await
    {
        Ok(Some(chain_state)) => {
            // Build per-step status from the chain config and results.
            let steps: Vec<ChainStepStatus> = (0..chain_state.total_steps)
                .map(|i| {
                    let result = chain_state.step_results.get(i).and_then(|r| r.as_ref());
                    let step_name =
                        result.map_or_else(|| format!("step-{i}"), |r| r.step_name.clone());
                    let (status, resp_body, error, completed) = if let Some(r) = result {
                        let s = if r.success { "completed" } else { "failed" };
                        (
                            s.to_string(),
                            r.response_body.clone(),
                            r.error.clone(),
                            Some(r.completed_at),
                        )
                    } else {
                        ("pending".to_string(), None, None, None)
                    };
                    ChainStepStatus {
                        name: step_name,
                        provider: String::new(),
                        status,
                        response_body: resp_body,
                        error,
                        completed_at: completed,
                    }
                })
                .collect();

            let detail = ChainDetailResponse {
                chain_id: chain_state.chain_id,
                chain_name: chain_state.chain_name,
                status: status_to_string(&chain_state.status),
                current_step: chain_state.current_step,
                total_steps: chain_state.total_steps,
                steps,
                started_at: chain_state.started_at,
                updated_at: chain_state.updated_at,
                expires_at: chain_state.expires_at,
                cancel_reason: chain_state.cancel_reason,
                cancelled_by: chain_state.cancelled_by,
            };
            (StatusCode::OK, Json(detail)).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("chain not found: {chain_id}"),
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

/// `POST /v1/chains/{chain_id}/cancel` -- cancel a running chain.
#[utoipa::path(
    post,
    path = "/v1/chains/{chain_id}/cancel",
    tag = "Chains",
    summary = "Cancel a running chain",
    description = "Cancels a running chain execution. Returns an error if already completed/failed.",
    params(("chain_id" = String, Path, description = "Chain execution ID")),
    responses(
        (status = 200, description = "Chain cancelled", body = ChainDetailResponse),
        (status = 404, description = "Chain not found", body = ErrorResponse),
        (status = 409, description = "Chain already finished", body = ErrorResponse),
    )
)]
pub async fn cancel_chain(
    State(state): State<AppState>,
    Path(chain_id): Path<String>,
    Json(params): Json<ChainCancelRequest>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;

    match gw
        .cancel_chain(
            &params.namespace,
            &params.tenant,
            &chain_id,
            params.reason,
            params.cancelled_by,
        )
        .await
    {
        Ok(chain_state) => {
            let detail = ChainDetailResponse {
                chain_id: chain_state.chain_id,
                chain_name: chain_state.chain_name,
                status: status_to_string(&chain_state.status),
                current_step: chain_state.current_step,
                total_steps: chain_state.total_steps,
                steps: Vec::new(),
                started_at: chain_state.started_at,
                updated_at: chain_state.updated_at,
                expires_at: chain_state.expires_at,
                cancel_reason: chain_state.cancel_reason,
                cancelled_by: chain_state.cancelled_by,
            };
            (StatusCode::OK, Json(detail)).into_response()
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") {
                (StatusCode::NOT_FOUND, Json(ErrorResponse { error: msg })).into_response()
            } else if msg.contains("not running") {
                (StatusCode::CONFLICT, Json(ErrorResponse { error: msg })).into_response()
            } else {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse { error: msg }),
                )
                    .into_response()
            }
        }
    }
}
