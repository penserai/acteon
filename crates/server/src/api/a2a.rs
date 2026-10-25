//! A2A protocol codecs — JSON-RPC 2.0 + REST binding (Phase 2).
//!
//! This is the external entry point for the A2A Task Engine
//! (`acteon-gateway`'s `task_engine`). Until this module, the engine
//! had no API surface — only the background stale-task reaper reached
//! it. Two transports share one set of method implementations:
//!
//! - **JSON-RPC 2.0** — `POST /a2a`. A2A's
//!   primary transport: one endpoint, the method named in the
//!   envelope. Single requests, batch arrays, and notifications
//!   (requests with no `id` — processed, but not answered) are all
//!   handled per the JSON-RPC 2.0 spec.
//! - **REST binding** (A2A spec §11) — `POST .../v1/message:send`,
//!   `GET .../v1/tasks/{id}`, `POST .../v1/tasks/{id}:cancel`. The
//!   final path segment is matched whole and the `:cancel` verb
//!   suffix split off in-handler, since axum's router matches whole
//!   segments — the URL stays spec-exact.
//!
//! Both transports use the exact context bound to the authenticated credential.
//! A2A itself has no notion of Acteon's multi-tenancy, so the tenant
//! is carried in the URL and authorized against the caller's grants,
//! mirroring the bus API. Request bodies are explicitly capped at
//! [`A2A_MAX_BODY_BYTES`].
//!
//! The official v1 JSON-RPC surface includes finite task methods,
//! `SendStreamingMessage`, `SubscribeToTask`, and push-notification
//! configuration. Streaming responses use SSE while preserving the JSON-RPC
//! request id in every frame.
//!
//! The `A2A-Version` header is negotiated up front — a request that
//! pins an unsupported version is rejected before any work is done.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use futures::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::convert::Infallible;
use std::time::Duration;
use tokio::sync::broadcast;

use acteon_core::{Agent, AgentCard, StreamEvent, StreamEventType, Task, TaskMessage, TaskState};
use acteon_gateway::{TaskEngine, TaskEngineError, TaskScope};
use acteon_state::{KeyKind, StateKey};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

use super::AppState;
use super::schemas::ErrorResponse;
use super::stream::{ConnectionGuard, StreamQuery, make_event_stream};
use crate::auth::identity::CallerIdentity;
use crate::auth::role::Permission;

/// A2A protocol version this server implements. Echoed on every
/// response as the `A2A-Version` header; an inbound `A2A-Version`
/// that names a different version is rejected.
pub const A2A_PROTOCOL_VERSION: &str = "1.0";

/// Header carrying the negotiated A2A protocol version. Compared
/// case-insensitively by `HeaderMap`.
const A2A_VERSION_HEADER: &str = "a2a-version";

/// Strong validator for one authoritative A2A task-row version. The value is
/// opaque to clients; only the target that issued it interprets it.
pub(crate) fn task_cursor(version: u64, history_length: Option<usize>) -> String {
    history_length.map_or_else(
        || format!("\"acteon-task-v1-{version}-full\""),
        |limit| format!("\"acteon-task-v1-{version}-h{limit}\""),
    )
}

/// Hard cap on an A2A request body. A JSON-RPC `message/send` carries
/// a [`TaskMessage`]; legitimate ones are well under this. The cap is
/// applied as an explicit per-route `DefaultBodyLimit` rather than
/// relying on axum's process-wide default, so it stays correct even
/// if that default is later tuned.
pub const A2A_MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

// ---------------------------------------------------------------------
// JSON-RPC 2.0 envelope
// ---------------------------------------------------------------------

/// Outbound JSON-RPC 2.0 response. Exactly one of `result` / `error`
/// is populated.
#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcErrorBody>,
    id: Value,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Serialize)]
struct JsonRpcErrorBody {
    code: i64,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

// Standard JSON-RPC 2.0 error codes.
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;
// A2A-specific error codes (A2A spec §8).
const TASK_NOT_FOUND: i64 = -32001;
const TASK_NOT_CANCELABLE: i64 = -32002;
const PUSH_NOTIFICATION_NOT_SUPPORTED: i64 = -32003;
const UNSUPPORTED_OPERATION: i64 = -32004;
const EXTENDED_AGENT_CARD_NOT_CONFIGURED: i64 = -32007;
// A2A 1.0 canonical version-negotiation error.
const VERSION_NOT_SUPPORTED: i64 = -32009;

// ---------------------------------------------------------------------
// A2A error model
// ---------------------------------------------------------------------

/// A method-level A2A failure. Carries a JSON-RPC error code; the same
/// value drives both the JSON-RPC error object and the REST HTTP
/// status, so the two transports report the same failure consistently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct A2aError {
    code: i64,
    message: String,
}

impl A2aError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn task_not_found(id: &str) -> Self {
        Self::new(TASK_NOT_FOUND, format!("task '{id}' not found"))
    }

    fn task_not_cancelable(id: &str) -> Self {
        Self::new(
            TASK_NOT_CANCELABLE,
            format!("task '{id}' is in a terminal state and cannot be canceled"),
        )
    }

    fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(INTERNAL_ERROR, message)
    }

    fn version_not_supported(got: &str, want: &str) -> Self {
        Self::new(
            VERSION_NOT_SUPPORTED,
            format!("A2A-Version '{got}' is not supported; this server speaks '{want}'"),
        )
    }

    /// HTTP status for the REST binding. JSON-RPC always answers 200
    /// with the error in the body; the REST binding maps the code to
    /// a status.
    fn http_status(&self) -> StatusCode {
        match self.code {
            TASK_NOT_FOUND | METHOD_NOT_FOUND => StatusCode::NOT_FOUND,
            TASK_NOT_CANCELABLE => StatusCode::CONFLICT,
            INVALID_PARAMS
            | INVALID_REQUEST
            | PARSE_ERROR
            | PUSH_NOTIFICATION_NOT_SUPPORTED
            | UNSUPPORTED_OPERATION
            | EXTENDED_AGENT_CARD_NOT_CONFIGURED
            | VERSION_NOT_SUPPORTED => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn to_jsonrpc(&self) -> JsonRpcErrorBody {
        let reason = match self.code {
            TASK_NOT_FOUND => Some("TASK_NOT_FOUND"),
            TASK_NOT_CANCELABLE => Some("TASK_NOT_CANCELABLE"),
            PUSH_NOTIFICATION_NOT_SUPPORTED => Some("PUSH_NOTIFICATION_NOT_SUPPORTED"),
            UNSUPPORTED_OPERATION => Some("UNSUPPORTED_OPERATION"),
            EXTENDED_AGENT_CARD_NOT_CONFIGURED => Some("EXTENDED_AGENT_CARD_NOT_CONFIGURED"),
            VERSION_NOT_SUPPORTED => Some("VERSION_NOT_SUPPORTED"),
            _ => None,
        };
        JsonRpcErrorBody {
            code: self.code,
            message: self.message.clone(),
            data: reason.map(|reason| {
                serde_json::json!([{
                    "@type": "type.googleapis.com/google.rpc.ErrorInfo",
                    "reason": reason,
                    "domain": "a2a-protocol.org",
                }])
            }),
        }
    }
}

impl From<TaskEngineError> for A2aError {
    fn from(e: TaskEngineError) -> Self {
        match e {
            TaskEngineError::NotFound(id) => A2aError::task_not_found(&id),
            TaskEngineError::AlreadyExists(id) => {
                A2aError::invalid_params(format!("task '{id}' already exists"))
            }
            // Client-attributable failures — the message names a task
            // id, a transition, or a structural limit the caller
            // controls, none of it server-internal.
            TaskEngineError::Validation(v) => A2aError::invalid_params(v.to_string()),
            TaskEngineError::PushConfigValidation(v) => A2aError::invalid_params(v.to_string()),
            TaskEngineError::ReferenceCycle { .. }
            | TaskEngineError::ReferenceDepthExceeded { .. }
            | TaskEngineError::ReferenceGraphTooLarge { .. }
            | TaskEngineError::InvalidPauseKind(_)
            | TaskEngineError::Approval(_)
            | TaskEngineError::ChallengeNotFound(_)
            | TaskEngineError::ChallengeMismatch { .. }
            | TaskEngineError::ChallengeKind { .. }
            | TaskEngineError::ChallengeClosed { .. }
            | TaskEngineError::ChallengeExpired(_)
            | TaskEngineError::ChallengeResponseConflict(_)
            | TaskEngineError::ChallengeResponseTaskMismatch
            | TaskEngineError::ChallengeResponseContextMismatch
            | TaskEngineError::ChallengeResponseRole
            | TaskEngineError::AuthorizationChallengeUnbound(_)
            | TaskEngineError::AuthorizationRequirementRequired
            | TaskEngineError::PushConfigIdentityMismatch
            | TaskEngineError::PushConfigAlreadyExists(_)
            | TaskEngineError::PushConfigCapacityExceeded
            | TaskEngineError::PushConfigNotFound(_)
            | TaskEngineError::PushConfigDeliveryInFlight
            | TaskEngineError::PushDeadLetterNotFound(_)
            | TaskEngineError::InvalidPushWorkerId
            | TaskEngineError::InvalidPushLeaseDuration
            | TaskEngineError::InvalidPushRetryTime
            | TaskEngineError::AuthorizationVerifierMismatch { .. }
            | TaskEngineError::AuthorizationEvidenceExpired
            | TaskEngineError::AuthorizationDecisionConflict(_)
            | TaskEngineError::AuthorizationVerification(
                acteon_gateway::TaskAuthorizationVerificationError::Denied,
            ) => A2aError::invalid_params(e.to_string()),
            // Contention is transient and server-side; the retry count
            // is not useful to the caller.
            TaskEngineError::CasExhausted(_) | TaskEngineError::ChallengeCasExhausted(_) => {
                A2aError::internal("the task is under contention; retry the request")
            }
            // State-store and serde failures can carry backend
            // internals — connection strings, SQL, file paths. Log the
            // detail server-side and return an opaque message so it
            // never reaches the caller (CWE-209).
            other @ (TaskEngineError::State(_)
            | TaskEngineError::Audit(_)
            | TaskEngineError::Serde(_)
            | TaskEngineError::UnsupportedRecordVersion(_)
            | TaskEngineError::InvalidDurableRecord(_)
            | TaskEngineError::DeliveryLogFull
            | TaskEngineError::PushDeliveryLeaseLost
            | TaskEngineError::PushDeliverySequenceExhausted
            | TaskEngineError::PushDeliveryAttemptExhausted
            | TaskEngineError::PushDeadLetterCapacityExceeded
            | TaskEngineError::ApprovalConflict(_)
            | TaskEngineError::AuthorizationVerification(
                acteon_gateway::TaskAuthorizationVerificationError::Unavailable
                | acteon_gateway::TaskAuthorizationVerificationError::Invalid(_),
            )) => {
                tracing::error!(error = %other, "a2a task-engine internal error");
                A2aError::internal("internal error")
            }
        }
    }
}

// ---------------------------------------------------------------------
// A2A method parameters
// ---------------------------------------------------------------------

/// `message/send` params — A2A `MessageSendParams`. Only `message` is
/// consumed in this phase; `configuration` / `metadata` are accepted
/// (and ignored) so spec-compliant clients are not rejected.
#[derive(Debug, Deserialize)]
pub struct MessageSendParams {
    message: TaskMessage,
}

/// `tasks/get` params — A2A `TaskQueryParams`.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct TaskQueryParams {
    id: String,
    /// Trim the returned `history` to its most recent N messages.
    #[serde(default)]
    history_length: Option<usize>,
}

/// `tasks/cancel` params — A2A `TaskIdParams`.
#[derive(Debug, Deserialize)]
struct TaskIdParams {
    id: String,
}

#[derive(Debug, Deserialize)]
struct V1SubscribeParams {
    #[serde(default)]
    tenant: Option<String>,
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListTasksParams {
    #[serde(default)]
    tenant: Option<String>,
    #[serde(default)]
    context_id: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    page_size: Option<usize>,
    #[serde(default)]
    page_token: Option<String>,
    #[serde(default)]
    history_length: Option<usize>,
    #[serde(default)]
    status_timestamp_after: Option<DateTime<Utc>>,
    #[serde(default)]
    include_artifacts: bool,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ListTasksCursor {
    last_id: String,
    last_updated_at: DateTime<Utc>,
    context_id: Option<String>,
    status: Option<String>,
    history_length: Option<usize>,
    status_timestamp_after: Option<DateTime<Utc>>,
    include_artifacts: bool,
}

fn parse_v1_task_state(value: &str) -> Result<Option<TaskState>, A2aError> {
    match value {
        "TASK_STATE_UNSPECIFIED" => Ok(None),
        "TASK_STATE_SUBMITTED" => Ok(Some(TaskState::Submitted)),
        "TASK_STATE_WORKING" => Ok(Some(TaskState::Working)),
        "TASK_STATE_COMPLETED" => Ok(Some(TaskState::Completed)),
        "TASK_STATE_FAILED" => Ok(Some(TaskState::Failed)),
        "TASK_STATE_CANCELED" => Ok(Some(TaskState::Canceled)),
        "TASK_STATE_INPUT_REQUIRED" => Ok(Some(TaskState::InputRequired)),
        "TASK_STATE_AUTH_REQUIRED" => Ok(Some(TaskState::AuthRequired)),
        "TASK_STATE_REJECTED" => Ok(Some(TaskState::Rejected)),
        _ => Err(A2aError::invalid_params(format!(
            "unsupported TaskState '{value}'"
        ))),
    }
}

fn decode_list_tasks_cursor(value: &str) -> Result<ListTasksCursor, A2aError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| A2aError::invalid_params("invalid pageToken"))?;
    serde_json::from_slice(&bytes).map_err(|_| A2aError::invalid_params("invalid pageToken"))
}

fn encode_list_tasks_cursor(cursor: &ListTasksCursor) -> Result<String, A2aError> {
    serde_json::to_vec(cursor)
        .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        .map_err(|_| A2aError::internal("internal error"))
}

