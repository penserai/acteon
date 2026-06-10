//! Workflow execution API: start, inspect, signal, cancel, checkpoint, and
//! spawn child workflows. Workflow code runs on external workers via the
//! task queue API; this surface manages execution state.

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use acteon_core::{ParentClosePolicy, WorkflowExecution, WorkflowStatus};
use acteon_gateway::WorkflowFilter;

use super::AppState;
use super::schemas::ErrorResponse;
use crate::auth::identity::CallerIdentity;

fn tenant_forbidden(namespace: &str, tenant: &str) -> axum::response::Response {
    (
        StatusCode::FORBIDDEN,
        Json(ErrorResponse {
            error: format!("forbidden: no grant covers tenant={tenant} namespace={namespace}"),
        }),
    )
        .into_response()
}

fn workflow_error_response(e: &acteon_gateway::GatewayError) -> axum::response::Response {
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

fn status_to_string(status: WorkflowStatus) -> &'static str {
    match status {
        WorkflowStatus::Running => "running",
        WorkflowStatus::WaitingTimer => "waiting_timer",
        WorkflowStatus::WaitingSignal => "waiting_signal",
        WorkflowStatus::Completed => "completed",
        WorkflowStatus::Failed => "failed",
        WorkflowStatus::Cancelled => "cancelled",
    }
}

fn parse_status(s: &str) -> Option<WorkflowStatus> {
    match s {
        "running" => Some(WorkflowStatus::Running),
        "waiting_timer" => Some(WorkflowStatus::WaitingTimer),
        "waiting_signal" => Some(WorkflowStatus::WaitingSignal),
        "completed" => Some(WorkflowStatus::Completed),
        "failed" => Some(WorkflowStatus::Failed),
        "cancelled" => Some(WorkflowStatus::Cancelled),
        _ => None,
    }
}

/// Wire representation of a workflow execution.
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkflowExecutionDto {
    /// Unique execution ID.
    pub execution_id: String,
    /// Workflow name.
    pub workflow: String,
    /// Worker queue continuation tasks are routed through.
    pub queue: String,
    /// Lifecycle status.
    pub status: String,
    /// Input the execution started with.
    #[schema(value_type = Object)]
    pub input: serde_json::Value,
    /// Result (when completed).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub result: Option<serde_json::Value>,
    /// Error (when failed/cancelled).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Recorded checkpoints (name, seq, data).
    #[schema(value_type = Vec<Object>)]
    pub checkpoints: Vec<serde_json::Value>,
    /// What the execution is waiting on, when suspended.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub awaiting: Option<serde_json::Value>,
    /// Parent execution ID for child workflows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// IDs of child executions.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<String>,
    /// User-defined search attributes.
    #[schema(value_type = Object)]
    pub search_attributes: HashMap<String, serde_json::Value>,
    /// When the execution started.
    pub created_at: DateTime<Utc>,
    /// When the execution was last updated.
    pub updated_at: DateTime<Utc>,
}

impl From<&WorkflowExecution> for WorkflowExecutionDto {
    fn from(exec: &WorkflowExecution) -> Self {
        Self {
            execution_id: exec.execution_id.clone(),
            workflow: exec.workflow.clone(),
            queue: exec.queue.clone(),
            status: status_to_string(exec.status).to_owned(),
            input: exec.input.clone(),
            result: exec.result.clone(),
            error: exec.error.clone(),
            checkpoints: exec
                .checkpoints
                .iter()
                .filter_map(|c| serde_json::to_value(c).ok())
                .collect(),
            awaiting: exec
                .awaiting
                .as_ref()
                .and_then(|a| serde_json::to_value(a).ok()),
            parent_id: exec.parent_id.clone(),
            children: exec
                .children
                .iter()
                .map(|c| c.execution_id.clone())
                .collect(),
            search_attributes: exec.search_attributes.clone(),
            created_at: exec.created_at,
            updated_at: exec.updated_at,
        }
    }
}

/// Request body for starting a workflow execution.
#[derive(Debug, Deserialize, ToSchema)]
pub struct StartWorkflowRequest {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Workflow name (matched to a handler registered on workers).
    pub workflow: String,
    /// Worker queue to route continuation tasks through.
    pub queue: String,
    /// Workflow input.
    #[serde(default)]
    #[schema(value_type = Object)]
    pub input: serde_json::Value,
    /// Optional search attributes.
    #[serde(default)]
    #[schema(value_type = Object)]
    pub search_attributes: HashMap<String, serde_json::Value>,
}

