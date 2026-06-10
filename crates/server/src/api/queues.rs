//! Worker task queue API: enqueue, poll/lease, heartbeat, complete, fail.
//!
//! External workers drive this API: they poll a named queue for tasks,
//! execute them, and report results. Chain `worker` steps and workflow
//! continuation tasks flow through the same queues.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use acteon_core::{WorkerTask, WorkerTaskStatus};

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

fn queue_error_response(e: &acteon_gateway::GatewayError) -> axum::response::Response {
    let msg = e.to_string();
    let code = if msg.contains("not found") {
        StatusCode::NOT_FOUND
    } else if msg.contains("not leased")
        || msg.contains("lease token mismatch")
        || msg.contains("not active")
    {
        StatusCode::CONFLICT
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (code, Json(ErrorResponse { error: msg })).into_response()
}

fn status_to_string(status: WorkerTaskStatus) -> &'static str {
    match status {
        WorkerTaskStatus::Pending => "pending",
        WorkerTaskStatus::Leased => "leased",
        WorkerTaskStatus::Completed => "completed",
        WorkerTaskStatus::Failed => "failed",
        WorkerTaskStatus::Cancelled => "cancelled",
    }
}

fn parse_status(s: &str) -> Option<WorkerTaskStatus> {
    match s {
        "pending" => Some(WorkerTaskStatus::Pending),
        "leased" => Some(WorkerTaskStatus::Leased),
        "completed" => Some(WorkerTaskStatus::Completed),
        "failed" => Some(WorkerTaskStatus::Failed),
        "cancelled" => Some(WorkerTaskStatus::Cancelled),
        _ => None,
    }
}

/// Wire representation of a worker task.
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkerTaskDto {
    /// Unique task ID.
    pub task_id: String,
    /// Queue the task is routed through.
    pub queue: String,
    /// Action type for worker handler dispatch.
    pub action_type: String,
    /// Task payload.
    #[schema(value_type = Object)]
    pub payload: serde_json::Value,
    /// Lifecycle status.
    pub status: String,
    /// Delivery attempt (1-based once leased).
    pub attempt: u32,
    /// Maximum delivery attempts.
    pub max_attempts: u32,
    /// Lease token (present in poll responses; required for heartbeat /
    /// complete / fail).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_token: Option<String>,
    /// When the current lease expires.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_expires_at: Option<DateTime<Utc>>,
    /// Result reported by the worker.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub result: Option<serde_json::Value>,
    /// Error reported on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Owning chain execution, for chain `worker` steps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<String>,
    /// Owning workflow execution, for workflow continuation tasks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_execution_id: Option<String>,
    /// When the task was enqueued.
    pub created_at: DateTime<Utc>,
    /// When the task was last updated.
    pub updated_at: DateTime<Utc>,
}

impl From<&WorkerTask> for WorkerTaskDto {
    fn from(task: &WorkerTask) -> Self {
        Self {
            task_id: task.task_id.clone(),
            queue: task.queue.clone(),
            action_type: task.action_type.clone(),
            payload: task.payload.clone(),
            status: status_to_string(task.status).to_owned(),
            attempt: task.attempt,
            max_attempts: task.max_attempts,
            lease_token: task.lease_token.clone(),
            lease_expires_at: task.lease_expires_at,
            result: task.result.clone(),
            error: task.error.clone(),
            chain_id: task.chain_id.clone(),
            workflow_execution_id: task.workflow_execution_id.clone(),
            created_at: task.created_at,
            updated_at: task.updated_at,
        }
    }
}

/// Request body for enqueueing a task.
#[derive(Debug, Deserialize, ToSchema)]
pub struct EnqueueTaskRequest {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Action type for worker handler dispatch.
    pub action_type: String,
    /// Task payload delivered to the worker.
    #[schema(value_type = Object)]
    pub payload: serde_json::Value,
    /// Maximum delivery attempts (default 3).
    #[serde(default)]
    pub max_attempts: Option<u32>,
}

/// Request body for polling a queue.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PollQueueRequest {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Maximum tasks to lease in one poll (default 1).
    #[serde(default)]
    pub max_tasks: Option<usize>,
    /// Lease duration in seconds (default 60, max 3600).
    #[serde(default)]
    pub lease_seconds: Option<u64>,
    /// Identifier of the polling worker (for observability).
    #[serde(default)]
    pub worker_id: Option<String>,
}

/// Response for a queue poll.
#[derive(Debug, Serialize, ToSchema)]
pub struct PollQueueResponse {
    /// Leased tasks (empty when the queue has no leasable tasks).
    pub tasks: Vec<WorkerTaskDto>,
}

/// Request body for extending a task lease.
#[derive(Debug, Deserialize, ToSchema)]
pub struct HeartbeatRequest {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Lease token returned by poll.
    pub lease_token: String,
    /// New lease duration in seconds from now (default 60).
    #[serde(default)]
    pub extend_seconds: Option<u64>,
}