#[allow(clippy::too_many_lines)] // Keep the ratified filter, cursor and projection contract together.
async fn method_list_tasks_v1(
    engine: &TaskEngine,
    scope: &TaskScope,
    params: ListTasksParams,
) -> Result<Value, A2aError> {
    let page_size = params.page_size.unwrap_or(50);
    if !(1..=100).contains(&page_size) {
        return Err(A2aError::invalid_params(
            "pageSize must be between 1 and 100",
        ));
    }
    if params
        .history_length
        .is_some_and(|length| length > i32::MAX as usize)
    {
        return Err(A2aError::invalid_params(
            "historyLength exceeds the A2A int32 range",
        ));
    }
    validate_v1_tenant(params.tenant.as_deref(), scope)?;
    let status = params
        .status
        .as_deref()
        .map(parse_v1_task_state)
        .transpose()?
        .flatten();
    let cursor = params
        .page_token
        .as_deref()
        .map(decode_list_tasks_cursor)
        .transpose()?;
    if let Some(cursor) = &cursor
        && (cursor.context_id != params.context_id
            || cursor.status != params.status
            || cursor.history_length != params.history_length
            || cursor.status_timestamp_after != params.status_timestamp_after
            || cursor.include_artifacts != params.include_artifacts)
    {
        return Err(A2aError::invalid_params(
            "pageToken does not match the current filters",
        ));
    }

    let mut tasks = engine.list_tasks(scope).await?;
    tasks.retain(|task| {
        params
            .context_id
            .as_ref()
            .is_none_or(|context| task.context_id.as_ref() == Some(context))
            && status.is_none_or(|expected| task.status.state == expected)
            && params
                .status_timestamp_after
                .is_none_or(|after| task.status.timestamp >= after)
    });
    tasks.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    let total_size = tasks.len();
    if let Some(cursor) = &cursor {
        tasks.retain(|task| {
            task.updated_at < cursor.last_updated_at
                || (task.updated_at == cursor.last_updated_at && task.id > cursor.last_id)
        });
    }
    let has_more = tasks.len() > page_size;
    tasks.truncate(page_size);
    if let Some(history_length) = params.history_length {
        for task in &mut tasks {
            if task.history.len() > history_length {
                task.history = task.history.split_off(task.history.len() - history_length);
            }
        }
    }
    if !params.include_artifacts {
        for task in &mut tasks {
            task.artifacts.clear();
        }
    }
    let projected = tasks
        .iter()
        .map(project_v1_task)
        .collect::<Result<Vec<_>, _>>()?;
    let next_page_token = if has_more {
        let last_id = tasks
            .last()
            .expect("a page with more results contains at least one task")
            .id
            .clone();
        let last_updated_at = tasks
            .last()
            .expect("a page with more results contains at least one task")
            .updated_at;
        encode_list_tasks_cursor(&ListTasksCursor {
            last_id,
            last_updated_at,
            context_id: params.context_id,
            status: params.status,
            history_length: params.history_length,
            status_timestamp_after: params.status_timestamp_after,
            include_artifacts: params.include_artifacts,
        })?
    } else {
        String::new()
    };
    Ok(serde_json::json!({
        "tasks": projected,
        "nextPageToken": next_page_token,
        "pageSize": page_size,
        "totalSize": total_size,
    }))
}

// ---------------------------------------------------------------------
// Method implementations — transport-agnostic
// ---------------------------------------------------------------------

/// Build a Task Engine for one request from the gateway's state store,
/// attaching the gateway's compliance-decorated audit store so A2A
/// task transitions land on the same hash chain as action records.
async fn task_engine(state: &AppState) -> TaskEngine {
    let gw = state.gateway.read().await;
    let mut engine = TaskEngine::new(gw.state_store().clone())
        .with_clock(gw.clock())
        .with_legacy_task_access();
    if let Some(audit) = gw.audit_store() {
        engine = engine.with_audit(audit);
    }
    // Wire the gateway's SSE broadcast so Task transitions land on the
    // same channel as the rest of the gateway's stream events; an A2A
    // subscriber can then filter for `action_type = "a2a.task"`.
    engine = engine.with_stream_tx(gw.stream_tx().clone());
    engine
}

/// `message/send` — send a message to an agent.
///
/// A message carrying a `taskId` continues that task (the message is
/// appended to its history); a message without one mints a fresh
/// `Submitted` task. Either way the result is the resulting [`Task`].
#[cfg(test)]
async fn method_message_send(
    engine: &TaskEngine,
    scope: &TaskScope,
    params: MessageSendParams,
) -> Result<Task, A2aError> {
    method_message_send_as(engine, scope, params, "a2a").await
}

async fn method_message_send_as(
    engine: &TaskEngine,
    scope: &TaskScope,
    params: MessageSendParams,
    actor: &str,
) -> Result<Task, A2aError> {
    method_message_send_as_with_push(engine, scope, params, actor, None).await
}

async fn method_message_send_as_with_push(
    engine: &TaskEngine,
    scope: &TaskScope,
    params: MessageSendParams,
    actor: &str,
    inline_push: Option<super::a2a_push::SetPushConfigInput>,
) -> Result<Task, A2aError> {
    let mut message = params.message;
    if message.parts.is_empty() {
        return Err(A2aError::invalid_params("message.parts must not be empty"));
    }
    if let Some(task_id) = message.task_id.clone() {
        let task = engine
            .get_task(scope, &task_id)
            .await?
            .ok_or_else(|| A2aError::task_not_found(&task_id))?;
        if message.context_id.is_none() {
            // A2A permits a continuation to carry only taskId. The server
            // infers the task's context; an explicitly supplied context must
            // still match exactly.
            message.context_id.clone_from(&task.context_id);
        } else if message.context_id != task.context_id {
            return Err(A2aError::invalid_params(
                "message.contextId must exactly match the existing task",
            ));
        }
        let inline_push = inline_push.map(|input| {
            materialize_inline_push_config(scope, &task_id, input, engine.clock().now())
        });
        match task.status.state {
            TaskState::InputRequired => {
                let challenge_id = message
                    .metadata
                    .get(acteon_core::TASK_CHALLENGE_ID_METADATA_KEY)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        A2aError::invalid_params(
                            "an InputRequired response must include metadata.acteon.challengeId",
                        )
                    })?;
                if task.pending_approval_id.as_deref() != Some(challenge_id.as_str()) {
                    return Err(A2aError::invalid_params(
                        "metadata.acteon.challengeId is not the task's active challenge",
                    ));
                }
                if let Some(config) = inline_push {
                    engine.upsert_push_config(scope, &task_id, config).await?;
                }
                Ok(engine
                    .resolve_input(scope, &task_id, &challenge_id, message, actor)
                    .await?
                    .0)
            }
            TaskState::AuthRequired => Err(A2aError::invalid_params(
                "AuthRequired must be fulfilled through a trusted authorization flow; an A2A message cannot grant authority",
            )),
            state if state.is_terminal() => Err(A2aError::invalid_params(format!(
                "task '{task_id}' is terminal ({}) and cannot accept messages",
                state.as_str()
            ))),
            _ => {
                if let Some(config) = inline_push {
                    engine.upsert_push_config(scope, &task_id, config).await?;
                }
                Ok(engine.append_history(scope, &task_id, message).await?)
            }
        }
    } else {
        // Mint a new task with this message as its first history entry.
        let task_id = uuid::Uuid::now_v7().to_string();
        let now = engine.clock().now();
        let mut task = Task::new_at(&task_id, &scope.namespace, &scope.tenant, now);
        task.context_id.clone_from(&message.context_id);
        // Bind the message to the task it now belongs to.
        message.task_id = Some(task_id.clone());
        task.append_history_at(message, now)
            .map_err(|e| A2aError::invalid_params(e.to_string()))?;
        let configs = inline_push
            .map(|input| materialize_inline_push_config(scope, &task_id, input, now))
            .into_iter()
            .collect();
        Ok(engine.create_task_with_push_configs(task, configs).await?)
    }
}

fn materialize_inline_push_config(
    scope: &TaskScope,
    task_id: &str,
    input: super::a2a_push::SetPushConfigInput,
    now: DateTime<Utc>,
) -> acteon_core::TaskPushNotificationConfig {
    acteon_core::TaskPushNotificationConfig {
        id: input.id.unwrap_or_else(|| uuid::Uuid::now_v7().to_string()),
        task_id: task_id.into(),
        namespace: scope.namespace.clone(),
        tenant: scope.tenant.clone(),
        url: input.url,
        token: input.token,
        token_ref: input.token_ref,
        authentication: input.authentication,
        created_at: now,
        updated_at: now,
    }
}

fn v1_return_immediately(params: &Value) -> Result<bool, A2aError> {
    let Some(configuration) = params.get("configuration") else {
        return Ok(false);
    };
    if configuration.is_null() {
        return Ok(false);
    }
    let Some(configuration) = configuration.as_object() else {
        return Err(A2aError::invalid_params("configuration must be an object"));
    };
    match configuration.get("returnImmediately") {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(A2aError::invalid_params(
            "configuration.returnImmediately must be a boolean",
        )),
    }
}

async fn await_v1_send_completion(
    engine: &TaskEngine,
    scope: &TaskScope,
    initial: Task,
) -> Result<Task, A2aError> {
    if initial.status.state.is_terminal() || initial.status.state.is_interrupt() {
        return Ok(initial);
    }
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let task = engine
            .get_task(scope, &initial.id)
            .await?
            .ok_or_else(|| A2aError::task_not_found(&initial.id))?;
        if task.status.state.is_terminal() || task.status.state.is_interrupt() {
            return Ok(task);
        }
    }
}

/// `tasks/get` — fetch a task by id, optionally trimming its history.
async fn method_tasks_get(
    engine: &TaskEngine,
    scope: &TaskScope,
    params: TaskQueryParams,
) -> Result<Task, A2aError> {
    let mut task = engine
        .get_task(scope, &params.id)
        .await?
        .ok_or_else(|| A2aError::task_not_found(&params.id))?;
    if let Some(limit) = params.history_length
        && task.history.len() > limit
    {
        let drop = task.history.len() - limit;
        task.history.drain(0..drop);
    }
    Ok(task)
}

/// `tasks/cancel` — cancel a task. A task already in a terminal state
/// yields [`TASK_NOT_CANCELABLE`].
///
/// If the task is bridge-backed (`Task.chain_id` set) **and** an
/// [`AppState`] is provided, the cancel propagates to the linked
/// Acteon Chain via `Gateway::cancel_chain`; the chain-side bridge
/// hook then projects `Cancelled` back onto the task and we re-fetch
/// the result. With no `AppState` (e.g. unit tests) cancel goes
/// engine-only.
async fn method_tasks_cancel(
    state: Option<&AppState>,
    engine: &TaskEngine,
    scope: &TaskScope,
    params: TaskIdParams,
) -> Result<Task, A2aError> {
    let task = engine
        .get_task(scope, &params.id)
        .await?
        .ok_or_else(|| A2aError::task_not_found(&params.id))?;
    if task.status.state.is_terminal() {
        return Err(A2aError::task_not_cancelable(&params.id));
    }
    if let (Some(chain_id), Some(app)) = (task.chain_id.clone(), state) {
        let gw = app.gateway.read().await;
        if let Err(e) = gw
            .cancel_chain(
                &scope.namespace,
                &scope.tenant,
                &chain_id,
                Some("a2a tasks/cancel".into()),
                None,
            )
            .await
        {
            tracing::error!(
                chain_id = %chain_id,
                error = %e,
                "a2a tasks/cancel: chain cancel failed",
            );
            return Err(A2aError::internal("internal error"));
        }
        drop(gw);
        // The chain IS canceled. The bridge hook attempted to project
        // Cancelled onto the task, but it is best-effort and may have
        // failed (CAS contention etc.). Ensure idempotently: try the
        // transition; if it is already `Canceled` (hook fired) the
        // engine surfaces `Validation(IllegalTransition)`, which we
        // treat as success and re-fetch the canonical row to return.
        return match engine
            .transition_task(scope, &params.id, TaskState::Canceled, None)
            .await
        {
            Ok(t) => Ok(t),
            Err(TaskEngineError::Validation(_)) => engine
                .get_task(scope, &params.id)
                .await?
                .ok_or_else(|| A2aError::task_not_found(&params.id)),
            Err(e) => Err(e.into()),
        };
    }
    Ok(engine
        .transition_task(scope, &params.id, TaskState::Canceled, None)
        .await?)
}

/// `agent/getAuthenticatedExtendedCard` — JSON-RPC counterpart to the
/// public well-known card.
///
/// Loads every `AgentCard` published under the caller's namespace and
/// tenant via the shared discovery helper, returning a single tenant
/// card (verbatim if exactly one is registered, aggregated if several).
/// The method is available only when that resolved card declares the
/// `extended_agent_card` capability.
///
/// The endpoint requires the standard A2A `Dispatch` permission grant
/// (enforced at the JSON-RPC entrypoint), so an unauthenticated caller
/// never reaches this method.
///
/// Returns the canonical A2A unsupported/not-configured errors when the
/// capability or card is absent. `InternalError` is reserved for a state-store
/// read failure.
async fn method_agent_get_authenticated_extended_card(
    state: &AppState,
    scope: &TaskScope,
) -> Result<AgentCard, A2aError> {
    use super::a2a_discovery::resolve_tenant_card;
    match resolve_tenant_card(state, &scope.namespace, &scope.tenant).await {
        Ok(Some(card)) if card.capabilities.extended_agent_card => Ok(card),
        Ok(Some(_)) => Err(A2aError::new(
            UNSUPPORTED_OPERATION,
            "the agent does not support an extended card",
        )),
        Ok(None) => Err(A2aError::new(
            EXTENDED_AGENT_CARD_NOT_CONFIGURED,
            "an extended agent card is not configured",
        )),
        Err(_) => Err(A2aError::internal("internal error")),
    }
}

// ---------------------------------------------------------------------
// Push-notification config methods (Phase 4.1)
// ---------------------------------------------------------------------

/// JSON-RPC params for `tasks/pushNotificationConfig/set`. The inner
/// `push_notification_config` body matches the spec's
/// `PushNotificationConfig` shape (`id` optional) and is delegated to
/// the shared `SetPushConfigInput` in `a2a_push`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PushConfigSetParams {
    task_id: String,
    push_notification_config: super::a2a_push::SetPushConfigInput,
}

/// JSON-RPC params for `tasks/pushNotificationConfig/{get, delete}`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PushConfigIdParams {
    task_id: String,
    push_notification_config_id: String,
}