/// Query parameters for listing workflow executions.
#[derive(Debug, Deserialize, IntoParams)]
pub struct WorkflowsQueryParams {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Optional workflow name filter.
    pub workflow: Option<String>,
    /// Optional status filter (`running`, `waiting_timer`, `waiting_signal`,
    /// `completed`, `failed`, `cancelled`).
    pub status: Option<String>,
    /// Maximum executions to return (default 200).
    pub limit: Option<usize>,
}

/// Namespace/tenant query for workflow detail endpoints.
#[derive(Debug, Deserialize, IntoParams)]
pub struct WorkflowNamespaceParams {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
}

/// Response for listing workflow executions.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListWorkflowsResponse {
    /// Matching executions, most recently started first.
    pub executions: Vec<WorkflowExecutionDto>,
}

/// Request body for signalling a workflow.
#[derive(Debug, Deserialize, ToSchema)]
pub struct WorkflowSignalRequest {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Optional signal payload (returned by the awaiting
    /// `ctx.wait_for_signal` call).
    #[serde(default)]
    #[schema(value_type = Option<Object>)]
    pub payload: Option<serde_json::Value>,
}

/// Request body for cancelling a workflow.
#[derive(Debug, Deserialize, ToSchema)]
pub struct WorkflowCancelRequest {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Optional reason.
    #[serde(default)]
    pub reason: Option<String>,
}

/// Request body for recording a checkpoint.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RecordCheckpointRequest {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Unique checkpoint name within the execution.
    pub name: String,
    /// Checkpoint payload, returned verbatim on replay.
    #[serde(default)]
    #[schema(value_type = Object)]
    pub data: serde_json::Value,
}

/// Response for a recorded checkpoint.
#[derive(Debug, Serialize, ToSchema)]
pub struct RecordCheckpointResponse {
    /// Checkpoint name.
    pub name: String,
    /// Sequence number within the execution.
    pub seq: u64,
    /// Recorded payload (the previously-recorded one on idempotent replays).
    #[schema(value_type = Object)]
    pub data: serde_json::Value,
}

/// Request body for starting a child workflow.
#[derive(Debug, Deserialize, ToSchema)]
pub struct StartChildRequest {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Idempotency checkpoint name recorded on the parent.
    pub checkpoint: String,
    /// Child workflow name.
    pub workflow: String,
    /// Worker queue for the child (defaults to the parent's queue).
    #[serde(default)]
    pub queue: Option<String>,
    /// Child input.
    #[serde(default)]
    #[schema(value_type = Object)]
    pub input: serde_json::Value,
    /// What happens to the child when the parent closes (`abandon` |
    /// `cancel`, default `abandon`).
    #[serde(default)]
    pub parent_close_policy: Option<String>,
}

/// Response for a started child workflow.
#[derive(Debug, Serialize, ToSchema)]
pub struct StartChildResponse {
    /// Child execution ID. Await its result with the signal
    /// `__child:{child_execution_id}`.
    pub child_execution_id: String,
}

/// `POST /v1/workflows/start` -- start a workflow execution.
#[utoipa::path(
    post,
    path = "/v1/workflows/start",
    tag = "Workflows",
    summary = "Start a workflow execution",
    description = "Creates a workflow execution and enqueues its first continuation \
                   task on the worker queue. Workflow code runs on external workers.",
    request_body = StartWorkflowRequest,
    responses(
        (status = 201, description = "Execution started", body = WorkflowExecutionDto),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
    )
)]
pub async fn start_workflow(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Json(req): Json<StartWorkflowRequest>,
) -> impl IntoResponse {
    if !identity.can_manage_scope(&req.tenant, &req.namespace) {
        return tenant_forbidden(&req.namespace, &req.tenant);
    }
    let gw = state.gateway.read().await;
    match gw
        .start_workflow(
            &req.namespace,
            &req.tenant,
            &req.workflow,
            &req.queue,
            req.input,
            req.search_attributes,
        )
        .await
    {
        Ok(exec) => (StatusCode::CREATED, Json(WorkflowExecutionDto::from(&exec))).into_response(),
        Err(e) => workflow_error_response(&e),
    }
}

