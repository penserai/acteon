use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use acteon_core::{ChainStatus, DagResponse};

use super::AppState;
use super::schemas::ErrorResponse;

/// Query parameters for listing chain executions.
#[derive(Debug, Deserialize, IntoParams)]
pub struct ChainQueryParams {
    /// Namespace to filter by.
    pub namespace: String,
    /// Tenant to filter by.
    pub tenant: String,
    /// Optional status filter: `"running"`, `"completed"`, `"failed"`, `"cancelled"`, `"timed_out"`, `"waiting_sub_chain"`.
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
    /// Parent chain ID if this chain was spawned as a sub-chain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_chain_id: Option<String>,
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
    /// Step status: `"pending"`, `"running"`, `"completed"`, `"failed"`, `"skipped"`,
    /// `"waiting_sub_chain"`, `"waiting_parallel"`. Parallel sub-steps may also report
    /// `"cancelled"`.
    pub status: String,
    /// Response body from the provider (if completed).
    #[schema(value_type = Option<Object>)]
    pub response_body: Option<serde_json::Value>,
    /// Error message (if failed).
    pub error: Option<String>,
    /// When this step completed.
    pub completed_at: Option<DateTime<Utc>>,
    /// Sub-chain name if this step invokes a sub-chain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_chain: Option<String>,
    /// Running child chain execution ID (if this sub-chain step has spawned a child).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub child_chain_id: Option<String>,
    /// Results from parallel sub-steps, if this is a parallel step.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Vec<Object>>)]
    pub parallel_sub_steps: Option<Vec<ChainStepStatus>>,
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
    /// The ordered list of step names that were executed (the branch path taken).
    /// Empty for chains that haven't started or for legacy chains without path tracking.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub execution_path: Vec<String>,
    /// Parent chain ID if this chain was spawned as a sub-chain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_chain_id: Option<String>,
    /// IDs of child chains spawned by sub-chain steps in this chain.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub child_chain_ids: Vec<String>,
}

