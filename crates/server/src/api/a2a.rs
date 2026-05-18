//! A2A protocol codecs — JSON-RPC 2.0 + REST binding (Phase 2).
//!
//! This is the external entry point for the A2A Task Engine
//! (`acteon-gateway`'s `task_engine`). Until this module, the engine
//! had no API surface — only the background stale-task reaper reached
//! it. Two transports share one set of method implementations:
//!
//! - **JSON-RPC 2.0** — `POST /a2a/{namespace}/{tenant}`. A2A's
//!   primary transport: one endpoint, the method named in the
//!   envelope.
//! - **REST binding** (A2A spec §11) — `POST .../v1/message:send`,
//!   `GET .../v1/tasks/{id}`, `POST .../v1/tasks/{id}/cancel`.
//!   (§11 spells cancel `:cancel`; axum's router cannot match a path
//!   parameter with a literal suffix in one segment, so the routable
//!   form is `/cancel`. The JSON-RPC method name stays spec-exact.)
//!
//! Both transports are scoped by a `{namespace}/{tenant}` path prefix.
//! A2A itself has no notion of Acteon's multi-tenancy, so the tenant
//! is carried in the URL and authorized against the caller's grants,
//! mirroring the bus API.
//!
//! Methods here are the non-streaming core: `message/send`,
//! `tasks/get`, `tasks/cancel`. Streaming (`message/stream`,
//! `tasks/resubscribe`) is the Phase 3 SSE bridge; push-notification
//! config is Phase 4.
//!
//! The `A2A-Version` header is negotiated up front — a request that
//! pins an unsupported version is rejected before any work is done.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use acteon_core::{Task, TaskMessage, TaskState};
use acteon_gateway::{TaskEngine, TaskEngineError, TaskScope};

use super::AppState;
use super::schemas::ErrorResponse;
use crate::auth::identity::CallerIdentity;
use crate::auth::role::Permission;

/// A2A protocol version this server implements. Echoed on every
/// response as the `A2A-Version` header; an inbound `A2A-Version`
/// that names a different version is rejected.
pub const A2A_PROTOCOL_VERSION: &str = "1.0";

/// Header carrying the negotiated A2A protocol version. Compared
/// case-insensitively by `HeaderMap`.
const A2A_VERSION_HEADER: &str = "a2a-version";

// ---------------------------------------------------------------------
// JSON-RPC 2.0 envelope
// ---------------------------------------------------------------------

/// Inbound JSON-RPC 2.0 request envelope. Parsed leniently — a bad
/// `jsonrpc` value or missing field becomes a structured JSON-RPC
/// error rather than an HTTP-level rejection.
#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[serde(default)]
    jsonrpc: String,
    method: String,
    #[serde(default)]
    params: Value,
    /// JSON-RPC `id` — string, number, or null. Echoed verbatim on
    /// the response.
    #[serde(default)]
    id: Value,
}

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
// Acteon: `A2A-Version` negotiation failure. JSON-RPC reserves
// -32000..-32099 for implementation-defined server errors.
const VERSION_NOT_SUPPORTED: i64 = -32000;

// ---------------------------------------------------------------------
// A2A error model
// ---------------------------------------------------------------------