/// Request body for completing a task.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CompleteTaskRequest {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Lease token returned by poll.
    pub lease_token: String,
    /// Task result. For chain worker steps this becomes the step's
    /// response body; for workflow tasks it carries the directive.
    #[serde(default)]
    #[schema(value_type = Object)]
    pub result: serde_json::Value,
}

/// Request body for failing a task.
#[derive(Debug, Deserialize, ToSchema)]
pub struct FailTaskRequest {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Lease token returned by poll.
    pub lease_token: String,
    /// Error message.
    pub error: String,
    /// Whether the failure is retryable (default true). Retryable failures
    /// within the attempt budget re-queue the task with backoff.
    #[serde(default = "default_true")]
    pub retryable: bool,
}

fn default_true() -> bool {
    true
}

/// Namespace/tenant (and optional status) query for task reads.
#[derive(Debug, Deserialize, IntoParams)]
pub struct TaskQueryParams {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Optional status filter (`pending`, `leased`, `completed`, `failed`,
    /// `cancelled`).
    pub status: Option<String>,
}

/// `POST /v1/queues/{queue}/tasks` -- enqueue a task.
#[utoipa::path(
    post,
    path = "/v1/queues/{queue}/tasks",
    tag = "Task Queues",
    summary = "Enqueue a worker task",
    params(("queue" = String, Path, description = "Queue name")),
    request_body = EnqueueTaskRequest,
    responses(
        (status = 201, description = "Task enqueued", body = WorkerTaskDto),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
    )
)]
pub async fn enqueue_task(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(queue): Path<String>,
    Json(req): Json<EnqueueTaskRequest>,
) -> impl IntoResponse {
    if !identity.can_manage_scope(&req.tenant, &req.namespace) {
        return tenant_forbidden(&req.namespace, &req.tenant);
    }
    let task = WorkerTask::new(
        req.namespace.as_str(),
        req.tenant.as_str(),
        &queue,
        &req.action_type,
        req.payload,
    )
    .with_max_attempts(
        req.max_attempts
            .unwrap_or(acteon_core::DEFAULT_TASK_MAX_ATTEMPTS),
    );

    let gw = state.gateway.read().await;
    match gw.enqueue_worker_task(task).await {
        Ok(task) => (StatusCode::CREATED, Json(WorkerTaskDto::from(&task))).into_response(),
        Err(e) => queue_error_response(&e),
    }
}

/// `POST /v1/queues/{queue}/poll` -- lease pending tasks.
#[utoipa::path(
    post,
    path = "/v1/queues/{queue}/poll",
    tag = "Task Queues",
    summary = "Poll a queue for tasks",
    description = "Leases up to `max_tasks` pending tasks for the calling worker. \
                   Expired leases on the queue are reclaimed first. Returned tasks \
                   carry a `lease_token` required for heartbeat/complete/fail.",
    params(("queue" = String, Path, description = "Queue name")),
    request_body = PollQueueRequest,
    responses(
        (status = 200, description = "Leased tasks", body = PollQueueResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
    )
)]
pub async fn poll_queue(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(queue): Path<String>,
    Json(req): Json<PollQueueRequest>,
) -> impl IntoResponse {
    if !identity.can_manage_scope(&req.tenant, &req.namespace) {
        return tenant_forbidden(&req.namespace, &req.tenant);
    }
    let gw = state.gateway.read().await;
    match gw
        .poll_worker_tasks(
            &req.namespace,
            &req.tenant,
            &queue,
            req.max_tasks.unwrap_or(1),
            req.lease_seconds,
            req.worker_id.as_deref(),
        )
        .await
    {
        Ok(tasks) => (
            StatusCode::OK,
            Json(PollQueueResponse {
                tasks: tasks.iter().map(WorkerTaskDto::from).collect(),
            }),
        )
            .into_response(),
        Err(e) => queue_error_response(&e),
    }
}

/// `POST /v1/queues/tasks/{task_id}/heartbeat` -- extend a lease.
#[utoipa::path(
    post,
    path = "/v1/queues/tasks/{task_id}/heartbeat",
    tag = "Task Queues",
    summary = "Heartbeat a leased task",
    params(("task_id" = String, Path, description = "Task ID")),
    request_body = HeartbeatRequest,
    responses(
        (status = 200, description = "Lease extended", body = WorkerTaskDto),
        (status = 404, description = "Task not found", body = ErrorResponse),
        (status = 409, description = "Lease token mismatch", body = ErrorResponse),
    )
)]
pub async fn heartbeat_task(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(task_id): Path<String>,
    Json(req): Json<HeartbeatRequest>,
) -> impl IntoResponse {
    if !identity.can_manage_scope(&req.tenant, &req.namespace) {
        return tenant_forbidden(&req.namespace, &req.tenant);
    }
    let gw = state.gateway.read().await;
    match gw
        .heartbeat_worker_task(
            &req.namespace,
            &req.tenant,
            &task_id,
            &req.lease_token,
            req.extend_seconds,
        )
        .await
    {
        Ok(task) => (StatusCode::OK, Json(WorkerTaskDto::from(&task))).into_response(),
        Err(e) => queue_error_response(&e),
    }
}

