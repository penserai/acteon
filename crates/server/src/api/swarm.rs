//! Swarm provider HTTP endpoints.
//!
//! Exposes list/get/cancel operations over the `acteon-swarm-provider`
//! registry. Only compiled when the server is built with the `swarm`
//! feature; the routes are still registered unconditionally but return
//! `503 Service Unavailable` when no registry is bound.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};

use super::AppState;
use super::schemas::ErrorResponse;

/// Query parameters for listing swarm runs.
#[derive(Debug, Deserialize, IntoParams)]
pub struct ListSwarmRunsParams {
    /// Optional namespace filter.
    #[serde(default)]
    pub namespace: Option<String>,
    /// Optional tenant filter.
    #[serde(default)]
    pub tenant: Option<String>,
    /// Optional status filter: `accepted`, `running`, `completed`, etc.
    #[serde(default)]
    pub status: Option<String>,
    /// Maximum number of results (default: 100).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Number of results to skip (default: 0).
    #[serde(default)]
    pub offset: Option<usize>,
}

/// Response body for list/get operations. Uses a newtype so the `OpenAPI`
/// schema reflects the registry's snapshot shape.
#[derive(Debug, serde::Serialize, ToSchema)]
pub struct SwarmRunList {
    pub runs: Vec<SwarmRunApiSnapshot>,
    pub total: usize,
}

/// Public snapshot shape — same fields as the registry's
/// `SwarmRunSnapshot` but with `metrics` widened to `Object` for `OpenAPI`.
#[derive(Debug, serde::Serialize, ToSchema)]
pub struct SwarmRunApiSnapshot {
    pub run_id: String,
    pub plan_id: String,
    pub objective: String,
    pub status: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub metrics: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub namespace: String,
    pub tenant: String,
}

#[cfg(feature = "swarm")]
fn to_api_snapshot(snap: acteon_swarm_provider::SwarmRunSnapshot) -> SwarmRunApiSnapshot {
    SwarmRunApiSnapshot {
        run_id: snap.run_id,
        plan_id: snap.plan_id,
        objective: snap.objective,
        status: format!("{:?}", snap.status).to_lowercase(),
        started_at: snap.started_at,
        finished_at: snap.finished_at,
        metrics: snap.metrics.and_then(|m| serde_json::to_value(m).ok()),
        error: snap.error,
        namespace: snap.namespace,
        tenant: snap.tenant,
    }
}

#[cfg(feature = "swarm")]
fn parse_status(s: &str) -> Option<acteon_swarm_provider::SwarmRunStatus> {
    use acteon_swarm_provider::SwarmRunStatus;
    match s.to_ascii_lowercase().as_str() {
        "accepted" => Some(SwarmRunStatus::Accepted),
        "running" => Some(SwarmRunStatus::Running),
        "adversarial" => Some(SwarmRunStatus::Adversarial),
        "completed" => Some(SwarmRunStatus::Completed),
        "failed" => Some(SwarmRunStatus::Failed),
        "cancelled" => Some(SwarmRunStatus::Cancelled),
        "timed_out" | "timedout" => Some(SwarmRunStatus::TimedOut),
        "cancelling" => Some(SwarmRunStatus::Cancelling),
        _ => None,
    }
}

/// Maximum runs returned in a single `GET /v1/swarm/runs` response.
/// Mirrors the registry-layer `MAX_LIST_PAGE` so bad client inputs or
/// malicious `?limit=2^64` queries cannot exhaust server resources.
#[cfg(feature = "swarm")]
const MAX_API_PAGE: usize = 500;