/// JSON-RPC params for `tasks/pushNotificationConfig/list`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PushConfigListParams {
    task_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct V1PushAuthentication {
    scheme: String,
    #[serde(default)]
    credentials: Option<String>,
    #[serde(default)]
    credentials_ref: Option<acteon_core::SecretRef>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct V1PushConfigInput {
    #[serde(default)]
    tenant: Option<String>,
    #[serde(default)]
    id: Option<String>,
    task_id: String,
    url: String,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    token_ref: Option<acteon_core::SecretRef>,
    #[serde(default)]
    authentication: Option<V1PushAuthentication>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct V1InlinePushConfig {
    #[serde(default)]
    tenant: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    task_id: Option<String>,
    url: String,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    token_ref: Option<acteon_core::SecretRef>,
    #[serde(default)]
    authentication: Option<V1PushAuthentication>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct V1PushConfigIdParams {
    #[serde(default)]
    tenant: Option<String>,
    task_id: String,
    id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct V1PushConfigListParams {
    #[serde(default)]
    tenant: Option<String>,
    task_id: String,
    #[serde(default)]
    page_size: Option<usize>,
    #[serde(default)]
    page_token: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct V1PushConfigCursor {
    task_id: String,
    last_id: String,
}

fn validate_v1_tenant(tenant: Option<&str>, _scope: &TaskScope) -> Result<(), A2aError> {
    if tenant.is_some() {
        return Err(A2aError::invalid_params(
            "tenant is derived from the authenticated credential",
        ));
    }
    Ok(())
}

fn validate_v1_params_tenant(params: &Value, _scope: &TaskScope) -> Result<(), A2aError> {
    if ["tenant", "namespace", "scope", "tenant_scope"]
        .iter()
        .any(|key| params.get(*key).is_some())
    {
        return Err(A2aError::invalid_params(
            "tenant and namespace are derived from the authenticated credential",
        ));
    }
    Ok(())
}

fn project_v1_push_config(
    config: &acteon_core::TaskPushNotificationConfig,
) -> Result<Value, A2aError> {
    let authentication = match &config.authentication {
        None => None,
        Some(authentication) if authentication.schemes.len() == 1 => {
            let mut value = serde_json::json!({
                "scheme": authentication.schemes[0],
            });
            if let Some(credentials) = &authentication.credentials {
                value["credentials"] = Value::String(credentials.clone());
            }
            if let Some(credentials_ref) = &authentication.credentials_ref {
                value["credentialsRef"] = serde_json::to_value(credentials_ref)
                    .map_err(|_| A2aError::internal("internal error"))?;
            }
            Some(value)
        }
        Some(_) => return Err(A2aError::internal("internal error")),
    };
    let mut value = serde_json::json!({
        "id": config.id,
        "taskId": config.task_id,
        "url": config.url,
    });
    if let Some(token) = &config.token {
        value["token"] = Value::String(token.clone());
    }
    if let Some(token_ref) = &config.token_ref {
        value["tokenRef"] =
            serde_json::to_value(token_ref).map_err(|_| A2aError::internal("internal error"))?;
    }
    if let Some(authentication) = authentication {
        value["authentication"] = authentication;
    }
    Ok(value)
}

fn v1_push_input(input: V1PushConfigInput) -> super::a2a_push::SetPushConfigInput {
    super::a2a_push::SetPushConfigInput {
        id: input.id.filter(|id| !id.is_empty()),
        url: input.url,
        token: input.token,
        token_ref: input.token_ref,
        authentication: input.authentication.map(|authentication| {
            acteon_core::PushAuthentication {
                schemes: vec![authentication.scheme],
                credentials: authentication.credentials,
                credentials_ref: authentication.credentials_ref,
            }
        }),
    }
}

fn v1_inline_push_input(
    params: &Value,
    scope: &TaskScope,
) -> Result<Option<super::a2a_push::SetPushConfigInput>, A2aError> {
    let Some(value) = params.pointer("/configuration/taskPushNotificationConfig") else {
        return Ok(None);
    };
    let input = serde_json::from_value::<V1InlinePushConfig>(value.clone())
        .map_err(|e| A2aError::invalid_params(format!("invalid push configuration: {e}")))?;
    validate_v1_tenant(input.tenant.as_deref(), scope)?;
    if input
        .task_id
        .as_deref()
        .is_some_and(|task_id| !task_id.is_empty())
    {
        return Err(A2aError::invalid_params(
            "an inline push configuration taskId must be empty",
        ));
    }
    if input
        .authentication
        .as_ref()
        .is_some_and(|authentication| authentication.scheme.is_empty())
    {
        return Err(A2aError::invalid_params(
            "authentication.scheme must not be empty",
        ));
    }
    Ok(Some(super::a2a_push::SetPushConfigInput {
        id: input.id.filter(|id| !id.is_empty()),
        url: input.url,
        token: input.token,
        token_ref: input.token_ref,
        authentication: input.authentication.map(|authentication| {
            acteon_core::PushAuthentication {
                schemes: vec![authentication.scheme],
                credentials: authentication.credentials,
                credentials_ref: authentication.credentials_ref,
            }
        }),
    }))
}

async fn ensure_v1_push_supported(state: &AppState, scope: &TaskScope) -> Result<(), A2aError> {
    match super::a2a_discovery::resolve_tenant_card(state, &scope.namespace, &scope.tenant).await {
        Ok(Some(card)) if card.capabilities.push_notifications => Ok(()),
        Ok(_) => Err(A2aError::new(
            PUSH_NOTIFICATION_NOT_SUPPORTED,
            "push notifications are not supported",
        )),
        Err(_) => Err(A2aError::internal("internal error")),
    }
}

async fn ensure_v1_streaming_supported(
    state: &AppState,
    scope: &TaskScope,
) -> Result<(), A2aError> {
    match super::a2a_discovery::resolve_tenant_card(state, &scope.namespace, &scope.tenant).await {
        Ok(Some(card)) if card.capabilities.streaming => Ok(()),
        Ok(_) => Err(A2aError::new(
            UNSUPPORTED_OPERATION,
            "streaming is not supported by this agent",
        )),
        Err(_) => Err(A2aError::internal("internal error")),
    }
}

fn validate_inline_push_before_task(
    scope: &TaskScope,
    input: &super::a2a_push::SetPushConfigInput,
) -> Result<(), A2aError> {
    let mut config = acteon_core::TaskPushNotificationConfig::new(
        input.id.as_deref().unwrap_or("pending-config"),
        "pending-task",
        &scope.namespace,
        &scope.tenant,
        &input.url,
    );
    config.token.clone_from(&input.token);
    config.authentication.clone_from(&input.authentication);
    config
        .validate()
        .map_err(|error| A2aError::invalid_params(error.to_string()))?;
    super::a2a_ssrf::check_url_literal(&config.url)
        .map_err(|error| A2aError::invalid_params(format!("push url rejected: {error}")))
}

async fn method_push_config_set(
    state: &AppState,
    scope: &TaskScope,
    params: PushConfigSetParams,
) -> Result<acteon_core::TaskPushNotificationConfig, A2aError> {
    super::a2a_push::save_config(
        state,
        scope,
        &params.task_id,
        params.push_notification_config,
    )
    .await
    .map_err(push_err_to_a2a)
}

async fn method_push_config_get(
    state: &AppState,
    scope: &TaskScope,
    params: PushConfigIdParams,
) -> Result<acteon_core::TaskPushNotificationConfig, A2aError> {
    super::a2a_push::load_config(
        state,
        scope,
        &params.task_id,
        &params.push_notification_config_id,
    )
    .await
    .map_err(push_err_to_a2a)
}

async fn method_push_config_list(
    state: &AppState,
    scope: &TaskScope,
    params: PushConfigListParams,
) -> Result<Vec<acteon_core::TaskPushNotificationConfig>, A2aError> {
    super::a2a_push::list_configs(state, scope, &params.task_id)
        .await
        .map_err(push_err_to_a2a)
}

async fn method_push_config_delete(
    state: &AppState,
    scope: &TaskScope,
    params: PushConfigIdParams,
) -> Result<(), A2aError> {
    super::a2a_push::delete_config(
        state,
        scope,
        &params.task_id,
        &params.push_notification_config_id,
    )
    .await
    .map_err(push_err_to_a2a)
}

async fn method_push_config_create_v1(
    state: &AppState,
    scope: &TaskScope,
    params: V1PushConfigInput,
) -> Result<Value, A2aError> {
    ensure_v1_push_supported(state, scope).await?;
    validate_v1_tenant(params.tenant.as_deref(), scope)?;
    if params.task_id.is_empty() {
        return Err(A2aError::invalid_params("taskId must not be empty"));
    }
    if params
        .authentication
        .as_ref()
        .is_some_and(|authentication| authentication.scheme.is_empty())
    {
        return Err(A2aError::invalid_params(
            "authentication.scheme must not be empty",
        ));
    }
    let task_id = params.task_id.clone();
    let config = super::a2a_push::save_config(state, scope, &task_id, v1_push_input(params))
        .await
        .map_err(push_err_to_a2a)?;
    project_v1_push_config(&config)
}

async fn method_push_config_get_v1(
    state: &AppState,
    scope: &TaskScope,
    params: V1PushConfigIdParams,
) -> Result<Value, A2aError> {
    ensure_v1_push_supported(state, scope).await?;
    validate_v1_tenant(params.tenant.as_deref(), scope)?;
    if params.task_id.is_empty() || params.id.is_empty() {
        return Err(A2aError::invalid_params("taskId and id must not be empty"));
    }
    let config = super::a2a_push::load_config(state, scope, &params.task_id, &params.id)
        .await
        .map_err(push_err_to_a2a)?;
    project_v1_push_config(&config)
}

async fn method_push_config_list_v1(
    state: &AppState,
    scope: &TaskScope,
    params: V1PushConfigListParams,
) -> Result<Value, A2aError> {
    ensure_v1_push_supported(state, scope).await?;
    validate_v1_tenant(params.tenant.as_deref(), scope)?;
    if params.task_id.is_empty() {
        return Err(A2aError::invalid_params("taskId must not be empty"));
    }
    let page_size = params.page_size.unwrap_or(50);
    if !(1..=100).contains(&page_size) {
        return Err(A2aError::invalid_params(
            "pageSize must be between 1 and 100",
        ));
    }
    let cursor = params
        .page_token
        .as_deref()
        .map(|token| {
            URL_SAFE_NO_PAD
                .decode(token)
                .map_err(|_| A2aError::invalid_params("invalid pageToken"))
                .and_then(|bytes| {
                    serde_json::from_slice::<V1PushConfigCursor>(&bytes)
                        .map_err(|_| A2aError::invalid_params("invalid pageToken"))
                })
        })
        .transpose()?;
    if cursor
        .as_ref()
        .is_some_and(|cursor| cursor.task_id != params.task_id)
    {
        return Err(A2aError::invalid_params(
            "pageToken does not match the requested task",
        ));
    }
    let mut configs = super::a2a_push::list_configs(state, scope, &params.task_id)
        .await
        .map_err(push_err_to_a2a)?;
    configs.sort_by(|left, right| left.id.cmp(&right.id));
    if let Some(cursor) = &cursor {
        configs.retain(|config| config.id > cursor.last_id);
    }
    let has_more = configs.len() > page_size;
    configs.truncate(page_size);
    let projected = configs
        .iter()
        .map(project_v1_push_config)
        .collect::<Result<Vec<_>, _>>()?;
    let next_page_token = if has_more {
        let cursor = V1PushConfigCursor {
            task_id: params.task_id,
            last_id: configs
                .last()
                .expect("a page with more results contains at least one config")
                .id
                .clone(),
        };
        URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&cursor).map_err(|_| A2aError::internal("internal error"))?)
    } else {
        String::new()
    };
    Ok(serde_json::json!({
        "configs": projected,
        "nextPageToken": next_page_token,
    }))
}

async fn method_push_config_delete_v1(
    state: &AppState,
    scope: &TaskScope,
    params: V1PushConfigIdParams,
) -> Result<Value, A2aError> {
    ensure_v1_push_supported(state, scope).await?;
    validate_v1_tenant(params.tenant.as_deref(), scope)?;
    if params.task_id.is_empty() || params.id.is_empty() {
        return Err(A2aError::invalid_params("taskId and id must not be empty"));
    }
    match super::a2a_push::delete_config(state, scope, &params.task_id, &params.id).await {
        Ok(()) | Err(super::a2a_push::PushConfigError::ConfigNotFound { .. }) => {}
        Err(error) => return Err(push_err_to_a2a(error)),
    }
    Ok(serde_json::json!({}))
}

/// Translate the helper's typed error into the JSON-RPC error model.
/// `ConfigNotFound` re-uses `TASK_NOT_FOUND` (-32001) — A2A does not
/// define a separate code for a missing sub-resource, and a sentinel
/// "resource missing" code is the closest fit.
fn push_err_to_a2a(e: super::a2a_push::PushConfigError) -> A2aError {
    use super::a2a_push::PushConfigError;
    match e {
        PushConfigError::TaskNotFound(id) => A2aError::task_not_found(&id),
        PushConfigError::ConfigNotFound { task_id, config_id } => A2aError::new(
            TASK_NOT_FOUND,
            format!("push config '{config_id}' not found on task '{task_id}'"),
        ),
        PushConfigError::Invalid(msg) => A2aError::invalid_params(msg),
        PushConfigError::Internal => A2aError::internal("internal error"),
    }
}

// ---------------------------------------------------------------------
// Shared request plumbing
// ---------------------------------------------------------------------

/// Reject a request that pins an `A2A-Version` this server does not
/// implement. An absent header is accepted — the caller is assumed to
/// speak the current version.
fn negotiate_version(headers: &HeaderMap) -> Result<(), A2aError> {
    if let Some(raw) = headers.get(A2A_VERSION_HEADER) {
        let got = raw.to_str().unwrap_or("");
        if got != A2A_PROTOCOL_VERSION {
            return Err(A2aError::version_not_supported(got, A2A_PROTOCOL_VERSION));
        }
    }
    Ok(())
}

/// Authorize an A2A call: the caller needs the `Dispatch` permission
/// and a grant covering `(tenant, namespace)` for the synthetic `a2a`
/// provider.
fn authorize(
    identity: &CallerIdentity,
    namespace: &str,
    tenant: &str,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if !identity.role.has_permission(Permission::Dispatch) {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "a2a requires the dispatch permission (admin, operator, or executor role)"
                    .into(),
            }),
        ));
    }
    if !identity.is_authorized(tenant, namespace, "a2a", "rpc") {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: format!(
                    "forbidden: no grant covers tenant={tenant}, namespace={namespace}, provider=a2a"
                ),
            }),
        ));
    }
    Ok(())
}

/// A2A methods that drive or mutate task state. A caller whose bound
/// agent has been suspended or banned by an operator is blocked from
/// these (mirroring the bus `is_routable` gate); read methods
/// (`tasks/get`, task events) and discovery stay open so a paused agent
/// can still observe its in-flight work.
fn is_write_method(method: &str) -> bool {
    matches!(
        method,
        "message/send"
            | "tasks/cancel"
            | "SendMessage"
            | "SendStreamingMessage"
            | "CancelTask"
            | "CreateTaskPushNotificationConfig"
            | "DeleteTaskPushNotificationConfig"
    )
}

/// Whether a parsed JSON-RPC payload (a single request object or a batch
/// array) contains any write method — i.e. whether the agent-lifecycle
/// gate must run for this request.
fn payload_has_write_method(parsed: &Value) -> bool {
    fn is_write_obj(v: &Value) -> bool {
        v.get("method")
            .and_then(Value::as_str)
            .is_some_and(is_write_method)
    }
    match parsed {
        Value::Array(items) => items.iter().any(is_write_obj),
        other => is_write_obj(other),
    }
}

/// Pure routability decision: the 403 a Suspended/Banned agent must
/// receive, or `None` when the agent's effective state is routable.
/// Mirrors the bus send-path message — it includes the operator's reason
/// when present but never the operator's identity (`admin_set_by`).
fn agent_routability_rejection(
    agent: &Agent,
    namespace: &str,
    tenant: &str,
    agent_id: &str,
) -> Option<(StatusCode, ErrorResponse)> {
    let effective = agent.effective_admin_state();
    if effective.is_routable() {
        return None;
    }
    let reason_part = agent
        .admin_reason
        .as_deref()
        .map(|r| format!(": {r}"))
        .unwrap_or_default();
    Some((
        StatusCode::FORBIDDEN,
        ErrorResponse {
            error: format!(
                "agent {namespace}/{tenant}/{agent_id} is {}{reason_part}",
                effective.as_str(),
            ),
        },
    ))
}

/// Enforce the operator lifecycle on the caller's bound agent before a
/// task-driving A2A method (`message/send`, `tasks/cancel`): a Suspended
/// or Banned agent is rejected with 403, mirroring the bus `is_routable`
/// gate so the two protocols that share the agent identity model also
/// share its kill-switch.
///
/// The agent is resolved from the caller's grant binding for this
/// `(tenant, namespace)` scope. A caller bound to no agent (`Ok(None)`),
/// or bound to an id that was never registered, is unaffected — only a
/// registered, non-routable agent is rejected. Conflicting bindings are
/// an operator misconfiguration and are refused.
pub(crate) async fn enforce_agent_routable(
    state: &AppState,
    identity: &CallerIdentity,
    namespace: &str,
    tenant: &str,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let agent_id = match identity.bus_agent_id_for_scope(tenant, namespace) {
        Ok(Some(id)) => id.to_string(),
        Ok(None) => return Ok(()),
        Err(e) => {
            return Err((
                StatusCode::FORBIDDEN,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            ));
        }
    };
    let Some(agent) = load_caller_agent(state, namespace, tenant, &agent_id).await? else {
        return Ok(());
    };
    match agent_routability_rejection(&agent, namespace, tenant, &agent_id) {
        Some((status, body)) => Err((status, Json(body))),
        None => Ok(()),
    }
}