/// A method-level A2A failure. Carries a JSON-RPC error code; the same
/// value drives both the JSON-RPC error object and the REST HTTP
/// status, so the two transports report the same failure consistently.
#[derive(Debug, Clone, PartialEq, Eq)]
struct A2aError {
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
            INVALID_PARAMS | INVALID_REQUEST | PARSE_ERROR | VERSION_NOT_SUPPORTED => {
                StatusCode::BAD_REQUEST
            }
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn to_jsonrpc(&self) -> JsonRpcErrorBody {
        JsonRpcErrorBody {
            code: self.code,
            message: self.message.clone(),
            data: None,
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
            TaskEngineError::Validation(v) => A2aError::invalid_params(v.to_string()),
            TaskEngineError::ReferenceCycle { .. }
            | TaskEngineError::ReferenceDepthExceeded { .. }
            | TaskEngineError::ReferenceGraphTooLarge { .. }
            | TaskEngineError::Approval(_)
            | TaskEngineError::InvalidPauseKind(_) => A2aError::invalid_params(e.to_string()),
            // CAS contention, serde, state-store, approval-id collision —
            // transient or server-side; surface as internal.
            TaskEngineError::CasExhausted(_)
            | TaskEngineError::ApprovalConflict(_)
            | TaskEngineError::State(_)
            | TaskEngineError::Serde(_) => A2aError::internal(e.to_string()),
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

// ---------------------------------------------------------------------
// Method implementations — transport-agnostic
// ---------------------------------------------------------------------

/// Build a Task Engine for one request from the gateway's state store,
/// attaching the gateway's compliance-decorated audit store so A2A
/// task transitions land on the same hash chain as action records.
async fn task_engine(state: &AppState) -> TaskEngine {
    let gw = state.gateway.read().await;
    let engine = TaskEngine::new(gw.state_store().clone());
    match gw.audit_store() {
        Some(audit) => engine.with_audit(audit),
        None => engine,
    }
}

/// `message/send` — send a message to an agent.
///
/// A message carrying a `taskId` continues that task (the message is
/// appended to its history); a message without one mints a fresh
/// `Submitted` task. Either way the result is the resulting [`Task`].
async fn method_message_send(
    engine: &TaskEngine,
    scope: &TaskScope,
    params: MessageSendParams,
) -> Result<Task, A2aError> {
    let mut message = params.message;
    if message.parts.is_empty() {
        return Err(A2aError::invalid_params("message.parts must not be empty"));
    }
    if let Some(task_id) = message.task_id.clone() {
        // Continue an existing task.
        Ok(engine.append_history(scope, &task_id, message).await?)
    } else {
        // Mint a new task with this message as its first history entry.
        let task_id = uuid::Uuid::now_v7().to_string();
        let mut task = Task::new(&task_id, &scope.namespace, &scope.tenant);
        task.context_id = message.context_id.clone();
        // Bind the message to the task it now belongs to.
        message.task_id = Some(task_id);
        task.append_history(message)
            .map_err(|e| A2aError::invalid_params(e.to_string()))?;
        Ok(engine.create_task(task).await?)
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
async fn method_tasks_cancel(
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
    Ok(engine
        .transition_task(scope, &params.id, TaskState::Canceled, None)
        .await?)
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
                error: "a2a requires the dispatch permission (admin or operator role)".into(),
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

/// The `A2A-Version` response header, attached to every A2A response.
fn version_header() -> [(&'static str, &'static str); 1] {
    [(A2A_VERSION_HEADER, A2A_PROTOCOL_VERSION)]
}

// ---------------------------------------------------------------------
// JSON-RPC transport
// ---------------------------------------------------------------------

/// Encode a JSON-RPC success response (HTTP 200 — JSON-RPC carries
/// failures in the body, not the status line).
fn rpc_ok(id: Value, result: Value) -> Response {
    (
        StatusCode::OK,
        version_header(),
        Json(JsonRpcResponse {
            jsonrpc: "2.0",
            result: Some(result),
            error: None,
            id,
        }),
    )
        .into_response()
}

/// Encode a JSON-RPC error response (also HTTP 200).
fn rpc_err(id: Value, err: &A2aError) -> Response {
    (
        StatusCode::OK,
        version_header(),
        Json(JsonRpcResponse {
            jsonrpc: "2.0",
            result: None,
            error: Some(err.to_jsonrpc()),
            id,
        }),
    )
        .into_response()
}

/// `POST /a2a/{namespace}/{tenant}` — the A2A JSON-RPC 2.0 endpoint.
///
/// The raw body is parsed here rather than via the `Json` extractor so
/// a malformed body becomes a JSON-RPC parse error (`-32700`) instead
/// of an HTTP 400 the A2A client cannot interpret.
pub async fn a2a_rpc(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant)): Path<(String, String)>,
    headers: HeaderMap,
    body: String,
) -> Response {
    if let Err(e) = negotiate_version(&headers) {
        return rpc_err(Value::Null, &e);
    }
    if let Err((status, body)) = authorize(&identity, &namespace, &tenant) {
        return (status, version_header(), body).into_response();
    }
    let req: JsonRpcRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(e) => {
            return rpc_err(
                Value::Null,
                &A2aError::new(PARSE_ERROR, format!("parse error: {e}")),
            );
        }
    };
    if req.jsonrpc != "2.0" {
        return rpc_err(
            req.id,
            &A2aError::new(INVALID_REQUEST, "jsonrpc field must be \"2.0\""),
        );
    }
    let scope = TaskScope::new(&namespace, &tenant);
    let engine = task_engine(&state).await;

    let outcome: Result<Task, A2aError> = match req.method.as_str() {
        "message/send" => match serde_json::from_value::<MessageSendParams>(req.params) {
            Ok(p) => method_message_send(&engine, &scope, p).await,
            Err(e) => Err(A2aError::invalid_params(format!("invalid params: {e}"))),
        },
        "tasks/get" => match serde_json::from_value::<TaskQueryParams>(req.params) {
            Ok(p) => method_tasks_get(&engine, &scope, p).await,
            Err(e) => Err(A2aError::invalid_params(format!("invalid params: {e}"))),
        },
        "tasks/cancel" => match serde_json::from_value::<TaskIdParams>(req.params) {
            Ok(p) => method_tasks_cancel(&engine, &scope, p).await,
            Err(e) => Err(A2aError::invalid_params(format!("invalid params: {e}"))),
        },
        other => Err(A2aError::new(
            METHOD_NOT_FOUND,
            format!("method '{other}' is not supported"),
        )),
    };

    match outcome {
        Ok(task) => match serde_json::to_value(&task) {
            Ok(v) => rpc_ok(req.id, v),
            Err(e) => rpc_err(req.id, &A2aError::internal(format!("encode error: {e}"))),
        },
        Err(e) => rpc_err(req.id, &e),
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

/// `POST /a2a/{namespace}/{tenant}/v1/message:send` — REST binding for
/// `message/send`.
pub async fn a2a_rest_message_send(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant)): Path<(String, String)>,
    headers: HeaderMap,
    Json(params): Json<MessageSendParams>,
) -> Response {
    if let Err(e) = negotiate_version(&headers) {
        return rest_result(Err(e));
    }
    if let Err((status, body)) = authorize(&identity, &namespace, &tenant) {
        return (status, version_header(), body).into_response();
    }
    let scope = TaskScope::new(&namespace, &tenant);
    let engine = task_engine(&state).await;
    rest_result(method_message_send(&engine, &scope, params).await)
}

/// `GET /a2a/{namespace}/{tenant}/v1/tasks/{id}` — REST binding for
/// `tasks/get`. `?historyLength=N` trims the returned history.
pub async fn a2a_rest_task_get(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, id)): Path<(String, String, String)>,
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
    let params = TaskQueryParams {
        id,
        history_length: query.history_length,
    };
    rest_result(method_tasks_get(&engine, &scope, params).await)
}

/// Query string for the REST `tasks/get`.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaskGetQuery {
    #[serde(default)]
    history_length: Option<usize>,
}