/// `GET /v1/workflows/executions` -- list workflow executions.
#[utoipa::path(
    get,
    path = "/v1/workflows/executions",
    tag = "Workflows",
    summary = "List workflow executions",
    params(WorkflowsQueryParams),
    responses(
        (status = 200, description = "Executions", body = ListWorkflowsResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
    )
)]
pub async fn list_workflows(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Query(params): Query<WorkflowsQueryParams>,
) -> impl IntoResponse {
    if !identity.can_manage_scope(&params.tenant, &params.namespace) {
        return tenant_forbidden(&params.namespace, &params.tenant);
    }
    let filter = WorkflowFilter {
        workflow: params.workflow.clone(),
        status: params.status.as_deref().and_then(parse_status),
        limit: params.limit,
    };
    let gw = state.gateway.read().await;
    match gw
        .list_workflow_executions(&params.namespace, &params.tenant, &filter)
        .await
    {
        Ok(executions) => (
            StatusCode::OK,
            Json(ListWorkflowsResponse {
                executions: executions.iter().map(WorkflowExecutionDto::from).collect(),
            }),
        )
            .into_response(),
        Err(e) => workflow_error_response(&e),
    }
}

/// `GET /v1/workflows/executions/{execution_id}` -- get one execution.
#[utoipa::path(
    get,
    path = "/v1/workflows/executions/{execution_id}",
    tag = "Workflows",
    summary = "Get a workflow execution",
    params(
        ("execution_id" = String, Path, description = "Execution ID"),
        WorkflowNamespaceParams,
    ),
    responses(
        (status = 200, description = "Execution", body = WorkflowExecutionDto),
        (status = 404, description = "Execution not found", body = ErrorResponse),
    )
)]
pub async fn get_workflow(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(execution_id): Path<String>,
    Query(params): Query<WorkflowNamespaceParams>,
) -> impl IntoResponse {
    if !identity.can_manage_scope(&params.tenant, &params.namespace) {
        return tenant_forbidden(&params.namespace, &params.tenant);
    }
    let gw = state.gateway.read().await;
    match gw
        .get_workflow_execution(&params.namespace, &params.tenant, &execution_id)
        .await
    {
        Ok(Some(exec)) => (StatusCode::OK, Json(WorkflowExecutionDto::from(&exec))).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("workflow execution not found: {execution_id}"),
            }),
        )
            .into_response(),
        Err(e) => workflow_error_response(&e),
    }
}

/// `POST /v1/workflows/executions/{execution_id}/signal/{signal_name}` -- signal.
#[utoipa::path(
    post,
    path = "/v1/workflows/executions/{execution_id}/signal/{signal_name}",
    tag = "Workflows",
    summary = "Signal a workflow execution",
    description = "Delivers a signal. A workflow paused on a matching \
                   `wait_for_signal` resumes immediately; otherwise the signal is \
                   buffered for the next matching await.",
    params(
        ("execution_id" = String, Path, description = "Execution ID"),
        ("signal_name" = String, Path, description = "Signal name"),
    ),
    request_body = WorkflowSignalRequest,
    responses(
        (status = 200, description = "Signal delivered"),
        (status = 404, description = "Execution not found", body = ErrorResponse),
        (status = 409, description = "Execution not active", body = ErrorResponse),
    )
)]
pub async fn signal_workflow(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((execution_id, signal_name)): Path<(String, String)>,
    Json(req): Json<WorkflowSignalRequest>,
) -> impl IntoResponse {
    if !identity.can_manage_scope(&req.tenant, &req.namespace) {
        return tenant_forbidden(&req.namespace, &req.tenant);
    }
    let gw = state.gateway.read().await;
    match gw
        .signal_workflow(
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
            Json(serde_json::json!({
                "execution_id": execution_id,
                "signal_name": signal_name,
                "status": "delivered",
            })),
        )
            .into_response(),
        Err(e) => workflow_error_response(&e),
    }
}