/// Load the caller's agent record from its bus-agent state key.
/// `Ok(None)` when the record does not exist (a binding to an
/// unregistered id has no lifecycle to enforce); `Err` only on a
/// store/decode failure.
async fn load_caller_agent(
    state: &AppState,
    namespace: &str,
    tenant: &str,
    agent_id: &str,
) -> Result<Option<Agent>, (StatusCode, Json<ErrorResponse>)> {
    let key = StateKey::new(
        namespace.to_string(),
        tenant.to_string(),
        KeyKind::BusAgent,
        agent_id.to_string(),
    );
    let gw = state.gateway.read().await;
    match gw.state_store().get(&key).await {
        Ok(Some(raw)) => serde_json::from_str::<Agent>(&raw).map(Some).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("corrupt agent record for {namespace}.{tenant}.{agent_id}: {e}"),
                }),
            )
        }),
        Ok(None) => Ok(None),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

/// The `A2A-Version` response header, attached to every A2A response.
fn version_header() -> [(&'static str, &'static str); 1] {
    [(A2A_VERSION_HEADER, A2A_PROTOCOL_VERSION)]
}

// ---------------------------------------------------------------------
// JSON-RPC transport
// ---------------------------------------------------------------------

fn success_response(id: Value, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        result: Some(result),
        error: None,
        id,
    }
}

fn error_response(id: Value, err: &A2aError) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        result: None,
        error: Some(err.to_jsonrpc()),
        id,
    }
}

/// Convert an internal Task into the schema-closed A2A 1.0 `ProtoJSON` view.
/// Acteon-only ownership, recovery and execution fields never cross the
/// protocol boundary.
pub(crate) fn project_v1_task(task: &Task) -> Result<Value, A2aError> {
    let mut value = to_value(task)?;
    let Some(object) = value.as_object_mut() else {
        return Err(A2aError::internal("internal error"));
    };
    for private in [
        "namespace",
        "tenant",
        "createdAt",
        "updatedAt",
        "lastProgressAt",
        "workingTtlMs",
        "pendingApprovalId",
        "chainId",
        "artifactStreams",
    ] {
        object.remove(private);
    }
    if let Some(status) = object.get_mut("status") {
        project_v1_status(status);
    }
    if let Some(history) = object.get_mut("history").and_then(Value::as_array_mut) {
        for message in history {
            project_v1_message(message);
        }
    }
    Ok(value)
}

fn project_v1_status(status: &mut Value) {
    let Some(object) = status.as_object_mut() else {
        return;
    };
    if let Some(state) = object.get_mut("state")
        && let Some(name) = state.as_str()
    {
        *state = Value::String(format!("TASK_STATE_{}", name.to_ascii_uppercase()));
    }
    if let Some(message) = object.get_mut("message") {
        project_v1_message(message);
    }
}

fn project_v1_message(message: &mut Value) {
    let Some(object) = message.as_object_mut() else {
        return;
    };
    if let Some(role) = object.get_mut("role")
        && let Some(name) = role.as_str()
    {
        *role = Value::String(format!("ROLE_{}", name.to_ascii_uppercase()));
    }
}

fn normalize_v1_send_params(mut params: Value) -> Value {
    if let Some(role) = params
        .get_mut("message")
        .and_then(Value::as_object_mut)
        .and_then(|message| message.get_mut("role"))
        && let Some(name) = role.as_str()
        && let Some(name) = name.strip_prefix("ROLE_")
    {
        *role = Value::String(name.to_ascii_lowercase());
    }
    params
}

/// A single JSON-RPC error response (HTTP 200, `id` null) — used for
/// failures detected before any request object is dispatched: version
/// mismatch, an unparseable body, an empty batch.
fn rpc_single_error(err: &A2aError) -> Response {
    (
        StatusCode::OK,
        version_header(),
        Json(error_response(Value::Null, err)),
    )
        .into_response()
}

/// The outcome of a JSON-RPC payload: a single response, a batch of
/// responses, or nothing at all (the payload was a notification, or a
/// batch of only notifications — the spec says answer with no body).
#[derive(Debug)]
enum RpcReply {
    Single(JsonRpcResponse),
    Batch(Vec<JsonRpcResponse>),
    Empty,
}

fn is_v1_method(method: &str) -> bool {
    matches!(
        method,
        "SendMessage"
            | "GetTask"
            | "ListTasks"
            | "CancelTask"
            | "GetExtendedAgentCard"
            | "SendStreamingMessage"
            | "SubscribeToTask"
            | "CreateTaskPushNotificationConfig"
            | "GetTaskPushNotificationConfig"
            | "ListTaskPushNotificationConfigs"
            | "DeleteTaskPushNotificationConfig"
    )
}

async fn method_send_v1(
    state: Option<&AppState>,
    engine: &TaskEngine,
    scope: &TaskScope,
    params: Value,
    actor: &str,
) -> Result<Value, A2aError> {
    let return_immediately = v1_return_immediately(&params)?;
    let mut task = method_send_v1_immediate(state, engine, scope, params, actor).await?;
    if !return_immediately {
        task = await_v1_send_completion(engine, scope, task).await?;
    }
    Ok(serde_json::json!({"task": project_v1_task(&task)?}))
}

/// Accept a v1 message and return its durable Task immediately. Streaming
/// calls use this path because completion is represented by later SSE frames;
/// the finite `SendMessage` adapter may still wait according to its request
/// configuration after this common acceptance step.
async fn method_send_v1_immediate(
    state: Option<&AppState>,
    engine: &TaskEngine,
    scope: &TaskScope,
    params: Value,
    actor: &str,
) -> Result<Task, A2aError> {
    validate_v1_params_tenant(&params, scope)?;
    let inline_push = v1_inline_push_input(&params, scope)?;
    if let Some(inline_push) = &inline_push {
        ensure_v1_push_supported(require_state(state)?, scope).await?;
        validate_inline_push_before_task(scope, inline_push)?;
    }
    let parsed = serde_json::from_value::<MessageSendParams>(normalize_v1_send_params(params))
        .map_err(|e| A2aError::invalid_params(format!("invalid params: {e}")))?;
    let task = method_message_send_as_with_push(engine, scope, parsed, actor, inline_push).await?;
    Ok(task)
}

async fn dispatch_v1_method(
    state: Option<&AppState>,
    engine: &TaskEngine,
    scope: &TaskScope,
    method: &str,
    params: Value,
    actor: &str,
) -> Result<Value, A2aError> {
    validate_v1_params_tenant(&params, scope)?;
    match method {
        "SendMessage" => method_send_v1(state, engine, scope, params, actor).await,
        "GetTask" => {
            let parsed = serde_json::from_value::<TaskQueryParams>(params)
                .map_err(|e| A2aError::invalid_params(format!("invalid params: {e}")))?;
            project_v1_task(&method_tasks_get(engine, scope, parsed).await?)
        }
        "ListTasks" => {
            let parsed = serde_json::from_value::<ListTasksParams>(params)
                .map_err(|e| A2aError::invalid_params(format!("invalid params: {e}")))?;
            method_list_tasks_v1(engine, scope, parsed).await
        }
        "CancelTask" => {
            let parsed = serde_json::from_value::<TaskIdParams>(params)
                .map_err(|e| A2aError::invalid_params(format!("invalid params: {e}")))?;
            project_v1_task(&method_tasks_cancel(state, engine, scope, parsed).await?)
        }
        "GetExtendedAgentCard" => {
            let card =
                method_agent_get_authenticated_extended_card(require_state(state)?, scope).await?;
            Ok(super::a2a_discovery::project_v1_card(&card))
        }
        "SendStreamingMessage" | "SubscribeToTask" => Err(A2aError::new(
            UNSUPPORTED_OPERATION,
            "streaming is not supported by this A2A interface",
        )),
        "CreateTaskPushNotificationConfig" => {
            let parsed = serde_json::from_value::<V1PushConfigInput>(params)
                .map_err(|e| A2aError::invalid_params(format!("invalid params: {e}")))?;
            method_push_config_create_v1(require_state(state)?, scope, parsed).await
        }
        "GetTaskPushNotificationConfig" | "DeleteTaskPushNotificationConfig" => {
            let parsed = serde_json::from_value::<V1PushConfigIdParams>(params)
                .map_err(|e| A2aError::invalid_params(format!("invalid params: {e}")))?;
            if method == "GetTaskPushNotificationConfig" {
                method_push_config_get_v1(require_state(state)?, scope, parsed).await
            } else {
                method_push_config_delete_v1(require_state(state)?, scope, parsed).await
            }
        }
        "ListTaskPushNotificationConfigs" => {
            let parsed = serde_json::from_value::<V1PushConfigListParams>(params)
                .map_err(|e| A2aError::invalid_params(format!("invalid params: {e}")))?;
            method_push_config_list_v1(require_state(state)?, scope, parsed).await
        }
        _ => unreachable!("dispatch_v1_method called with a legacy method"),
    }
}

/// Route an A2A method to its implementation. `state` is the
/// production hook for `tasks/cancel`'s chain-cancel propagation and
/// for the discovery-store reads behind `agent/getAuthenticatedExtendedCard`;
/// tests pass `None` for engine-only behavior.
///
/// Returns the method's result already serialized to a JSON `Value` so
/// the dispatcher can carry differently-typed results (`Task`,
/// `AgentCard`) through one signature.
async fn dispatch_method(
    state: Option<&AppState>,
    engine: &TaskEngine,
    scope: &TaskScope,
    method: &str,
    params: Value,
    actor: &str,
) -> Result<Value, A2aError> {
    validate_v1_params_tenant(&params, scope)?;
    if is_v1_method(method) {
        return dispatch_v1_method(state, engine, scope, method, params, actor).await;
    }
    match method {
        "message/send" => {
            let p = serde_json::from_value::<MessageSendParams>(params)
                .map_err(|e| A2aError::invalid_params(format!("invalid params: {e}")))?;
            to_value(&method_message_send_as(engine, scope, p, actor).await?)
        }
        "tasks/get" => {
            let p = serde_json::from_value::<TaskQueryParams>(params)
                .map_err(|e| A2aError::invalid_params(format!("invalid params: {e}")))?;
            to_value(&method_tasks_get(engine, scope, p).await?)
        }
        "tasks/cancel" => {
            let p = serde_json::from_value::<TaskIdParams>(params)
                .map_err(|e| A2aError::invalid_params(format!("invalid params: {e}")))?;
            to_value(&method_tasks_cancel(state, engine, scope, p).await?)
        }
        "agent/getAuthenticatedExtendedCard" => {
            // The discovery store hangs off `AppState`; without it the
            // method has no card source. In tests that pass `None` the
            // method is unavailable — an honest error rather than a
            // fabricated empty card.
            let Some(s) = state else {
                return Err(A2aError::new(
                    INTERNAL_ERROR,
                    "extended-card discovery is unavailable in this dispatcher",
                ));
            };
            to_value(&method_agent_get_authenticated_extended_card(s, scope).await?)
        }
        "tasks/pushNotificationConfig/set" => {
            // Param validation runs before `require_state` so a
            // malformed body reports INVALID_PARAMS even when the
            // dispatcher is wired without an `AppState` (in tests).
            let p = serde_json::from_value::<PushConfigSetParams>(params)
                .map_err(|e| A2aError::invalid_params(format!("invalid params: {e}")))?;
            let s = require_state(state)?;
            to_value(&method_push_config_set(s, scope, p).await?)
        }
        "tasks/pushNotificationConfig/get" => {
            let p = serde_json::from_value::<PushConfigIdParams>(params)
                .map_err(|e| A2aError::invalid_params(format!("invalid params: {e}")))?;
            let s = require_state(state)?;
            to_value(&method_push_config_get(s, scope, p).await?)
        }
        "tasks/pushNotificationConfig/list" => {
            let p = serde_json::from_value::<PushConfigListParams>(params)
                .map_err(|e| A2aError::invalid_params(format!("invalid params: {e}")))?;
            let s = require_state(state)?;
            to_value(&method_push_config_list(s, scope, p).await?)
        }
        "tasks/pushNotificationConfig/delete" => {
            let p = serde_json::from_value::<PushConfigIdParams>(params)
                .map_err(|e| A2aError::invalid_params(format!("invalid params: {e}")))?;
            let s = require_state(state)?;
            method_push_config_delete(s, scope, p).await?;
            Ok(Value::Null)
        }
        other => Err(A2aError::new(
            METHOD_NOT_FOUND,
            format!("method '{other}' is not supported"),
        )),
    }
}

/// Methods that touch the state store directly (push-config CRUD,
/// extended card) need the `AppState`. In tests that pass `None` we
/// answer with a honest `InternalError` rather than acting on a
/// fabricated empty store.
fn require_state(state: Option<&AppState>) -> Result<&AppState, A2aError> {
    state.ok_or_else(|| {
        A2aError::new(
            INTERNAL_ERROR,
            "this method is unavailable in the dispatcher (no AppState)",
        )
    })
}

/// Serialize a method result for the JSON-RPC envelope. A failure here
/// is a server-internal bug (every method's return type is `Serialize`),
/// so it is mapped to JSON-RPC `-32603 InternalError` with a generic
/// message — never leaking the underlying serde diagnostic.
fn to_value<T: Serialize>(value: &T) -> Result<Value, A2aError> {
    serde_json::to_value(value).map_err(|_| A2aError::internal("internal error"))
}

/// Dispatch one JSON-RPC request object.
///
/// Returns `None` when the object is a **notification** — a request
/// with no `id` member: per JSON-RPC 2.0 it is still processed, but
/// the server MUST NOT answer it (even if it is otherwise malformed).
/// Returns `Some(response)` for an id-bearing request.
async fn dispatch_rpc_value(
    state: Option<&AppState>,
    engine: &TaskEngine,
    scope: &TaskScope,
    value: &Value,
    actor: &str,
) -> Option<JsonRpcResponse> {
    let Some(obj) = value.as_object() else {
        // Not an object — cannot be a notification (no `id` to be
        // absent *from*), so it is an Invalid Request.
        return Some(error_response(
            Value::Null,
            &A2aError::new(INVALID_REQUEST, "request must be a JSON object"),
        ));
    };
    // Presence of the `id` member — not its value — decides
    // notification vs. request. A present `"id": null` is a request.
    let is_notification = !obj.contains_key("id");
    let id = obj.get("id").cloned().unwrap_or(Value::Null);

    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return (!is_notification).then(|| {
            error_response(
                id.clone(),
                &A2aError::new(INVALID_REQUEST, "the jsonrpc field must be \"2.0\""),
            )
        });
    }
    let Some(method) = obj.get("method").and_then(Value::as_str) else {
        return (!is_notification).then(|| {
            error_response(
                id.clone(),
                &A2aError::new(INVALID_REQUEST, "missing or non-string method"),
            )
        });
    };
    let params = obj.get("params").cloned().unwrap_or(Value::Null);
    let outcome = dispatch_method(state, engine, scope, method, params, actor).await;
    if is_notification {
        // Processed; a notification is answered with nothing.
        return None;
    }
    Some(match outcome {
        Ok(value) => success_response(id, value),
        Err(e) => error_response(id, &e),
    })
}

/// Process a parsed JSON-RPC payload — a single request object or a
/// batch array.
#[cfg(test)]
async fn handle_rpc_payload(
    state: Option<&AppState>,
    engine: &TaskEngine,
    scope: &TaskScope,
    parsed: Value,
) -> RpcReply {
    handle_rpc_payload_as(state, engine, scope, parsed, "a2a").await
}