fn parse_status_filter(s: &str) -> Option<ChainStatus> {
    match s {
        "running" => Some(ChainStatus::Running),
        "completed" => Some(ChainStatus::Completed),
        "failed" => Some(ChainStatus::Failed),
        "cancelled" => Some(ChainStatus::Cancelled),
        "timed_out" => Some(ChainStatus::TimedOut),
        "waiting_sub_chain" => Some(ChainStatus::WaitingSubChain),
        "waiting_parallel" => Some(ChainStatus::WaitingParallel),
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
        ChainStatus::WaitingSubChain => "waiting_sub_chain".into(),
        ChainStatus::WaitingParallel => "waiting_parallel".into(),
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
                    parent_chain_id: c.parent_chain_id.clone(),
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
#[allow(clippy::too_many_lines)]
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
                    // Build parallel sub-step statuses. When a parallel group
                    // is in-flight (`parallel_state` present), include Pending
                    // sub-steps alongside any already-completed results so the
                    // UI can show a proper loading state for every sub-step.
                    let parallel_sub_steps = if let Some(ps) = chain_state
                        .parallel_state
                        .as_ref()
                        .filter(|ps| ps.step_index == i)
                    {
                        let subs: Vec<ChainStepStatus> = ps
                            .sub_steps
                            .iter()
                            .map(|(name, sub_status)| {
                                if let Some(sr) = chain_state.parallel_sub_results.get(name) {
                                    ChainStepStatus {
                                        name: name.clone(),
                                        provider: String::new(),
                                        status: if sr.success {
                                            "completed".to_string()
                                        } else {
                                            "failed".to_string()
                                        },
                                        response_body: sr.response_body.clone(),
                                        error: sr.error.clone(),
                                        completed_at: Some(sr.completed_at),
                                        sub_chain: None,
                                        child_chain_id: None,
                                        parallel_sub_steps: None,
                                    }
                                } else {
                                    ChainStepStatus {
                                        name: name.clone(),
                                        provider: String::new(),
                                        status: format!("{sub_status:?}").to_lowercase(),
                                        response_body: None,
                                        error: None,
                                        completed_at: None,
                                        sub_chain: None,
                                        child_chain_id: None,
                                        parallel_sub_steps: None,
                                    }
                                }
                            })
                            .collect();
                        if subs.is_empty() { None } else { Some(subs) }
                    } else if !chain_state.parallel_sub_results.is_empty()
                        && chain_state
                            .step_results
                            .get(i)
                            .and_then(|r| r.as_ref())
                            .is_some()
                    {
                        // Parallel group already finished: show completed results.
                        let subs: Vec<ChainStepStatus> = chain_state
                            .parallel_sub_results
                            .iter()
                            .map(|(name, sr)| ChainStepStatus {
                                name: name.clone(),
                                provider: String::new(),
                                status: if sr.success {
                                    "completed".to_string()
                                } else {
                                    "failed".to_string()
                                },
                                response_body: sr.response_body.clone(),
                                error: sr.error.clone(),
                                completed_at: Some(sr.completed_at),
                                sub_chain: None,
                                child_chain_id: None,
                                parallel_sub_steps: None,
                            })
                            .collect();
                        if subs.is_empty() { None } else { Some(subs) }
                    } else {
                        None
                    };

                    ChainStepStatus {
                        name: step_name,
                        provider: String::new(),
                        status,
                        response_body: resp_body,
                        error,
                        completed_at: completed,
                        sub_chain: None,
                        child_chain_id: None,
                        parallel_sub_steps,
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
                execution_path: chain_state.execution_path,
                parent_chain_id: chain_state.parent_chain_id,
                child_chain_ids: chain_state.child_chain_ids,
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
                execution_path: chain_state.execution_path,
                parent_chain_id: chain_state.parent_chain_id,
                child_chain_ids: chain_state.child_chain_ids,
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

/// `GET /v1/chains/{chain_id}/dag` -- get DAG visualization for a chain instance.
#[utoipa::path(
    get,
    path = "/v1/chains/{chain_id}/dag",
    tag = "Chains",
    summary = "Get chain DAG visualization",
    description = "Returns a DAG representation of a running or completed chain instance for visualization.",
    params(
        ("chain_id" = String, Path, description = "Chain execution ID"),
        ChainNamespaceParams,
    ),
    responses(
        (status = 200, description = "Chain DAG", body = Object),
        (status = 404, description = "Chain not found", body = ErrorResponse),
    )
)]
pub async fn get_chain_dag(
    State(state): State<AppState>,
    Path(chain_id): Path<String>,
    Query(params): Query<ChainNamespaceParams>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;

    // Load the chain state to find the chain name and build the DAG.
    match gw
        .get_chain_status(&params.namespace, &params.tenant, &chain_id)
        .await
    {
        Ok(Some(chain_state)) => {
            if let Ok(dag) = gw
                .build_chain_dag(&chain_state.chain_name, Some(&chain_state), 0)
                .await
            {
                (StatusCode::OK, Json(dag)).into_response()
            } else {
                // Fallback to basic DAG from state if gateway method fails.
                let dag = build_dag_from_state(&chain_state);
                (StatusCode::OK, Json(dag)).into_response()
            }
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

/// `GET /v1/chains/definitions/{name}/dag` -- get DAG visualization for a chain definition.
#[utoipa::path(
    get,
    path = "/v1/chains/definitions/{name}/dag",
    tag = "Chains",
    summary = "Get chain definition DAG",
    description = "Returns a config-only DAG representation of a chain definition for visualization (no running instance).",
    params(
        ("name" = String, Path, description = "Chain definition name"),
    ),
    responses(
        (status = 200, description = "Chain definition DAG", body = Object),
        (status = 404, description = "Chain definition not found", body = ErrorResponse),
    )
)]
pub async fn get_chain_definition_dag(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;

    match gw.build_chain_dag(&name, None, 0).await {
        Ok(dag) => (StatusCode::OK, Json(dag)).into_response(),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") {
                (
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse {
                        error: format!("chain definition not found: {name}"),
                    }),
                )
                    .into_response()
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

/// Build a basic DAG from chain runtime state.
fn build_dag_from_state(state: &acteon_core::ChainState) -> DagResponse {
    use acteon_core::{DagEdge, DagNode};

    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    for i in 0..state.total_steps {
        let result = state.step_results.get(i).and_then(|r| r.as_ref());
        let step_name = result.map_or_else(|| format!("step-{i}"), |r| r.step_name.clone());

        let status = if let Some(r) = result {
            Some(if r.success {
                "completed".to_string()
            } else {
                "failed".to_string()
            })
        } else if i == state.current_step
            && state.status == acteon_core::ChainStatus::WaitingSubChain
        {
            Some("waiting_sub_chain".to_string())
        } else if i == state.current_step
            && state.status == acteon_core::ChainStatus::WaitingParallel
        {
            Some("waiting_parallel".to_string())
        } else if i == state.current_step && state.status == acteon_core::ChainStatus::Running {
            Some("running".to_string())
        } else {
            Some("pending".to_string())
        };

        let on_path = state.execution_path.contains(&step_name);

        nodes.push(DagNode {
            name: step_name.clone(),
            node_type: "step".into(),
            provider: None,
            action_type: None,
            sub_chain_name: None,
            status,
            child_chain_id: None,
            children: None,
            parallel_children: None,
            parallel_join: None,
        });

        // Add edge to next step.
        if i + 1 < state.total_steps {
            let next_result = state.step_results.get(i + 1).and_then(|r| r.as_ref());
            let next_name =
                next_result.map_or_else(|| format!("step-{}", i + 1), |r| r.step_name.clone());
            edges.push(DagEdge {
                source: step_name,
                target: next_name,
                label: None,
                on_execution_path: on_path,
            });
        }
    }

    DagResponse {
        chain_name: state.chain_name.clone(),
        chain_id: Some(state.chain_id.clone()),
        status: Some(status_to_string(&state.status)),
        nodes,
        edges,
        execution_path: state.execution_path.clone(),
    }
}
