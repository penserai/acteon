//! Execution visibility API: list/filter executions across chains
//! (including terminal ones), per-execution event history, external
//! signals, and search-attribute upserts.

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use acteon_gateway::ExecutionFilter;

use super::AppState;
use super::chains::{parse_status_filter, status_to_string};
use super::schemas::ErrorResponse;
use crate::auth::identity::CallerIdentity;

/// Build a `403 Forbidden` response for a caller whose grants don't cover
/// the requested `(namespace, tenant)`.
fn tenant_forbidden(namespace: &str, tenant: &str) -> axum::response::Response {
    (
        StatusCode::FORBIDDEN,
        Json(ErrorResponse {
            error: format!("forbidden: no grant covers tenant={tenant} namespace={namespace}"),
        }),
    )
        .into_response()
}

/// Query parameters for listing executions.
#[derive(Debug, Deserialize, IntoParams)]
pub struct ExecutionsQueryParams {
    /// Namespace to filter by.
    pub namespace: String,
    /// Tenant to filter by.
    pub tenant: String,
    /// Optional chain definition name filter.
    pub chain_name: Option<String>,
    /// Optional status filter (`running`, `completed`, `failed`, `cancelled`,
    /// `timed_out`, `waiting_sub_chain`, `waiting_parallel`, `waiting_timer`,
    /// `waiting_signal`, `waiting_worker`).
    pub status: Option<String>,
    /// Only executions started at or after this time (RFC 3339).
    pub started_after: Option<DateTime<Utc>>,
    /// Only executions started at or before this time (RFC 3339).
    pub started_before: Option<DateTime<Utc>>,
    /// Search-attribute filter in `key=value` form.
    pub attr: Option<String>,
    /// Maximum number of executions to return (default 200).
    pub limit: Option<usize>,
}

/// Namespace/tenant query for execution detail endpoints.
#[derive(Debug, Deserialize, IntoParams)]
pub struct ExecutionNamespaceParams {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
}

/// Summary of one execution for visibility list responses.
#[derive(Debug, Serialize, ToSchema)]
pub struct ExecutionSummary {
    /// Unique execution ID.
    pub execution_id: String,
    /// Name of the chain definition.
    pub chain_name: String,
    /// Definition version pinned by this execution.
    pub version: u64,
    /// Current status.
    pub status: String,
    /// Current step index (0-based).
    pub current_step: usize,
    /// Total number of steps.
    pub total_steps: usize,
    /// When the execution started.
    pub started_at: DateTime<Utc>,
    /// When the execution was last updated.
    pub updated_at: DateTime<Utc>,
    /// User-defined search attributes.
    #[schema(value_type = Object)]
    pub search_attributes: HashMap<String, serde_json::Value>,
    /// What the execution is currently waiting on (timer / signal / worker),
    /// when paused.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub wait_state: Option<serde_json::Value>,
    /// Parent execution ID for sub-chains.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_execution_id: Option<String>,
}

/// Response for listing executions.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListExecutionsResponse {
    /// Matching executions, most recently started first.
    pub executions: Vec<ExecutionSummary>,
}

/// Response carrying an execution's full event history.
#[derive(Debug, Serialize, ToSchema)]
pub struct ExecutionHistoryResponse {
    /// Execution ID.
    pub execution_id: String,
    /// Ordered history events (`event_id`, `timestamp`, `event_type`, and
    /// per-event payload fields).
    #[schema(value_type = Vec<Object>)]
    pub events: Vec<serde_json::Value>,
}

/// Request body for delivering a signal to an execution.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SignalRequest {
    /// Namespace of the execution.
    pub namespace: String,
    /// Tenant of the execution.
    pub tenant: String,
    /// Optional signal payload. Becomes the wait step's response body.
    #[serde(default)]
    #[schema(value_type = Option<Object>)]
    pub payload: Option<serde_json::Value>,
}

/// Response for a delivered signal.
#[derive(Debug, Serialize, ToSchema)]
pub struct SignalResponse {
    /// Execution the signal was delivered to.
    pub execution_id: String,
    /// Name of the delivered signal.
    pub signal_name: String,
    /// Always `"delivered"`.
    pub status: String,
}

/// Request body for upserting search attributes.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpsertAttributesRequest {
    /// Namespace of the execution.
    pub namespace: String,
    /// Tenant of the execution.
    pub tenant: String,
    /// Attributes to merge into the execution (existing keys overwritten).
    #[schema(value_type = Object)]
    pub attributes: HashMap<String, serde_json::Value>,
}