async fn handle_rpc_payload_as(
    state: Option<&AppState>,
    engine: &TaskEngine,
    scope: &TaskScope,
    parsed: Value,
    actor: &str,
) -> RpcReply {
    match parsed {
        Value::Array(items) => {
            if items.is_empty() {
                return RpcReply::Single(error_response(
                    Value::Null,
                    &A2aError::new(INVALID_REQUEST, "a batch request must not be empty"),
                ));
            }
            let mut responses = Vec::new();
            for item in &items {
                if let Some(resp) = dispatch_rpc_value(state, engine, scope, item, actor).await {
                    responses.push(resp);
                }
            }
            // A batch of only notifications is answered with no body.
            if responses.is_empty() {
                RpcReply::Empty
            } else {
                RpcReply::Batch(responses)
            }
        }
        obj @ Value::Object(_) => match dispatch_rpc_value(state, engine, scope, &obj, actor).await
        {
            Some(resp) => RpcReply::Single(resp),
            None => RpcReply::Empty,
        },
        _ => RpcReply::Single(error_response(
            Value::Null,
            &A2aError::new(
                INVALID_REQUEST,
                "request must be a JSON object or a batch array",
            ),
        )),
    }
}

struct V1TaskStreamState {
    rx: broadcast::Receiver<StreamEvent>,
    engine: TaskEngine,
    scope: TaskScope,
    task_id: String,
    request_id: Value,
    initial: Option<Value>,
    done: bool,
    close_on_interrupt: bool,
    _guard: ConnectionGuard,
}

struct V1TaskStreamSeed {
    task_id: String,
    initial_task: Value,
    request_id: Value,
    close_on_interrupt: bool,
}

fn v1_stream_response_event(id: &Value, result: Value) -> Option<Event> {
    serde_json::to_string(&success_response(id.clone(), result))
        .ok()
        .map(|data| Event::default().data(data))
}

fn v1_status_update(
    task: &Task,
    state: TaskState,
    timestamp: DateTime<Utc>,
) -> Result<Value, A2aError> {
    acteon_gateway::task_engine::project_task_status_push_payload(task, state, timestamp)
        .map_err(|_| A2aError::internal("internal error"))
}

pub(crate) fn project_v1_status_update(
    task: &Task,
    state: TaskState,
    timestamp: DateTime<Utc>,
) -> Option<Value> {
    v1_status_update(task, state, timestamp).ok()
}

fn v1_artifact_update(task: &Task, artifact_id: &str, last_chunk: bool) -> Option<Value> {
    acteon_gateway::task_engine::project_task_artifact_push_payload(task, artifact_id, last_chunk)
        .ok()
}

pub(crate) fn project_v1_artifact_update(
    task: &Task,
    artifact_id: &str,
    last_chunk: bool,
) -> Option<Value> {
    v1_artifact_update(task, artifact_id, last_chunk)
}

fn make_v1_task_stream(
    rx: broadcast::Receiver<StreamEvent>,
    engine: TaskEngine,
    scope: TaskScope,
    seed: V1TaskStreamSeed,
    guard: ConnectionGuard,
) -> impl Stream<Item = Result<Event, Infallible>> {
    let mut initial = serde_json::Map::new();
    initial.insert("task".into(), seed.initial_task);
    let state = V1TaskStreamState {
        rx,
        engine,
        scope,
        task_id: seed.task_id,
        request_id: seed.request_id,
        initial: Some(Value::Object(initial)),
        done: false,
        close_on_interrupt: seed.close_on_interrupt,
        _guard: guard,
    };

    stream::unfold(state, |mut state| async move {
        if state.done {
            return None;
        }
        if let Some(initial) = state.initial.take() {
            let event = v1_stream_response_event(&state.request_id, initial)?;
            return Some((Ok(event), state));
        }

        loop {
            let event = match state.rx.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Closed) => return None,
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let task = state
                        .engine
                        .get_task(&state.scope, &state.task_id)
                        .await
                        .ok()
                        .flatten()?;
                    state.done = task.status.state.is_terminal()
                        || (state.close_on_interrupt && task.status.state.is_interrupt());
                    let result = serde_json::json!({"task": project_v1_task(&task).ok()?});
                    let frame = v1_stream_response_event(&state.request_id, result)?;
                    return Some((Ok(frame), state));
                }
            };
            if event.namespace != state.scope.namespace
                || event.tenant != state.scope.tenant
                || event.action_type.as_deref() != Some("a2a.task")
                || event.action_id.as_deref() != Some(state.task_id.as_str())
            {
                continue;
            }

            let task = state
                .engine
                .get_task(&state.scope, &state.task_id)
                .await
                .ok()
                .flatten()?;
            let (result, closes_stream) = match &event.event_type {
                StreamEventType::TaskTransitioned { to, .. } => (
                    v1_status_update(&task, *to, event.timestamp).ok(),
                    to.is_terminal() || (state.close_on_interrupt && to.is_interrupt()),
                ),
                StreamEventType::TaskArtifactUpdated {
                    artifact_id,
                    last_chunk,
                    ..
                } => (v1_artifact_update(&task, artifact_id, *last_chunk), false),
                // StreamResponse task lifecycles permit Task first, followed
                // by status/artifact updates. In particular, a Message is a
                // standalone response that closes the stream, so history
                // notifications remain in the durable Task snapshot.
                _ => (None, false),
            };
            let Some(result) = result else {
                continue;
            };
            state.done = closes_stream;
            let frame = v1_stream_response_event(&state.request_id, result)?;
            return Some((Ok(frame), state));
        }
    })
}

fn v1_stream_rpc_error(id: Value, error: &A2aError) -> Response {
    (
        StatusCode::OK,
        version_header(),
        Json(error_response(id, error)),
    )
        .into_response()
}

#[allow(clippy::too_many_lines)]
async fn handle_v1_streaming_rpc(
    state: &AppState,
    engine: &TaskEngine,
    scope: &TaskScope,
    parsed: &Value,
    actor: &str,
) -> Response {
    let Some(object) = parsed.as_object() else {
        return v1_stream_rpc_error(
            Value::Null,
            &A2aError::new(INVALID_REQUEST, "streaming requests must be JSON objects"),
        );
    };
    let request_id = object.get("id").cloned().unwrap_or(Value::Null);
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") || !object.contains_key("id") {
        return v1_stream_rpc_error(
            request_id,
            &A2aError::new(
                INVALID_REQUEST,
                "streaming requests require jsonrpc \"2.0\" and an id",
            ),
        );
    }
    let method = object
        .get("method")
        .and_then(Value::as_str)
        .expect("streaming dispatcher is entered only for a string method");
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    if let Err(error) = validate_v1_params_tenant(&params, scope) {
        return v1_stream_rpc_error(request_id, &error);
    }
    if let Err(error) = ensure_v1_streaming_supported(state, scope).await {
        return v1_stream_rpc_error(request_id, &error);
    }

    let Some(registry) = state.connection_registry.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            version_header(),
            Json(ErrorResponse {
                error: "SSE streaming is not enabled".into(),
            }),
        )
            .into_response();
    };
    let bucket = format!("{}:{}", scope.namespace, scope.tenant);
    let Some(guard) = registry.try_acquire(&bucket).await else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            version_header(),
            Json(ErrorResponse {
                error: "too many concurrent SSE connections for this tenant".into(),
            }),
        )
            .into_response();
    };

    // Register before the send/fetch operation. A transition racing with the
    // initial snapshot remains buffered in this receiver and cannot be lost.
    let gateway = state.gateway.read().await;
    let rx = gateway.stream_tx().subscribe();
    drop(gateway);

    let (task, close_on_interrupt) = match method {
        "SendStreamingMessage" => {
            if let Err(error) = v1_return_immediately(&params) {
                return v1_stream_rpc_error(request_id, &error);
            }
            match method_send_v1_immediate(Some(state), engine, scope, params, actor).await {
                Ok(task) => (task, true),
                Err(error) => return v1_stream_rpc_error(request_id, &error),
            }
        }
        "SubscribeToTask" => {
            let parsed = match serde_json::from_value::<V1SubscribeParams>(params) {
                Ok(parsed) => parsed,
                Err(error) => {
                    return v1_stream_rpc_error(
                        request_id,
                        &A2aError::invalid_params(format!("invalid params: {error}")),
                    );
                }
            };
            if let Err(error) = validate_v1_tenant(parsed.tenant.as_deref(), scope) {
                return v1_stream_rpc_error(request_id, &error);
            }
            match engine.get_task(scope, &parsed.id).await {
                Ok(Some(task)) if task.status.state.is_terminal() => {
                    return v1_stream_rpc_error(
                        request_id,
                        &A2aError::new(
                            UNSUPPORTED_OPERATION,
                            "a terminal task cannot be subscribed to",
                        ),
                    );
                }
                Ok(Some(task)) => (task, false),
                Ok(None) => {
                    return v1_stream_rpc_error(request_id, &A2aError::task_not_found(&parsed.id));
                }
                Err(error) => return v1_stream_rpc_error(request_id, &error.into()),
            }
        }
        _ => unreachable!("non-streaming method entered streaming dispatcher"),
    };

    let initial_task = match project_v1_task(&task) {
        Ok(task) => task,
        Err(error) => return v1_stream_rpc_error(request_id, &error),
    };
    let event_stream = make_v1_task_stream(
        rx,
        engine.clone(),
        scope.clone(),
        V1TaskStreamSeed {
            task_id: task.id,
            initial_task,
            request_id,
            close_on_interrupt,
        },
        guard,
    );
    (
        version_header(),
        Sse::new(event_stream).keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("ping"),
        ),
    )
        .into_response()
}

/// `POST /a2a` — the A2A JSON-RPC 2.0 endpoint.
///
/// The raw body is parsed here rather than via the `Json` extractor so
/// a malformed body becomes a JSON-RPC parse error (`-32700`) instead
/// of an HTTP 400 the A2A client cannot interpret.
pub async fn a2a_rpc(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    crate::auth::context::ScopedPath((namespace, tenant)): crate::auth::context::ScopedPath<(
        String,
        String,
    )>,
    headers: HeaderMap,
    body: String,
) -> Response {
    if let Err(e) = negotiate_version(&headers) {
        return rpc_single_error(&e);
    }
    if let Err((status, errbody)) = authorize(&identity, &namespace, &tenant) {
        return (status, version_header(), errbody).into_response();
    }
    let parsed: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            return rpc_single_error(&A2aError::new(PARSE_ERROR, format!("parse error: {e}")));
        }
    };
    // The RPC endpoint multiplexes read, execution and administration verbs.
    // Match the REST role ceiling before processing *any* batch member, including
    // notifications, so a mixed batch cannot partially mutate state.
    if !identity.role.has_permission(Permission::OperationsManage)
        && payload_requires_management(&parsed)
    {
        return (
            StatusCode::FORBIDDEN,
            version_header(),
            Json(ErrorResponse {
                error: "A2A method requires OperationsManage permission".into(),
            }),
        )
            .into_response();
    }
    // Operator lifecycle: block a Suspended/Banned caller agent from any
    // request that drives or mutates tasks. Read-only payloads (tasks/get)
    // stay open, so the gate runs only when a write method is present.
    if payload_has_write_method(&parsed)
        && let Err((status, body)) =
            enforce_agent_routable(&state, &identity, &namespace, &tenant).await
    {
        return (status, version_header(), body).into_response();
    }
    let scope = TaskScope::new(&namespace, &tenant);
    let engine = task_engine(&state).await;
    let actor = if identity.id.is_empty() {
        identity.auth_method.as_str()
    } else {
        identity.id.as_str()
    };
    if parsed
        .get("method")
        .and_then(Value::as_str)
        .is_some_and(|method| matches!(method, "SendStreamingMessage" | "SubscribeToTask"))
    {
        return handle_v1_streaming_rpc(&state, &engine, &scope, &parsed, actor).await;
    }
    match handle_rpc_payload_as(Some(&state), &engine, &scope, parsed, actor).await {
        RpcReply::Single(resp) => (StatusCode::OK, version_header(), Json(resp)).into_response(),
        RpcReply::Batch(resps) => (StatusCode::OK, version_header(), Json(resps)).into_response(),
        // Notification(s) only — JSON-RPC 2.0 says answer with no body.
        RpcReply::Empty => (StatusCode::NO_CONTENT, version_header()).into_response(),
    }
}

fn payload_requires_management(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(payload_requires_management),
        // Explicit allowlist: a newly implemented RPC method cannot inherit
        // execution permission just because it shares this HTTP endpoint.
        Value::Object(obj) => {
            let method = obj.get("method").and_then(Value::as_str);
            let inline_push = matches!(method, Some("SendMessage" | "SendStreamingMessage"))
                && obj
                    .get("params")
                    .and_then(|params| params.pointer("/configuration/taskPushNotificationConfig"))
                    .is_some();
            inline_push
                || method.is_some_and(|method| {
                    !matches!(
                        method,
                        "message/send"
                            | "SendMessage"
                            | "SendStreamingMessage"
                            | "tasks/get"
                            | "GetTask"
                            | "ListTasks"
                            | "tasks/cancel"
                            | "CancelTask"
                            | "agent/getAuthenticatedExtendedCard"
                            | "GetExtendedAgentCard"
                            | "SubscribeToTask"
                    )
                })
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------
// REST binding (A2A spec §11)
// ---------------------------------------------------------------------

/// Render a method result for the REST binding: the task as JSON on
/// success, or the mapped HTTP status + error body on failure.
fn rest_result(outcome: Result<Task, A2aError>) -> Response {
    match outcome {
        Ok(task) => (StatusCode::OK, version_header(), Json(task)).into_response(),
        Err(e) => (
            e.http_status(),
            version_header(),
            Json(ErrorResponse { error: e.message }),
        )
            .into_response(),
    }
}

fn versioned_task_response(task: Task, version: u64, history_length: Option<usize>) -> Response {
    let mut response = (StatusCode::OK, version_header(), Json(task)).into_response();
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&task_cursor(version, history_length))
            .expect("fixed task cursor is a valid header"),
    );
    response
}

fn task_not_modified_response(version: u64, history_length: Option<usize>) -> Response {
    let mut response = (StatusCode::NOT_MODIFIED, version_header()).into_response();
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&task_cursor(version, history_length))
            .expect("fixed task cursor is a valid header"),
    );
    response
}

/// `POST /a2a/v1/message:send` — REST binding for
/// `message/send`.
pub async fn a2a_rest_message_send(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    crate::auth::context::ScopedPath((namespace, tenant)): crate::auth::context::ScopedPath<(
        String,
        String,
    )>,
    headers: HeaderMap,
    Json(params): Json<MessageSendParams>,
) -> Response {
    if let Err(e) = negotiate_version(&headers) {
        return rest_result(Err(e));
    }
    if let Err((status, body)) = authorize(&identity, &namespace, &tenant) {
        return (status, version_header(), body).into_response();
    }
    if let Err((status, body)) =
        enforce_agent_routable(&state, &identity, &namespace, &tenant).await
    {
        return (status, version_header(), body).into_response();
    }
    let scope = TaskScope::new(&namespace, &tenant);
    let engine = task_engine(&state).await;
    let actor = if identity.id.is_empty() {
        identity.auth_method.as_str()
    } else {
        identity.id.as_str()
    };
    let task = match method_message_send_as(&engine, &scope, params, actor).await {
        Ok(task) => task,
        Err(error) => return rest_result(Err(error)),
    };
    match engine.get_task_versioned(&scope, &task.id).await {
        Ok(Some((task, version))) => versioned_task_response(task, version, None),
        Ok(None) => rest_result(Err(A2aError::task_not_found(&task.id))),
        Err(error) => rest_result(Err(error.into())),
    }
}

