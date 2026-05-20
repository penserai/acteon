//! A2A protocol codecs — JSON-RPC 2.0 + REST binding (Phase 2).
//!
//! This is the external entry point for the A2A Task Engine
//! (`acteon-gateway`'s `task_engine`). Until this module, the engine
//! had no API surface — only the background stale-task reaper reached
//! it. Two transports share one set of method implementations:
//!
//! - **JSON-RPC 2.0** — `POST /a2a/{namespace}/{tenant}`. A2A's
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
//! Both transports are scoped by a `{namespace}/{tenant}` path prefix.
//! A2A itself has no notion of Acteon's multi-tenancy, so the tenant
//! is carried in the URL and authorized against the caller's grants,
//! mirroring the bus API. Request bodies are explicitly capped at
//! [`A2A_MAX_BODY_BYTES`].
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
            // Client-attributable failures — the message names a task
            // id, a transition, or a structural limit the caller
            // controls, none of it server-internal.
            TaskEngineError::Validation(v) => A2aError::invalid_params(v.to_string()),
            TaskEngineError::ReferenceCycle { .. }
            | TaskEngineError::ReferenceDepthExceeded { .. }
            | TaskEngineError::ReferenceGraphTooLarge { .. }
            | TaskEngineError::InvalidPauseKind(_)
            | TaskEngineError::Approval(_) => A2aError::invalid_params(e.to_string()),
            // Contention is transient and server-side; the retry count
            // is not useful to the caller.
            TaskEngineError::CasExhausted(_) => {
                A2aError::internal("the task is under contention; retry the request")
            }
            // State-store and serde failures can carry backend
            // internals — connection strings, SQL, file paths. Log the
            // detail server-side and return an opaque message so it
            // never reaches the caller (CWE-209).
            other @ (TaskEngineError::State(_)
            | TaskEngineError::Serde(_)
            | TaskEngineError::ApprovalConflict(_)) => {
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

/// Route an A2A method to its implementation. `state` is the
/// production hook for `tasks/cancel`'s chain-cancel propagation;
/// tests pass `None` for engine-only behavior.
async fn dispatch_method(
    state: Option<&AppState>,
    engine: &TaskEngine,
    scope: &TaskScope,
    method: &str,
    params: Value,
) -> Result<Task, A2aError> {
    match method {
        "message/send" => {
            let p = serde_json::from_value::<MessageSendParams>(params)
                .map_err(|e| A2aError::invalid_params(format!("invalid params: {e}")))?;
            method_message_send(engine, scope, p).await
        }
        "tasks/get" => {
            let p = serde_json::from_value::<TaskQueryParams>(params)
                .map_err(|e| A2aError::invalid_params(format!("invalid params: {e}")))?;
            method_tasks_get(engine, scope, p).await
        }
        "tasks/cancel" => {
            let p = serde_json::from_value::<TaskIdParams>(params)
                .map_err(|e| A2aError::invalid_params(format!("invalid params: {e}")))?;
            method_tasks_cancel(state, engine, scope, p).await
        }
        other => Err(A2aError::new(
            METHOD_NOT_FOUND,
            format!("method '{other}' is not supported"),
        )),
    }
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
    let outcome = dispatch_method(state, engine, scope, method, params).await;
    if is_notification {
        // Processed; a notification is answered with nothing.
        return None;
    }
    Some(match outcome {
        Ok(task) => match serde_json::to_value(&task) {
            Ok(v) => success_response(id, v),
            Err(_) => error_response(id, &A2aError::internal("internal error")),
        },
        Err(e) => error_response(id, &e),
    })
}

/// Process a parsed JSON-RPC payload — a single request object or a
/// batch array.
async fn handle_rpc_payload(
    state: Option<&AppState>,
    engine: &TaskEngine,
    scope: &TaskScope,
    parsed: Value,
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
                if let Some(resp) = dispatch_rpc_value(state, engine, scope, item).await {
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
        obj @ Value::Object(_) => match dispatch_rpc_value(state, engine, scope, &obj).await {
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
    let scope = TaskScope::new(&namespace, &tenant);
    let engine = task_engine(&state).await;
    match handle_rpc_payload(Some(&state), &engine, &scope, parsed).await {
        RpcReply::Single(resp) => (StatusCode::OK, version_header(), Json(resp)).into_response(),
        RpcReply::Batch(resps) => (StatusCode::OK, version_header(), Json(resps)).into_response(),
        // Notification(s) only — JSON-RPC 2.0 says answer with no body.
        RpcReply::Empty => (StatusCode::NO_CONTENT, version_header()).into_response(),
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

/// Query string for the REST `tasks/get`.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaskGetQuery {
    #[serde(default)]
    history_length: Option<usize>,
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

/// `POST /a2a/{namespace}/{tenant}/v1/tasks/{id}:cancel` — REST
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
    Path((namespace, tenant, id_and_verb)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = negotiate_version(&headers) {
        return rest_result(Err(e));
    }
    if let Err((status, body)) = authorize(&identity, &namespace, &tenant) {
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
}