#[utoipa::path(
    get,
    path = "/v1/swarm/runs",
    params(ListSwarmRunsParams),
    responses(
        (status = 200, description = "List of swarm runs", body = SwarmRunList),
        (status = 400, description = "Invalid query parameter", body = ErrorResponse),
        (status = 503, description = "Swarm feature disabled", body = ErrorResponse),
    ),
    tag = "swarm",
)]
pub async fn list_swarm_runs(
    State(state): State<AppState>,
    Query(params): Query<ListSwarmRunsParams>,
) -> impl IntoResponse {
    #[cfg(feature = "swarm")]
    {
        let Some(registry) = state.swarm_registry.as_ref() else {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse {
                    error: "swarm provider not configured".into(),
                }),
            )
                .into_response();
        };
        // Reject unknown status values rather than silently dropping the
        // filter and returning an unfiltered dataset.
        let status = match params.status.as_deref() {
            Some(raw) => match parse_status(raw) {
                Some(s) => Some(s),
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse {
                            error: format!(
                                "unknown status '{raw}'. Expected one of: accepted, \
                                 running, adversarial, completed, failed, cancelled, \
                                 cancelling, timed_out"
                            ),
                        }),
                    )
                        .into_response();
                }
            },
            None => None,
        };
        // Clamp limit so a `?limit=2^30` request cannot starve the server.
        let effective_limit = params.limit.map(|n| n.min(MAX_API_PAGE));
        let filter = acteon_swarm_provider::SwarmRunFilter {
            namespace: params.namespace,
            tenant: params.tenant,
            status,
            limit: effective_limit,
            offset: params.offset,
        };
        let (runs, total) = registry.list(&filter);
        let body = SwarmRunList {
            runs: runs.into_iter().map(to_api_snapshot).collect(),
            total,
        };
        (StatusCode::OK, Json(body)).into_response()
    }
    #[cfg(not(feature = "swarm"))]
    {
        let _ = (state, params);
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "swarm feature not compiled".into(),
            }),
        )
            .into_response()
    }
}

#[utoipa::path(
    get,
    path = "/v1/swarm/runs/{run_id}",
    responses(
        (status = 200, description = "Swarm run snapshot", body = SwarmRunApiSnapshot),
        (status = 404, description = "Run not found", body = ErrorResponse),
        (status = 503, description = "Swarm feature disabled", body = ErrorResponse),
    ),
    tag = "swarm",
)]
pub async fn get_swarm_run(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> impl IntoResponse {
    #[cfg(feature = "swarm")]
    {
        let Some(registry) = state.swarm_registry.as_ref() else {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse {
                    error: "swarm provider not configured".into(),
                }),
            )
                .into_response();
        };
        match registry.get(&run_id) {
            Some(snap) => (StatusCode::OK, Json(to_api_snapshot(snap))).into_response(),
            None => (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("swarm run not found: {run_id}"),
                }),
            )
                .into_response(),
        }
    }
    #[cfg(not(feature = "swarm"))]
    {
        let _ = (state, run_id);
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "swarm feature not compiled".into(),
            }),
        )
            .into_response()
    }
}

#[utoipa::path(
    post,
    path = "/v1/swarm/runs/{run_id}/cancel",
    responses(
        (status = 200, description = "Cancellation requested", body = SwarmRunApiSnapshot),
        (status = 404, description = "Run not found", body = ErrorResponse),
        (status = 503, description = "Swarm feature disabled", body = ErrorResponse),
    ),
    tag = "swarm",
)]
pub async fn cancel_swarm_run(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> impl IntoResponse {
    #[cfg(feature = "swarm")]
    {
        let Some(registry) = state.swarm_registry.as_ref() else {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse {
                    error: "swarm provider not configured".into(),
                }),
            )
                .into_response();
        };
        match registry.cancel(&run_id).await {
            Ok(snap) => (StatusCode::OK, Json(to_api_snapshot(snap))).into_response(),
            Err(acteon_swarm_provider::SwarmProviderError::NotFound(_)) => (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("swarm run not found: {run_id}"),
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
    #[cfg(not(feature = "swarm"))]
    {
        let _ = (state, run_id);
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "swarm feature not compiled".into(),
            }),
        )
            .into_response()
    }
}