/// Query string for the REST `tasks/get`.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaskGetQuery {
    #[serde(default)]
    history_length: Option<usize>,
}

/// `GET /a2a/v1/tasks/{id}` — REST binding for
/// `tasks/get`. `?historyLength=N` trims the returned history.
pub async fn a2a_rest_task_get(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    crate::auth::context::ScopedPath((namespace, tenant, id)): crate::auth::context::ScopedPath<(
        String,
        String,
        String,
    )>,
    headers: HeaderMap,
    Query(query): Query<TaskGetQuery>,
) -> Response {
    if let Err(e) = negotiate_version(&headers) {
        return rest_result(Err(e));
    }
    if let Err((status, body)) = authorize(&identity, &namespace, &tenant) {
        return (status, version_header(), body).into_response();
    }
    let scope = TaskScope::new(&namespace, &tenant);
    let engine = task_engine(&state).await;
    let Some((mut task, version)) = (match engine.get_task_versioned(&scope, &id).await {
        Ok(task) => task,
        Err(error) => return rest_result(Err(error.into())),
    }) else {
        return rest_result(Err(A2aError::task_not_found(&id)));
    };
    let history_length = query.history_length;
    let cursor = task_cursor(version, history_length);
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == cursor)
    {
        return task_not_modified_response(version, history_length);
    }
    if let Some(limit) = history_length
        && task.history.len() > limit
    {
        let drop = task.history.len() - limit;
        task.history.drain(0..drop);
    }
    versioned_task_response(task, version, history_length)
}

/// `POST /a2a/v1/tasks/{id}:cancel` — REST
/// binding for `tasks/cancel`.
///
/// A2A spec §11 spells this `tasks/{id}:cancel`. axum's router matches
/// whole path segments, so the final segment — `{id}:cancel` — is
/// captured intact and the `:cancel` verb suffix is split off here.
/// Task ids are `[A-Za-z0-9._-]` (no `:`), so the split is
/// unambiguous; a final segment that is not `<id>:cancel` is a
/// method-not-found.
pub async fn a2a_rest_task_cancel(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    crate::auth::context::ScopedPath((namespace, tenant, id_and_verb)): crate::auth::context::ScopedPath<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = negotiate_version(&headers) {
        return rest_result(Err(e));
    }
    if let Err((status, body)) = authorize(&identity, &namespace, &tenant) {
        return (status, version_header(), body).into_response();
    }
    if let Err((status, body)) =
        enforce_agent_routable(&state, &identity, &namespace, &tenant).await
    {
        return (status, version_header(), body).into_response();
    }
    let Some(task_id) = id_and_verb.strip_suffix(":cancel") else {
        return rest_result(Err(A2aError::new(
            METHOD_NOT_FOUND,
            format!("unknown task action '{id_and_verb}' — expected '<id>:cancel'"),
        )));
    };
    let scope = TaskScope::new(&namespace, &tenant);
    let engine = task_engine(&state).await;
    rest_result(
        method_tasks_cancel(
            Some(&state),
            &engine,
            &scope,
            TaskIdParams {
                id: task_id.to_string(),
            },
        )
        .await,
    )
}