/// `POST /v1/workflows/executions/{execution_id}/cancel` -- cancel.
#[utoipa::path(
    post,
    path = "/v1/workflows/executions/{execution_id}/cancel",
    tag = "Workflows",
    summary = "Cancel a workflow execution",
    params(("execution_id" = String, Path, description = "Execution ID")),
    request_body = WorkflowCancelRequest,
    responses(
        (status = 200, description = "Execution cancelled", body = WorkflowExecutionDto),
        (status = 404, description = "Execution not found", body = ErrorResponse),
        (status = 409, description = "Execution not active", body = ErrorResponse),
    )
)]
pub async fn cancel_workflow(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(execution_id): Path<String>,
    Json(req): Json<WorkflowCancelRequest>,
) -> impl IntoResponse {
    if !identity.can_manage_scope(&req.tenant, &req.namespace) {
        return tenant_forbidden(&req.namespace, &req.tenant);
    }
    let gw = state.gateway.read().await;
    match gw
        .cancel_workflow(&req.namespace, &req.tenant, &execution_id, req.reason)
        .await
    {
        Ok(exec) => (StatusCode::OK, Json(WorkflowExecutionDto::from(&exec))).into_response(),
        Err(e) => workflow_error_response(&e),
    }
}

/// `POST /v1/workflows/executions/{execution_id}/checkpoints` -- record a checkpoint.
#[utoipa::path(
    post,
    path = "/v1/workflows/executions/{execution_id}/checkpoints",
    tag = "Workflows",
    summary = "Record a workflow checkpoint",
    description = "Records the durable result of a completed step. Idempotent by \
                   checkpoint name: replays return the originally-recorded data.",
    params(("execution_id" = String, Path, description = "Execution ID")),
    request_body = RecordCheckpointRequest,
    responses(
        (status = 200, description = "Checkpoint recorded", body = RecordCheckpointResponse),
        (status = 404, description = "Execution not found", body = ErrorResponse),
        (status = 409, description = "Execution not active", body = ErrorResponse),
    )
)]
pub async fn record_checkpoint(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(execution_id): Path<String>,
    Json(req): Json<RecordCheckpointRequest>,
) -> impl IntoResponse {
    if !identity.can_manage_scope(&req.tenant, &req.namespace) {
        return tenant_forbidden(&req.namespace, &req.tenant);
    }
    let gw = state.gateway.read().await;
    match gw
        .record_workflow_checkpoint(
            &req.namespace,
            &req.tenant,
            &execution_id,
            &req.name,
            req.data,
        )
        .await
    {
        Ok(checkpoint) => (
            StatusCode::OK,
            Json(RecordCheckpointResponse {
                name: checkpoint.name,
                seq: checkpoint.seq,
                data: checkpoint.data,
            }),
        )
            .into_response(),
        Err(e) => workflow_error_response(&e),
    }
}

/// `POST /v1/workflows/executions/{execution_id}/children` -- start a child.
#[utoipa::path(
    post,
    path = "/v1/workflows/executions/{execution_id}/children",
    tag = "Workflows",
    summary = "Start a child workflow",
    description = "Starts a child workflow execution, idempotently keyed by the \
                   `checkpoint` name on the parent. The child's terminal result is \
                   delivered to the parent as the signal `__child:{child_id}`.",
    params(("execution_id" = String, Path, description = "Parent execution ID")),
    request_body = StartChildRequest,
    responses(
        (status = 201, description = "Child started", body = StartChildResponse),
        (status = 404, description = "Parent not found", body = ErrorResponse),
        (status = 409, description = "Parent not active", body = ErrorResponse),
    )
)]
pub async fn start_child_workflow(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(execution_id): Path<String>,
    Json(req): Json<StartChildRequest>,
) -> impl IntoResponse {
    if !identity.can_manage_scope(&req.tenant, &req.namespace) {
        return tenant_forbidden(&req.namespace, &req.tenant);
    }
    let policy = match req.parent_close_policy.as_deref() {
        None | Some("abandon") => ParentClosePolicy::Abandon,
        Some("cancel") => ParentClosePolicy::Cancel,
        Some(other) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!(
                        "invalid parent_close_policy: {other} (expected abandon|cancel)"
                    ),
                }),
            )
                .into_response();
        }
    };
    let gw = state.gateway.read().await;
    match gw
        .start_child_workflow(
            &req.namespace,
            &req.tenant,
            &execution_id,
            &req.checkpoint,
            &req.workflow,
            req.queue.as_deref(),
            req.input,
            policy,
        )
        .await
    {
        Ok(child_id) => (
            StatusCode::CREATED,
            Json(StartChildResponse {
                child_execution_id: child_id,
            }),
        )
            .into_response(),
        Err(e) => workflow_error_response(&e),
    }
}