/// `POST /v1/queues/tasks/{task_id}/complete` -- complete a task.
#[utoipa::path(
    post,
    path = "/v1/queues/tasks/{task_id}/complete",
    tag = "Task Queues",
    summary = "Complete a leased task",
    description = "Marks the task completed with a result. Resumes the owning \
                   chain or workflow execution, if any.",
    params(("task_id" = String, Path, description = "Task ID")),
    request_body = CompleteTaskRequest,
    responses(
        (status = 200, description = "Task completed", body = WorkerTaskDto),
        (status = 404, description = "Task not found", body = ErrorResponse),
        (status = 409, description = "Lease token mismatch", body = ErrorResponse),
    )
)]
pub async fn complete_task(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(task_id): Path<String>,
    Json(req): Json<CompleteTaskRequest>,
) -> impl IntoResponse {
    if !identity.can_manage_scope(&req.tenant, &req.namespace) {
        return tenant_forbidden(&req.namespace, &req.tenant);
    }
    let gw = state.gateway.read().await;
    match gw
        .complete_worker_task(
            &req.namespace,
            &req.tenant,
            &task_id,
            &req.lease_token,
            req.result,
        )
        .await
    {
        Ok(task) => (StatusCode::OK, Json(WorkerTaskDto::from(&task))).into_response(),
        Err(e) => queue_error_response(&e),
    }
}

/// `POST /v1/queues/tasks/{task_id}/fail` -- fail a task.
#[utoipa::path(
    post,
    path = "/v1/queues/tasks/{task_id}/fail",
    tag = "Task Queues",
    summary = "Fail a leased task",
    description = "Reports a task failure. Retryable failures within the attempt \
                   budget re-queue the task with backoff; otherwise the task fails \
                   terminally, goes to the DLQ, and fails the owning chain step.",
    params(("task_id" = String, Path, description = "Task ID")),
    request_body = FailTaskRequest,
    responses(
        (status = 200, description = "Failure recorded", body = WorkerTaskDto),
        (status = 404, description = "Task not found", body = ErrorResponse),
        (status = 409, description = "Lease token mismatch", body = ErrorResponse),
    )
)]
pub async fn fail_task(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(task_id): Path<String>,
    Json(req): Json<FailTaskRequest>,
) -> impl IntoResponse {
    if !identity.can_manage_scope(&req.tenant, &req.namespace) {
        return tenant_forbidden(&req.namespace, &req.tenant);
    }
    let gw = state.gateway.read().await;
    match gw
        .fail_worker_task(
            &req.namespace,
            &req.tenant,
            &task_id,
            &req.lease_token,
            &req.error,
            req.retryable,
        )
        .await
    {
        Ok(task) => (StatusCode::OK, Json(WorkerTaskDto::from(&task))).into_response(),
        Err(e) => queue_error_response(&e),
    }
}

/// `GET /v1/queues/tasks/{task_id}` -- get a task.
#[utoipa::path(
    get,
    path = "/v1/queues/tasks/{task_id}",
    tag = "Task Queues",
    summary = "Get a worker task",
    params(
        ("task_id" = String, Path, description = "Task ID"),
        TaskQueryParams,
    ),
    responses(
        (status = 200, description = "Task", body = WorkerTaskDto),
        (status = 404, description = "Task not found", body = ErrorResponse),
    )
)]
pub async fn get_task(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(task_id): Path<String>,
    Query(params): Query<TaskQueryParams>,
) -> impl IntoResponse {
    if !identity.can_manage_scope(&params.tenant, &params.namespace) {
        return tenant_forbidden(&params.namespace, &params.tenant);
    }
    let gw = state.gateway.read().await;
    match gw
        .get_worker_task(&params.namespace, &params.tenant, &task_id)
        .await
    {
        Ok(Some(task)) => (StatusCode::OK, Json(WorkerTaskDto::from(&task))).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("task not found: {task_id}"),
            }),
        )
            .into_response(),
        Err(e) => queue_error_response(&e),
    }
}

/// `GET /v1/queues/{queue}/tasks` -- list tasks on a queue.
#[utoipa::path(
    get,
    path = "/v1/queues/{queue}/tasks",
    tag = "Task Queues",
    summary = "List tasks on a queue",
    params(
        ("queue" = String, Path, description = "Queue name"),
        TaskQueryParams,
    ),
    responses(
        (status = 200, description = "Tasks", body = PollQueueResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
    )
)]
pub async fn list_tasks(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(queue): Path<String>,
    Query(params): Query<TaskQueryParams>,
) -> impl IntoResponse {
    if !identity.can_manage_scope(&params.tenant, &params.namespace) {
        return tenant_forbidden(&params.namespace, &params.tenant);
    }
    let status = params.status.as_deref().and_then(parse_status);
    let gw = state.gateway.read().await;
    match gw
        .list_worker_tasks(&params.namespace, &params.tenant, Some(&queue), status)
        .await
    {
        Ok(tasks) => (
            StatusCode::OK,
            Json(PollQueueResponse {
                tasks: tasks.iter().map(WorkerTaskDto::from).collect(),
            }),
        )
            .into_response(),
        Err(e) => queue_error_response(&e),
    }
}