/// `GET /a2a/v1/tasks/{id}/events` — SSE stream
/// of task-lifecycle events.
///
/// Emits `TaskTransitioned`, `TaskHistoryAppended`, and
/// `TaskArtifactUpdated` envelopes for a single task, by subscribing to
/// the gateway-wide broadcast and filtering by `action_id == task_id`.
///
/// Per-tenant concurrent-connection caps come from the same
/// `ConnectionRegistry` that backs `/v1/stream`, so a tenant cannot
/// open more SSE connections via A2A than via the existing transport.
/// Task events are not persisted to the audit store, so `Last-Event-ID`
/// catch-up is intentionally not supported on this endpoint.
pub async fn a2a_task_events(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    crate::auth::context::ScopedPath((namespace, tenant, task_id)): crate::auth::context::ScopedPath<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = negotiate_version(&headers) {
        return rest_result(Err(e));
    }
    if let Err((status, body)) = authorize(&identity, &namespace, &tenant) {
        return (status, version_header(), body).into_response();
    }

    // 404 if the task does not exist — same semantics as the REST GET,
    // so a probe-then-subscribe race can't trick the endpoint into
    // returning an empty SSE stream for a non-existent task.
    let scope = TaskScope::new(&namespace, &tenant);
    let engine = task_engine(&state).await;
    match engine.get_task(&scope, &task_id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return rest_result(Err(A2aError::new(
                TASK_NOT_FOUND,
                format!("task '{task_id}' not found"),
            )));
        }
        Err(e) => return rest_result(Err(e.into())),
    }

    // Per-tenant connection cap — reuses the registry that backs the
    // long-standing `/v1/stream` endpoint. The bucket is keyed by
    // namespace+tenant so an A2A streamer counts against the same
    // shared budget as the rest of that tenant's SSE consumers.
    let Some(conn_registry) = state.connection_registry.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            version_header(),
            Json(ErrorResponse {
                error: "SSE streaming is not enabled".to_string(),
            }),
        )
            .into_response();
    };
    let bucket = format!("{namespace}:{tenant}");
    let Some(guard) = conn_registry.try_acquire(&bucket).await else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            version_header(),
            Json(ErrorResponse {
                error: "too many concurrent SSE connections for this tenant".to_string(),
            }),
        )
            .into_response();
    };

    // Subscribe to the gateway broadcast BEFORE returning, so events
    // emitted between the existence check and the subscribe call are
    // not lost. `stream_tx().subscribe()` opens a new receiver per
    // request — bounded by tokio's broadcast channel capacity.
    let gateway = state.gateway.read().await;
    let rx = gateway.stream_tx().subscribe();
    drop(gateway);

    // Filter is the same one `/v1/stream` understands: namespace +
    // action_type + action_id pins the stream to events from *this*
    // task only. Tenant isolation runs through the `allowed_tenants`
    // arg, which `make_event_stream` enforces against `event.tenant`.
    let allowed_tenants = Some(vec![tenant.clone()]);
    let query = StreamQuery {
        namespace: Some(namespace.clone()),
        action_type: Some("a2a.task".to_string()),
        outcome: None,
        event_type: None,
        chain_id: None,
        group_id: None,
        action_id: Some(task_id.clone()),
    };

    let event_stream = make_event_stream(rx, allowed_tenants, query, guard, None);

    Sse::new(event_stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("ping"),
        )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_core::{Artifact, TaskPart, TaskRole};
    use acteon_state_memory::MemoryStateStore;
    use serde_json::json;
    use std::sync::Arc;

    fn engine() -> TaskEngine {
        TaskEngine::new(Arc::new(MemoryStateStore::new()))
    }

    fn scope() -> TaskScope {
        TaskScope::new("agents", "demo")
    }

    fn user_message(text: &str) -> TaskMessage {
        TaskMessage::text(uuid::Uuid::now_v7().to_string(), TaskRole::User, text)
    }

    /// Mint a task via `message/send` and return its id.
    async fn seed_task(e: &TaskEngine) -> String {
        method_message_send(
            e,
            &scope(),
            MessageSendParams {
                message: user_message("seed"),
            },
        )
        .await
        .unwrap()
        .id
    }

    #[tokio::test]
    async fn v1_task_stream_emits_official_frames_and_closes_at_terminal_state() {
        let (tx, _) = broadcast::channel(16);
        let e = TaskEngine::new(Arc::new(MemoryStateStore::new())).with_stream_tx(tx.clone());
        let task_id = seed_task(&e).await;
        let task = e.get_task(&scope(), &task_id).await.unwrap().unwrap();
        let rx = tx.subscribe();
        let registry = super::super::stream::ConnectionRegistry::new(1);
        let guard = registry.try_acquire("agents:demo").await.unwrap();
        let initial_task = project_v1_task(&task).unwrap();
        let stream = make_v1_task_stream(
            rx,
            e.clone(),
            scope(),
            V1TaskStreamSeed {
                task_id: task.id,
                initial_task,
                request_id: json!(41),
                close_on_interrupt: false,
            },
            guard,
        );
        let response = Sse::new(stream).into_response();
        let transition_engine = e.clone();
        let transition_task_id = task_id.clone();
        let transitions = tokio::spawn(async move {
            tokio::task::yield_now().await;
            transition_engine
                .transition_task(&scope(), &transition_task_id, TaskState::Working, None)
                .await
                .unwrap();
            transition_engine
                .transition_task(
                    &scope(),
                    &transition_task_id,
                    TaskState::InputRequired,
                    None,
                )
                .await
                .unwrap();
            transition_engine
                .transition_task(&scope(), &transition_task_id, TaskState::Working, None)
                .await
                .unwrap();
            transition_engine
                .transition_task(&scope(), &transition_task_id, TaskState::Completed, None)
                .await
                .unwrap();
        });
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        transitions.await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("\"id\":41"));
        assert!(body.contains("\"task\""));
        assert!(body.contains("TASK_STATE_SUBMITTED"));
        assert!(body.contains("\"statusUpdate\""));
        assert!(body.contains("TASK_STATE_WORKING"));
        assert!(body.contains("TASK_STATE_INPUT_REQUIRED"));
        assert!(body.contains("TASK_STATE_COMPLETED"));
        assert!(!body.contains("\"final\""));
    }

    #[tokio::test]
    async fn v1_send_stream_closes_at_an_interrupted_state() {
        let (tx, _) = broadcast::channel(16);
        let e = TaskEngine::new(Arc::new(MemoryStateStore::new())).with_stream_tx(tx.clone());
        let task_id = seed_task(&e).await;
        let task = e.get_task(&scope(), &task_id).await.unwrap().unwrap();
        let initial_task = project_v1_task(&task).unwrap();
        let registry = super::super::stream::ConnectionRegistry::new(1);
        let guard = registry.try_acquire("agents:demo").await.unwrap();
        let stream = make_v1_task_stream(
            tx.subscribe(),
            e.clone(),
            scope(),
            V1TaskStreamSeed {
                task_id: task.id,
                initial_task,
                request_id: json!(42),
                close_on_interrupt: true,
            },
            guard,
        );
        let response = Sse::new(stream).into_response();
        let transition_engine = e.clone();
        let transitions = tokio::spawn(async move {
            tokio::task::yield_now().await;
            transition_engine
                .transition_task(&scope(), &task_id, TaskState::Working, None)
                .await
                .unwrap();
            transition_engine
                .transition_task(&scope(), &task_id, TaskState::InputRequired, None)
                .await
                .unwrap();
        });
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        transitions.await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("\"id\":42"));
        assert!(body.contains("TASK_STATE_INPUT_REQUIRED"));
        assert!(!body.contains("TASK_STATE_COMPLETED"));
    }

    #[tokio::test]
    async fn v1_send_message_uses_pascal_method_and_protojson_envelope() {
        let e = engine();
        let result = dispatch_method(
            None,
            &e,
            &scope(),
            "SendMessage",
            json!({
                "message": {
                    "messageId": "official-client-message-1",
                    "role": "ROLE_USER",
                    "parts": [{"text": "hello from the official SDK"}]
                },
                "configuration": {"returnImmediately": true}
            }),
            "official-client",
        )
        .await
        .unwrap();

        assert_eq!(result["task"]["status"]["state"], "TASK_STATE_SUBMITTED");
        assert_eq!(result["task"]["history"][0]["role"], "ROLE_USER");
        assert!(
            result.get("id").is_none(),
            "SendMessage must return its oneof wrapper"
        );
        for private in [
            "namespace",
            "tenant",
            "createdAt",
            "updatedAt",
            "lastProgressAt",
            "workingTtlMs",
            "pendingApprovalId",
            "chainId",
            "artifactStreams",
        ] {
            assert!(
                result["task"].get(private).is_none(),
                "leaked internal Task field {private}"
            );
        }
    }

    #[tokio::test]
    async fn v1_send_message_waits_without_return_immediately() {
        let e = engine();
        let task_id = seed_task(&e).await;
        let transition_engine = e.clone();
        let transition_id = task_id.clone();
        let transition = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            transition_engine
                .transition_task(&scope(), &transition_id, TaskState::Working, None)
                .await
                .unwrap();
            transition_engine
                .transition_task(&scope(), &transition_id, TaskState::Completed, None)
                .await
                .unwrap();
        });

        let result = dispatch_method(
            None,
            &e,
            &scope(),
            "SendMessage",
            json!({
                "message": {
                    "messageId": "official-client-message-blocking",
                    "taskId": task_id,
                    "role": "ROLE_USER",
                    "parts": [{"text": "continue and wait"}]
                }
            }),
            "official-client",
        )
        .await
        .unwrap();
        transition.await.unwrap();
        assert_eq!(result["task"]["status"]["state"], "TASK_STATE_COMPLETED");
    }

    #[test]
    fn v1_send_message_validates_return_immediately() {
        let error = v1_return_immediately(&json!({
            "configuration": {"returnImmediately": "yes"}
        }))
        .unwrap_err();
        assert_eq!(error.code, INVALID_PARAMS);
        assert!(!v1_return_immediately(&json!({})).unwrap());
        assert!(
            v1_return_immediately(&json!({
                "configuration": {"returnImmediately": true}
            }))
            .unwrap()
        );
    }

    #[tokio::test]
    async fn v1_list_tasks_paginates_with_filter_bound_tokens() {
        let e = engine();
        let first_id = seed_task(&e).await;
        let second_id = seed_task(&e).await;
        e.upsert_artifact(
            &scope(),
            &first_id,
            Artifact::new("result", vec![TaskPart::text("governed result")]),
            false,
        )
        .await
        .unwrap();
        let expected_ids = [first_id, second_id];

        let first_page = dispatch_method(
            None,
            &e,
            &scope(),
            "ListTasks",
            json!({"pageSize": 1, "historyLength": 1}),
            "official-client",
        )
        .await
        .unwrap();

        assert_eq!(first_page["pageSize"], 1);
        assert_eq!(first_page["totalSize"], 2);
        assert_eq!(first_page["tasks"].as_array().unwrap().len(), 1);
        assert_eq!(first_page["tasks"][0]["id"], expected_ids[0]);
        assert_eq!(
            first_page["tasks"][0]["status"]["state"],
            "TASK_STATE_SUBMITTED"
        );
        assert_eq!(first_page["tasks"][0]["history"][0]["role"], "ROLE_USER");
        assert!(first_page["tasks"][0].get("namespace").is_none());
        assert!(first_page["tasks"][0].get("artifacts").is_none());
        let token = first_page["nextPageToken"].as_str().unwrap();
        assert!(!token.is_empty());

        let second_page = dispatch_method(
            None,
            &e,
            &scope(),
            "ListTasks",
            json!({
                "pageSize": 1,
                "pageToken": token,
                "historyLength": 1,
            }),
            "official-client",
        )
        .await
        .unwrap();
        assert_eq!(second_page["tasks"][0]["id"], expected_ids[1]);
        assert_eq!(second_page["totalSize"], 2);
        assert_eq!(second_page["nextPageToken"], "");

        let with_artifacts = dispatch_method(
            None,
            &e,
            &scope(),
            "ListTasks",
            json!({"includeArtifacts": true}),
            "official-client",
        )
        .await
        .unwrap();
        let task_with_artifact = with_artifacts["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["id"] == expected_ids[0])
            .unwrap();
        assert_eq!(task_with_artifact["artifacts"][0]["artifactId"], "result");

        let mismatch = dispatch_method(
            None,
            &e,
            &scope(),
            "ListTasks",
            json!({
                "pageSize": 1,
                "pageToken": token,
                "historyLength": 0,
            }),
            "official-client",
        )
        .await
        .unwrap_err();
        assert_eq!(mismatch.code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn v1_list_tasks_rejects_invalid_page_size_and_state() {
        let e = engine();
        for params in [
            json!({"pageSize": 0}),
            json!({"pageSize": 101}),
            json!({"status": "submitted"}),
            json!({"pageToken": "not-a-cursor"}),
            json!({"tenant": "another-tenant"}),
        ] {
            let error = dispatch_method(None, &e, &scope(), "ListTasks", params, "official-client")
                .await
                .unwrap_err();
            assert_eq!(error.code, INVALID_PARAMS);
        }

        let unspecified = dispatch_method(
            None,
            &e,
            &scope(),
            "ListTasks",
            json!({"status": "TASK_STATE_UNSPECIFIED"}),
            "official-client",
        )
        .await
        .unwrap();
        assert_eq!(unspecified["totalSize"], 0);
    }

    #[tokio::test]
    async fn v1_methods_reject_a_tenant_outside_the_selected_interface() {
        let e = engine();
        for method in ["SendMessage", "GetTask", "CancelTask"] {
            let error = dispatch_method(
                None,
                &e,
                &scope(),
                method,
                json!({"tenant": "another-tenant"}),
                "official-client",
            )
            .await
            .unwrap_err();
            assert_eq!(error.code, INVALID_PARAMS);
            assert!(error.message.contains("tenant"));
        }
    }

    #[tokio::test]
    async fn message_send_mints_a_new_task() {
        let e = engine();
        let params = MessageSendParams {
            message: user_message("hello"),
        };
        let task = method_message_send(&e, &scope(), params).await.unwrap();
        assert_eq!(task.status.state, TaskState::Submitted);
        assert_eq!(task.history.len(), 1);
        let got = e.get_task(&scope(), &task.id).await.unwrap().unwrap();
        assert_eq!(got.id, task.id);
        assert_eq!(got.history[0].task_id.as_deref(), Some(task.id.as_str()));
    }

    #[tokio::test]
    async fn message_send_continues_an_existing_task() {
        let e = engine();
        let mut first_message = user_message("first");
        first_message.context_id = Some("context-1".into());
        let first = method_message_send(
            &e,
            &scope(),
            MessageSendParams {
                message: first_message,
            },
        )
        .await
        .unwrap();
        let mut follow = user_message("second");
        follow.task_id = Some(first.id.clone());
        let task = method_message_send(&e, &scope(), MessageSendParams { message: follow })
            .await
            .unwrap();
        assert_eq!(task.id, first.id);
        assert_eq!(task.history.len(), 2);
        assert_eq!(task.history[1].context_id.as_deref(), Some("context-1"));
    }

    #[tokio::test]
    async fn rejected_continuation_does_not_register_inline_push_config() {
        let e = engine();
        let task_id = seed_task(&e).await;
        e.transition_task(&scope(), &task_id, TaskState::Canceled, None)
            .await
            .unwrap();
        let mut message = user_message("too late");
        message.task_id = Some(task_id.clone());
        let error = method_message_send_as_with_push(
            &e,
            &scope(),
            MessageSendParams { message },
            "test",
            Some(super::super::a2a_push::SetPushConfigInput {
                id: Some("must-not-exist".into()),
                url: "https://callbacks.example/a2a".into(),
                token: None,
                token_ref: None,
                authentication: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, INVALID_PARAMS);
        assert!(
            e.get_push_config(&scope(), &task_id, "must-not-exist")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn message_send_resolves_exact_input_challenge() {
        let e = engine();
        let task_id = seed_task(&e).await;
        e.transition_task(&scope(), &task_id, TaskState::Working, None)
            .await
            .unwrap();
        let (_, challenge) = e
            .pause_for_human(
                &scope(),
                &task_id,
                acteon_core::PauseKind::UserInput,
                Some("which region?".into()),
                None,
            )
            .await
            .unwrap();
        let mut response = user_message("us-west-2");
        response.task_id = Some(task_id.clone());
        response
            .metadata
            .insert("acteon.challengeId".into(), json!(challenge.approval_id));

        let task = method_message_send(&e, &scope(), MessageSendParams { message: response })
            .await
            .unwrap();
        assert_eq!(task.status.state, TaskState::Working);
        assert!(task.pending_approval_id.is_none());
        assert_eq!(
            task.history.last().unwrap().parts[0].text.as_deref(),
            Some("us-west-2")
        );
    }

    #[tokio::test]
    async fn message_send_requires_exact_input_challenge_id() {
        let e = engine();
        let task_id = seed_task(&e).await;
        e.transition_task(&scope(), &task_id, TaskState::Working, None)
            .await
            .unwrap();
        e.pause_for_human(
            &scope(),
            &task_id,
            acteon_core::PauseKind::UserInput,
            None,
            None,
        )
        .await
        .unwrap();
        let mut response = user_message("answer");
        response.task_id = Some(task_id);
        let error = method_message_send(&e, &scope(), MessageSendParams { message: response })
            .await
            .unwrap_err();
        assert_eq!(error.code, INVALID_PARAMS);
        assert!(error.message.contains("acteon.challengeId"));
    }

    #[tokio::test]
    async fn message_send_cannot_satisfy_auth_with_message_content() {
        let e = engine();
        let task_id = seed_task(&e).await;
        e.transition_task(&scope(), &task_id, TaskState::Working, None)
            .await
            .unwrap();
        e.pause_for_authorization(
            &scope(),
            &task_id,
            acteon_core::TaskAuthorizationRequirement {
                verifier_id: "test-verifier".into(),
                verifier_revision: 1,
                authorization_request_id: "request-1".into(),
                recipient: acteon_core::PrincipalIdentity::new(
                    "test-agent",
                    acteon_core::PrincipalKind::Agent,
                )
                .unwrap(),
                credential_authority: "test-authority".into(),
                audience: "test-audience".into(),
                required_scopes: vec!["read".into()],
            },
            None,
            None,
        )
        .await
        .unwrap();
        let mut response = user_message("bearer secret");
        response.task_id = Some(task_id.clone());
        let error = method_message_send(&e, &scope(), MessageSendParams { message: response })
            .await
            .unwrap_err();
        assert_eq!(error.code, INVALID_PARAMS);
        assert!(error.message.contains("trusted authorization flow"));
        assert_eq!(
            e.get_task(&scope(), &task_id)
                .await
                .unwrap()
                .unwrap()
                .status
                .state,
            TaskState::AuthRequired
        );
    }

    #[tokio::test]
    async fn message_send_rejects_empty_parts() {
        let e = engine();
        let mut msg = user_message("x");
        msg.parts.clear();
        let err = method_message_send(&e, &scope(), MessageSendParams { message: msg })
            .await
            .unwrap_err();
        assert_eq!(err.code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn tasks_get_returns_task_and_trims_history() {
        let e = engine();
        let task = method_message_send(
            &e,
            &scope(),
            MessageSendParams {
                message: user_message("one"),
            },
        )
        .await
        .unwrap();
        let mut second = user_message("two");
        second.task_id = Some(task.id.clone());
        method_message_send(&e, &scope(), MessageSendParams { message: second })
            .await
            .unwrap();

        let full = method_tasks_get(
            &e,
            &scope(),
            TaskQueryParams {
                id: task.id.clone(),
                history_length: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(full.history.len(), 2);

        let trimmed = method_tasks_get(
            &e,
            &scope(),
            TaskQueryParams {
                id: task.id.clone(),
                history_length: Some(1),
            },
        )
        .await
        .unwrap();
        assert_eq!(trimmed.history.len(), 1);
        assert_eq!(trimmed.history[0].parts[0].text.as_deref(), Some("two"));
    }

    #[tokio::test]
    async fn tasks_get_missing_is_task_not_found() {
        let e = engine();
        let err = method_tasks_get(
            &e,
            &scope(),
            TaskQueryParams {
                id: "ghost".into(),
                history_length: None,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, TASK_NOT_FOUND);
        assert_eq!(err.http_status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn tasks_cancel_cancels_a_live_task() {
        let e = engine();
        let id = seed_task(&e).await;
        let canceled = method_tasks_cancel(None, &e, &scope(), TaskIdParams { id: id.clone() })
            .await
            .unwrap();
        assert_eq!(canceled.status.state, TaskState::Canceled);
    }

    #[tokio::test]
    async fn tasks_cancel_on_terminal_is_not_cancelable() {
        let e = engine();
        let id = seed_task(&e).await;
        method_tasks_cancel(None, &e, &scope(), TaskIdParams { id: id.clone() })
            .await
            .unwrap();
        let err = method_tasks_cancel(None, &e, &scope(), TaskIdParams { id: id.clone() })
            .await
            .unwrap_err();
        assert_eq!(err.code, TASK_NOT_CANCELABLE);
        assert_eq!(err.http_status(), StatusCode::CONFLICT);
    }

    #[test]
    fn version_negotiation() {
        let mut h = HeaderMap::new();
        assert!(negotiate_version(&h).is_ok()); // absent — accepted
        h.insert(A2A_VERSION_HEADER, A2A_PROTOCOL_VERSION.parse().unwrap());
        assert!(negotiate_version(&h).is_ok()); // matching — accepted
        h.insert(A2A_VERSION_HEADER, "0.1".parse().unwrap());
        let err = negotiate_version(&h).unwrap_err();
        assert_eq!(err.code, VERSION_NOT_SUPPORTED);
        assert_eq!(err.http_status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn engine_error_maps_to_a2a_error() {
        let nf: A2aError = TaskEngineError::NotFound("t1".into()).into();
        assert_eq!(nf.code, TASK_NOT_FOUND);

        let cas: A2aError = TaskEngineError::CasExhausted("t1".into()).into();
        assert_eq!(cas.code, INTERNAL_ERROR);

        // A serde failure must not leak its detail to the caller
        // (CWE-209) — the message is opaque.
        let serde_err = serde_json::from_str::<i32>("not-a-number").unwrap_err();
        let masked: A2aError = TaskEngineError::Serde(serde_err).into();
        assert_eq!(masked.code, INTERNAL_ERROR);
        assert_eq!(masked.message, "internal error");
    }

    #[tokio::test]
    async fn rpc_notification_gets_no_response() {
        let e = engine();
        let id = seed_task(&e).await;
        // A request object with no `id` member is a notification.
        let payload = json!({
            "jsonrpc": "2.0",
            "method": "tasks/get",
            "params": { "id": id },
        });
        let reply = handle_rpc_payload(None, &e, &scope(), payload).await;
        assert!(matches!(reply, RpcReply::Empty));
    }

    #[tokio::test]
    async fn rpc_request_with_null_id_still_answered() {
        let e = engine();
        let id = seed_task(&e).await;
        // A present `"id": null` is a request, not a notification.
        let payload = json!({
            "jsonrpc": "2.0",
            "method": "tasks/get",
            "params": { "id": id },
            "id": null,
        });
        match handle_rpc_payload(None, &e, &scope(), payload).await {
            RpcReply::Single(resp) => {
                assert_eq!(resp.id, Value::Null);
                assert!(resp.result.is_some());
            }
            other => panic!("expected a single response, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rpc_batch_mixes_requests_and_notifications() {
        let e = engine();
        let id = seed_task(&e).await;
        let payload = json!([
            { "jsonrpc": "2.0", "method": "tasks/get", "params": {"id": id}, "id": 1 },
            // notification — no `id`, processed but not answered
            { "jsonrpc": "2.0", "method": "tasks/get", "params": {"id": id} },
            { "jsonrpc": "2.0", "method": "tasks/get", "params": {"id": id}, "id": 2 },
        ]);
        match handle_rpc_payload(None, &e, &scope(), payload).await {
            RpcReply::Batch(resps) => {
                // Two id-bearing requests answered; the notification omitted.
                assert_eq!(resps.len(), 2);
                assert_eq!(resps[0].id, json!(1));
                assert_eq!(resps[1].id, json!(2));
            }
            other => panic!("expected a batch, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rpc_batch_of_only_notifications_is_empty() {
        let e = engine();
        let id = seed_task(&e).await;
        let payload = json!([
            { "jsonrpc": "2.0", "method": "tasks/get", "params": {"id": id} },
            { "jsonrpc": "2.0", "method": "tasks/get", "params": {"id": id} },
        ]);
        assert!(matches!(
            handle_rpc_payload(None, &e, &scope(), payload).await,
            RpcReply::Empty
        ));
    }

    #[tokio::test]
    async fn rpc_empty_batch_is_invalid_request() {
        let e = engine();
        match handle_rpc_payload(None, &e, &scope(), json!([])).await {
            RpcReply::Single(resp) => {
                assert_eq!(resp.error.unwrap().code, INVALID_REQUEST);
            }
            other => panic!("expected a single error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rpc_unknown_method_is_method_not_found() {
        let e = engine();
        let payload = json!({
            "jsonrpc": "2.0",
            "method": "tasks/teleport",
            "params": {},
            "id": 9,
        });
        match handle_rpc_payload(None, &e, &scope(), payload).await {
            RpcReply::Single(resp) => {
                assert_eq!(resp.error.unwrap().code, METHOD_NOT_FOUND);
            }
            other => panic!("expected a single error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn v1_unsupported_streaming_has_typed_error_details() {
        let e = engine();
        let payload = json!({
            "jsonrpc": "2.0",
            "method": "SubscribeToTask",
            "params": {"id": "task-1"},
            "id": 10,
        });
        let RpcReply::Single(response) = handle_rpc_payload(None, &e, &scope(), payload).await
        else {
            panic!("expected a single response");
        };
        let error = response.error.unwrap();
        assert_eq!(error.code, UNSUPPORTED_OPERATION);
        assert_eq!(error.data.unwrap()[0]["reason"], "UNSUPPORTED_OPERATION");
    }

    #[tokio::test]
    async fn rpc_extended_card_without_app_state_is_internal_error() {
        // The method needs an `AppState` for the discovery store. With
        // `state = None` the dispatcher must surface an honest
        // InternalError, not a fabricated empty card.
        let e = engine();
        let payload = json!({
            "jsonrpc": "2.0",
            "method": "agent/getAuthenticatedExtendedCard",
            "id": 1,
        });
        match handle_rpc_payload(None, &e, &scope(), payload).await {
            RpcReply::Single(resp) => {
                let err = resp.error.expect("error envelope");
                assert_eq!(err.code, INTERNAL_ERROR);
                // The message must NOT echo serde details or stack
                // info — see [`engine_error_maps_to_a2a_error`].
                assert!(
                    !err.message.contains("AppState"),
                    "error message should not leak internal type names: {}",
                    err.message
                );
            }
            other => panic!("expected a single error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rpc_push_config_methods_without_app_state_are_internal_error() {
        // Each of the four push-config methods needs the `AppState`'s
        // state store. With `state = None` the dispatcher must surface
        // `InternalError` — never a silent no-op.
        let e = engine();
        for (method, params) in &[
            (
                "tasks/pushNotificationConfig/set",
                json!({"taskId": "t1", "pushNotificationConfig": {"url": "https://x"}}),
            ),
            (
                "tasks/pushNotificationConfig/get",
                json!({"taskId": "t1", "pushNotificationConfigId": "c1"}),
            ),
            ("tasks/pushNotificationConfig/list", json!({"taskId": "t1"})),
            (
                "tasks/pushNotificationConfig/delete",
                json!({"taskId": "t1", "pushNotificationConfigId": "c1"}),
            ),
            (
                "CreateTaskPushNotificationConfig",
                json!({"taskId": "t1", "url": "https://example.com/push"}),
            ),
            (
                "GetTaskPushNotificationConfig",
                json!({"taskId": "t1", "id": "c1"}),
            ),
            ("ListTaskPushNotificationConfigs", json!({"taskId": "t1"})),
            (
                "DeleteTaskPushNotificationConfig",
                json!({"taskId": "t1", "id": "c1"}),
            ),
        ] {
            let payload = json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params,
                "id": 1,
            });
            let RpcReply::Single(resp) = handle_rpc_payload(None, &e, &scope(), payload).await
            else {
                panic!("expected a single response for {method}");
            };
            let err = resp.error.unwrap_or_else(|| {
                panic!("{method}: expected an error envelope, got a success");
            });
            assert_eq!(
                err.code, INTERNAL_ERROR,
                "{method}: expected INTERNAL_ERROR for no-AppState path"
            );
        }
    }

    #[test]
    fn v1_push_config_projection_is_schema_closed() {
        let mut config = acteon_core::TaskPushNotificationConfig::new(
            "config-1",
            "task-1",
            "agents",
            "demo",
            "https://example.com/push",
        );
        config.token = Some("push-token".into());
        config.authentication = Some(acteon_core::PushAuthentication {
            schemes: vec!["Bearer".into()],
            credentials: Some("push-secret".into()),
            credentials_ref: None,
        });
        let value = project_v1_push_config(&config).unwrap();
        assert_eq!(value["id"], "config-1");
        assert_eq!(value["taskId"], "task-1");
        assert_eq!(value["authentication"]["scheme"], "Bearer");
        for private in ["namespace", "tenant", "createdAt", "updatedAt"] {
            assert!(
                value.get(private).is_none(),
                "leaked private field {private}"
            );
        }

        let inline = v1_inline_push_input(
            &json!({
                "configuration": {
                    "taskPushNotificationConfig": {
                        "id": "inline-1",
                        "url": "https://example.com/inline",
                        "authentication": {"scheme": "Bearer"}
                    }
                }
            }),
            &scope(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(inline.id.as_deref(), Some("inline-1"));
        assert_eq!(
            inline.authentication.unwrap().schemes,
            vec!["Bearer".to_string()]
        );

        let blocked = crate::api::a2a_push::SetPushConfigInput {
            id: None,
            url: "http://127.0.0.1/push".into(),
            token: None,
            token_ref: None,
            authentication: None,
        };
        assert_eq!(
            validate_inline_push_before_task(&scope(), &blocked)
                .unwrap_err()
                .code,
            INVALID_PARAMS
        );

        let error = v1_inline_push_input(
            &json!({
                "configuration": {
                    "taskPushNotificationConfig": {
                        "taskId": "caller-bound-task",
                        "url": "https://example.com/inline"
                    }
                }
            }),
            &scope(),
        )
        .unwrap_err();
        assert_eq!(error.code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn rpc_push_config_set_invalid_params_is_invalid_params() {
        // The DTO requires `taskId` + `pushNotificationConfig`. A
        // payload missing `taskId` is caught at serde_from_value time
        // before the dispatcher reaches the state-store path, so the
        // error reports INVALID_PARAMS regardless of state availability.
        let e = engine();
        let payload = json!({
            "jsonrpc": "2.0",
            "method": "tasks/pushNotificationConfig/set",
            "params": {"pushNotificationConfig": {"url": "https://x"}},
            "id": 1,
        });
        let RpcReply::Single(resp) = handle_rpc_payload(None, &e, &scope(), payload).await else {
            panic!("expected a single response");
        };
        assert_eq!(resp.error.unwrap().code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn rpc_bad_jsonrpc_version_is_invalid_request() {
        let e = engine();
        let payload = json!({ "jsonrpc": "1.0", "method": "tasks/get", "id": 1 });
        match handle_rpc_payload(None, &e, &scope(), payload).await {
            RpcReply::Single(resp) => {
                assert_eq!(resp.error.unwrap().code, INVALID_REQUEST);
            }
            other => panic!("expected a single error, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // Codec fuzzing (Phase 5)
    //
    // Property tests guaranteeing the JSON-RPC + REST codec layers
    // never panic on malformed input — generated `serde_json::Value`
    // trees, raw arbitrary strings, and arbitrary param-struct
    // payloads. "No panic" is the whole contract: a malformed A2A
    // request must always degrade into a structured error, never
    // crash the worker thread.
    // -----------------------------------------------------------------
    mod fuzz {
        use super::*;
        use proptest::prelude::*;

        /// Recursive arbitrary `serde_json::Value`. Leaf values
        /// deliberately include JSON-RPC-meaningful strings (method
        /// names, envelope keys) so a fraction of the generated trees
        /// look like real envelopes and exercise the *dispatch* path
        /// rather than only the `InvalidRequest` reject.
        fn arb_json() -> impl Strategy<Value = serde_json::Value> {
            let leaf = prop_oneof![
                Just(serde_json::Value::Null),
                any::<bool>().prop_map(serde_json::Value::Bool),
                any::<i64>().prop_map(|n| serde_json::json!(n)),
                // f64 must stay finite — NaN / ±Inf aren't valid JSON
                // and `serde_json::json!` would refuse to build them.
                any::<f64>()
                    .prop_filter("finite", |f| f.is_finite())
                    .prop_map(|f| serde_json::json!(f)),
                // `\PC` = any non-control char; arbitrary free strings.
                "\\PC*".prop_map(serde_json::Value::String),
                // Biased toward strings that mean something to the
                // codec so the dispatcher's real branches get hit.
                prop::sample::select(vec![
                    "2.0",
                    "1.0",
                    "jsonrpc",
                    "method",
                    "params",
                    "id",
                    "message/send",
                    "tasks/get",
                    "tasks/cancel",
                    "tasks/pushNotificationConfig/set",
                    "tasks/pushNotificationConfig/list",
                    "agent/getAuthenticatedExtendedCard",
                    "message",
                    "messageId",
                    "taskId",
                    "role",
                    "user",
                    "pushNotificationConfig",
                ])
                .prop_map(|s| serde_json::Value::String(s.to_string())),
            ];
            // Depth 4, up to 48 total nodes, up to 6 children each —
            // deep enough to exercise nested params, bounded enough
            // that 256 cases stay fast.
            leaf.prop_recursive(4, 48, 6, |inner| {
                prop_oneof![
                    prop::collection::vec(inner.clone(), 0..6).prop_map(serde_json::Value::Array),
                    prop::collection::vec(("\\PC*", inner), 0..6)
                        .prop_map(|kvs| { serde_json::Value::Object(kvs.into_iter().collect()) }),
                ]
            })
        }

        /// proptest bodies are synchronous; the codec is async. A
        /// fresh current-thread runtime per case keeps the cases
        /// fully isolated and costs sub-millisecond to build.
        fn block_on<F: std::future::Future>(fut: F) -> F::Output {
            tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("build current-thread runtime")
                .block_on(fut)
        }

        proptest! {
            /// The JSON-RPC codec must never panic on an arbitrary
            /// parsed `Value`. Every input — object, array (batch),
            /// scalar, deeply nested — must map to an `RpcReply`.
            #[test]
            fn handle_rpc_payload_never_panics(v in arb_json()) {
                block_on(async {
                    let e = engine();
                    // state = None: the AppState-backed methods take
                    // their honest-error branch; message/send,
                    // tasks/get, tasks/cancel run against the empty
                    // engine. Either way the call must return.
                    let _reply = handle_rpc_payload(None, &e, &scope(), v).await;
                });
            }

            /// The raw-string entry path the HTTP handler uses:
            /// arbitrary bytes → `serde_json::from_str` → dispatch.
            /// Covers the `PARSE_ERROR` branch and whatever input
            /// happens to parse into a `Value`.
            #[test]
            fn raw_string_parse_then_dispatch_never_panics(s in "\\PC*") {
                block_on(async {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                        let e = engine();
                        let _ = handle_rpc_payload(None, &e, &scope(), v).await;
                    }
                });
            }

            /// Every JSON-RPC / REST param struct's `Deserialize`
            /// must be total — an arbitrary `Value` yields `Ok` or
            /// `Err`, never a panic. A panic here would be reachable
            /// straight from an unauthenticated request body.
            #[test]
            fn param_struct_deserialization_never_panics(v in arb_json()) {
                let _ = serde_json::from_value::<MessageSendParams>(v.clone());
                let _ = serde_json::from_value::<TaskQueryParams>(v.clone());
                let _ = serde_json::from_value::<TaskIdParams>(v.clone());
                let _ = serde_json::from_value::<PushConfigSetParams>(v.clone());
                let _ = serde_json::from_value::<PushConfigIdParams>(v.clone());
                let _ = serde_json::from_value::<PushConfigListParams>(v);
            }

            /// A non-array, non-object `Value` is structurally
            /// incapable of being a valid JSON-RPC request, so the
            /// codec must answer with exactly one `Single` error —
            /// never a `Batch`, never `Empty`. Pins the "scalars are
            /// Invalid Request" contract under fuzzing.
            #[test]
            fn scalar_payloads_yield_a_single_error(
                v in prop_oneof![
                    Just(serde_json::Value::Null),
                    any::<bool>().prop_map(serde_json::Value::Bool),
                    any::<i64>().prop_map(|n| serde_json::json!(n)),
                    "\\PC*".prop_map(serde_json::Value::String),
                ]
            ) {
                block_on(async {
                    let e = engine();
                    let reply = handle_rpc_payload(None, &e, &scope(), v).await;
                    prop_assert!(
                        matches!(reply, RpcReply::Single(_)),
                        "a scalar payload must yield a single error reply, got {reply:?}",
                    );
                    Ok(())
                })?;
            }
        }
    }

    // ---- operator-lifecycle gate ----------------------------------------

    #[test]
    fn write_methods_are_gated_reads_are_not() {
        assert!(is_write_method("message/send"));
        assert!(is_write_method("tasks/cancel"));
        assert!(is_write_method("SendMessage"));
        assert!(is_write_method("SendStreamingMessage"));
        assert!(is_write_method("CancelTask"));
        assert!(is_write_method("CreateTaskPushNotificationConfig"));
        assert!(is_write_method("DeleteTaskPushNotificationConfig"));
        assert!(!is_write_method("tasks/get"));
        assert!(!is_write_method("GetTask"));
        assert!(!is_write_method("agent/getAuthenticatedExtendedCard"));
        assert!(!is_write_method("unknown"));
    }

    #[test]
    fn inline_push_configuration_requires_management() {
        assert!(!payload_requires_management(&json!({
            "method": "SendMessage",
            "params": {
                "message": {"messageId": "m1", "role": "ROLE_USER", "parts": [{"text": "x"}]},
                "configuration": {"returnImmediately": true}
            }
        })));
        assert!(payload_requires_management(&json!({
            "method": "SendMessage",
            "params": {
                "message": {"messageId": "m1", "role": "ROLE_USER", "parts": [{"text": "x"}]},
                "configuration": {
                    "taskPushNotificationConfig": {"url": "https://example.com/push"}
                }
            }
        })));
        assert!(!payload_requires_management(&json!({
            "method": "SendStreamingMessage",
            "params": {
                "message": {"messageId": "m2", "role": "ROLE_USER", "parts": [{"text": "x"}]}
            }
        })));
        assert!(payload_requires_management(&json!({
            "method": "SendStreamingMessage",
            "params": {
                "message": {"messageId": "m2", "role": "ROLE_USER", "parts": [{"text": "x"}]},
                "configuration": {
                    "taskPushNotificationConfig": {"url": "https://example.com/push"}
                }
            }
        })));
        assert!(!payload_requires_management(
            &json!({"method": "SubscribeToTask"})
        ));
        assert!(payload_requires_management(&json!({
            "method": "CreateTaskPushNotificationConfig",
            "params": {"taskId": "t1", "url": "https://example.com/push"}
        })));
        for method in [
            "GetTaskPushNotificationConfig",
            "ListTaskPushNotificationConfigs",
            "DeleteTaskPushNotificationConfig",
            "tasks/pushNotificationConfig/get",
            "tasks/pushNotificationConfig/list",
        ] {
            assert!(payload_requires_management(&json!({"method": method})));
        }
    }

    #[test]
    fn payload_write_detection_single_and_batch() {
        // Single read → no gate.
        assert!(!payload_has_write_method(
            &json!({"jsonrpc":"2.0","method":"tasks/get","params":{"id":"t"},"id":1})
        ));
        // Single write → gate.
        assert!(payload_has_write_method(
            &json!({"jsonrpc":"2.0","method":"message/send","params":{},"id":1})
        ));
        // Batch of only reads → no gate.
        assert!(!payload_has_write_method(&json!([
            {"jsonrpc":"2.0","method":"tasks/get","params":{"id":"a"},"id":1},
            {"jsonrpc":"2.0","method":"tasks/get","params":{"id":"b"},"id":2},
        ])));
        // Batch containing one write → gate (the whole request is gated).
        assert!(payload_has_write_method(&json!([
            {"jsonrpc":"2.0","method":"tasks/get","params":{"id":"a"},"id":1},
            {"jsonrpc":"2.0","method":"tasks/cancel","params":{"id":"b"},"id":2},
        ])));
        // Non-object scalar → no method, no gate.
        assert!(!payload_has_write_method(&json!("nope")));
    }

    #[test]
    fn active_agent_is_routable() {
        let agent = Agent::new("planner-1", "agents", "demo");
        assert!(agent_routability_rejection(&agent, "agents", "demo", "planner-1").is_none());
    }

    #[test]
    fn banned_agent_is_rejected_with_reason_but_not_operator() {
        use acteon_core::AgentAdminState;
        let mut agent = Agent::new("planner-1", "agents", "demo");
        agent.apply_admin_state(
            AgentAdminState::Banned,
            Some("exfiltration".into()),
            Some("op@acme.io".into()),
            None,
        );
        let (status, body) = agent_routability_rejection(&agent, "agents", "demo", "planner-1")
            .expect("banned agent must be rejected");
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.error.contains("banned"), "{}", body.error);
        assert!(body.error.contains("exfiltration"), "{}", body.error);
        assert!(
            body.error.contains("agents/demo/planner-1"),
            "{}",
            body.error
        );
        // The operator's identity is never leaked in the 403.
        assert!(!body.error.contains("op@acme.io"), "{}", body.error);
    }

    #[test]
    fn suspended_agent_is_rejected_but_expired_suspension_is_routable() {
        use acteon_core::AgentAdminState;
        use chrono::{Duration, Utc};

        let mut suspended = Agent::new("planner-1", "agents", "demo");
        suspended.apply_admin_state(AgentAdminState::Suspended, None, None, None);
        assert!(
            agent_routability_rejection(&suspended, "agents", "demo", "planner-1").is_some(),
            "an active suspension must reject",
        );

        // A suspension whose expiry is in the past reads as Active.
        let mut expired = Agent::new("planner-1", "agents", "demo");
        expired.apply_admin_state(
            AgentAdminState::Suspended,
            None,
            None,
            Some(Utc::now() - Duration::hours(1)),
        );
        assert!(
            agent_routability_rejection(&expired, "agents", "demo", "planner-1").is_none(),
            "an expired suspension must be routable",
        );
    }
}