fn summarize(state: &acteon_core::ChainState) -> ExecutionSummary {
    ExecutionSummary {
        execution_id: state.chain_id.clone(),
        chain_name: state.chain_name.clone(),
        version: state.chain_version,
        status: status_to_string(&state.status),
        current_step: state.current_step,
        total_steps: state.total_steps,
        started_at: state.started_at,
        updated_at: state.updated_at,
        search_attributes: state.search_attributes.clone(),
        wait_state: state
            .wait_state
            .as_ref()
            .and_then(|w| serde_json::to_value(w).ok()),
        parent_execution_id: state.parent_chain_id.clone(),
    }
}

/// `GET /v1/executions` -- list executions across chains.
#[utoipa::path(
    get,
    path = "/v1/executions",
    tag = "Executions",
    summary = "List executions",
    description = "Returns executions (including terminal ones) filtered by chain name, \
                   status, start-time window, and search attributes.",
    params(ExecutionsQueryParams),
    responses(
        (status = 200, description = "Execution list", body = ListExecutionsResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
    )
)]
pub async fn list_executions(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Query(params): Query<ExecutionsQueryParams>,
) -> impl IntoResponse {
    if !identity.can_manage_scope(&params.tenant, &params.namespace) {
        return tenant_forbidden(&params.namespace, &params.tenant);
    }

    let mut attributes = Vec::new();
    if let Some(ref attr) = params.attr {
        match attr.split_once('=') {
            Some((k, v)) if !k.is_empty() => attributes.push((k.to_owned(), v.to_owned())),
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: format!("invalid attr filter (expected key=value): {attr}"),
                    }),
                )
                    .into_response();
            }
        }
    }

    let filter = ExecutionFilter {
        chain_name: params.chain_name.clone(),
        status: params.status.as_deref().and_then(parse_status_filter),
        started_after: params.started_after,
        started_before: params.started_before,
        attributes,
        limit: params.limit,
    };

    let gw = state.gateway.read().await;
    match gw
        .list_executions(&params.namespace, &params.tenant, &filter)
        .await
    {
        Ok(executions) => (
            StatusCode::OK,
            Json(ListExecutionsResponse {
                executions: executions.iter().map(summarize).collect(),
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

/// `GET /v1/executions/{execution_id}` -- get one execution summary.
#[utoipa::path(
    get,
    path = "/v1/executions/{execution_id}",
    tag = "Executions",
    summary = "Get execution",
    description = "Returns the visibility summary for one execution, including its \
                   pinned definition version, search attributes, and current wait state.",
    params(
        ("execution_id" = String, Path, description = "Execution ID"),
        ExecutionNamespaceParams,
    ),
    responses(
        (status = 200, description = "Execution summary", body = ExecutionSummary),
        (status = 404, description = "Execution not found", body = ErrorResponse),
    )
)]
pub async fn get_execution(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(execution_id): Path<String>,
    Query(params): Query<ExecutionNamespaceParams>,
) -> impl IntoResponse {
    if !identity.can_manage_scope(&params.tenant, &params.namespace) {
        return tenant_forbidden(&params.namespace, &params.tenant);
    }
    let gw = state.gateway.read().await;
    match gw
        .get_chain_status(&params.namespace, &params.tenant, &execution_id)
        .await
    {
        Ok(Some(chain_state)) => (StatusCode::OK, Json(summarize(&chain_state))).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("execution not found: {execution_id}"),
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

/// `GET /v1/executions/{execution_id}/history` -- get the event history.
#[utoipa::path(
    get,
    path = "/v1/executions/{execution_id}/history",
    tag = "Executions",
    summary = "Get execution event history",
    description = "Returns the ordered, append-only event log for an execution: start, \
                   step transitions, timers, signals, and the terminal outcome.",
    params(
        ("execution_id" = String, Path, description = "Execution ID"),
        ExecutionNamespaceParams,
    ),
    responses(
        (status = 200, description = "Event history", body = ExecutionHistoryResponse),
        (status = 404, description = "Execution not found", body = ErrorResponse),
    )
)]
pub async fn get_execution_history(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(execution_id): Path<String>,
    Query(params): Query<ExecutionNamespaceParams>,
) -> impl IntoResponse {
    if !identity.can_manage_scope(&params.tenant, &params.namespace) {
        return tenant_forbidden(&params.namespace, &params.tenant);
    }
    let gw = state.gateway.read().await;
    match gw
        .get_execution_history(&params.namespace, &params.tenant, &execution_id)
        .await
    {
        Ok(history) => {
            if history.events.is_empty() {
                // Distinguish "no history" from "unknown execution".
                match gw
                    .get_chain_status(&params.namespace, &params.tenant, &execution_id)
                    .await
                {
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        return (
                            StatusCode::NOT_FOUND,
                            Json(ErrorResponse {
                                error: format!("execution not found: {execution_id}"),
                            }),
                        )
                            .into_response();
                    }
                    Err(e) => {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(ErrorResponse {
                                error: e.to_string(),
                            }),
                        )
                            .into_response();
                    }
                }
            }
            let events = history
                .events
                .iter()
                .filter_map(|e| serde_json::to_value(e).ok())
                .collect();
            (
                StatusCode::OK,
                Json(ExecutionHistoryResponse {
                    execution_id,
                    events,
                }),
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

/// `POST /v1/executions/{execution_id}/signal/{signal_name}` -- deliver a signal.
#[utoipa::path(
    post,
    path = "/v1/executions/{execution_id}/signal/{signal_name}",
    tag = "Executions",
    summary = "Signal an execution",
    description = "Delivers an external signal to a running execution. If the execution \
                   is paused on a matching wait_for_signal step it resumes immediately; \
                   otherwise the signal is buffered and consumed when the wait step is \
                   reached.",
    params(
        ("execution_id" = String, Path, description = "Execution ID"),
        ("signal_name" = String, Path, description = "Signal name"),
    ),
    request_body = SignalRequest,
    responses(
        (status = 200, description = "Signal delivered", body = SignalResponse),
        (status = 404, description = "Execution not found", body = ErrorResponse),
        (status = 409, description = "Execution is not active", body = ErrorResponse),
    )
)]
pub async fn signal_execution(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((execution_id, signal_name)): Path<(String, String)>,
    Json(req): Json<SignalRequest>,
) -> impl IntoResponse {
    if !identity.can_manage_scope(&req.tenant, &req.namespace) {
        return tenant_forbidden(&req.namespace, &req.tenant);
    }
    let gw = state.gateway.read().await;
    match gw
        .signal_chain(
            &req.namespace,
            &req.tenant,
            &execution_id,
            &signal_name,
            req.payload.unwrap_or(serde_json::Value::Null),
        )
        .await
    {
        Ok(()) => (
            StatusCode::OK,
            Json(SignalResponse {
                execution_id,
                signal_name,
                status: "delivered".to_owned(),
            }),
        )
            .into_response(),
        Err(e) => {
            let msg = e.to_string();
            let code = if msg.contains("not found") {
                StatusCode::NOT_FOUND
            } else if msg.contains("not active") {
                StatusCode::CONFLICT
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (code, Json(ErrorResponse { error: msg })).into_response()
        }
    }
}

/// `PUT /v1/executions/{execution_id}/attributes` -- upsert search attributes.
#[utoipa::path(
    put,
    path = "/v1/executions/{execution_id}/attributes",
    tag = "Executions",
    summary = "Upsert execution search attributes",
    description = "Merges the supplied attributes into the execution's search attributes \
                   (existing keys are overwritten). Attributes are queryable via \
                   `GET /v1/executions?attr=key=value`.",
    params(("execution_id" = String, Path, description = "Execution ID")),
    request_body = UpsertAttributesRequest,
    responses(
        (status = 200, description = "Updated execution", body = ExecutionSummary),
        (status = 404, description = "Execution not found", body = ErrorResponse),
    )
)]
pub async fn upsert_execution_attributes(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(execution_id): Path<String>,
    Json(req): Json<UpsertAttributesRequest>,
) -> impl IntoResponse {
    if !identity.can_manage_scope(&req.tenant, &req.namespace) {
        return tenant_forbidden(&req.namespace, &req.tenant);
    }
    let gw = state.gateway.read().await;
    match gw
        .upsert_search_attributes(&req.namespace, &req.tenant, &execution_id, req.attributes)
        .await
    {
        Ok(chain_state) => (StatusCode::OK, Json(summarize(&chain_state))).into_response(),
        Err(e) => {
            let msg = e.to_string();
            let code = if msg.contains("not found") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (code, Json(ErrorResponse { error: msg })).into_response()
        }
    }
}