/// `POST /a2a/{namespace}/{tenant}/v1/tasks/{id}/cancel` — REST
/// binding for `tasks/cancel`.
pub async fn a2a_rest_task_cancel(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = negotiate_version(&headers) {
        return rest_result(Err(e));
    }
    if let Err((status, body)) = authorize(&identity, &namespace, &tenant) {
        return (status, version_header(), body).into_response();
    }
    let scope = TaskScope::new(&namespace, &tenant);
    let engine = task_engine(&state).await;
    rest_result(method_tasks_cancel(&engine, &scope, TaskIdParams { id }).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_core::TaskRole;
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

    #[tokio::test]
    async fn message_send_mints_a_new_task() {
        let e = engine();
        let params = MessageSendParams {
            message: user_message("hello"),
        };
        let task = method_message_send(&e, &scope(), params).await.unwrap();
        assert_eq!(task.status.state, TaskState::Submitted);
        assert_eq!(task.history.len(), 1);
        // The minted task is persisted and fetchable.
        let got = e.get_task(&scope(), &task.id).await.unwrap().unwrap();
        assert_eq!(got.id, task.id);
        // The message was bound to the new task.
        assert_eq!(got.history[0].task_id.as_deref(), Some(task.id.as_str()));
    }

    #[tokio::test]
    async fn message_send_continues_an_existing_task() {
        let e = engine();
        let first = method_message_send(
            &e,
            &scope(),
            MessageSendParams {
                message: user_message("first"),
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
        // Trimming keeps the most recent message.
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
        let task = method_message_send(
            &e,
            &scope(),
            MessageSendParams {
                message: user_message("work"),
            },
        )
        .await
        .unwrap();
        let canceled = method_tasks_cancel(
            &e,
            &scope(),
            TaskIdParams {
                id: task.id.clone(),
            },
        )
        .await
        .unwrap();
        assert_eq!(canceled.status.state, TaskState::Canceled);
    }

    #[tokio::test]
    async fn tasks_cancel_on_terminal_is_not_cancelable() {
        let e = engine();
        let task = method_message_send(
            &e,
            &scope(),
            MessageSendParams {
                message: user_message("work"),
            },
        )
        .await
        .unwrap();
        method_tasks_cancel(
            &e,
            &scope(),
            TaskIdParams {
                id: task.id.clone(),
            },
        )
        .await
        .unwrap();
        // Second cancel — already Canceled (terminal).
        let err = method_tasks_cancel(
            &e,
            &scope(),
            TaskIdParams {
                id: task.id.clone(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, TASK_NOT_CANCELABLE);
        assert_eq!(err.http_status(), StatusCode::CONFLICT);
    }

    #[test]
    fn version_negotiation() {
        let mut h = HeaderMap::new();
        // Absent header — accepted.
        assert!(negotiate_version(&h).is_ok());
        // Matching version — accepted.
        h.insert(A2A_VERSION_HEADER, A2A_PROTOCOL_VERSION.parse().unwrap());
        assert!(negotiate_version(&h).is_ok());
        // Mismatched version — rejected.
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
    }

    #[test]
    fn jsonrpc_request_parses() {
        let raw = json!({
            "jsonrpc": "2.0",
            "method": "tasks/get",
            "params": {"id": "t1"},
            "id": 7
        });
        let req: JsonRpcRequest = serde_json::from_value(raw).unwrap();
        assert_eq!(req.method, "tasks/get");
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.id, json!(7));
    }
}
