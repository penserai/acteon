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
use axum::response::sse::{KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

use acteon_core::{Agent, AgentCard, Task, TaskMessage, TaskState};
use acteon_gateway::{TaskEngine, TaskEngineError, TaskScope};
use acteon_state::{KeyKind, StateKey};

use super::AppState;
use super::schemas::ErrorResponse;
use super::stream::{StreamQuery, make_event_stream};
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
    let mut engine = TaskEngine::new(gw.state_store().clone());
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
        task.context_id.clone_from(&message.context_id);
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

/// `agent/getAuthenticatedExtendedCard` — JSON-RPC counterpart to the
/// public well-known card.
///
/// Loads every `AgentCard` published under the caller's namespace and
/// tenant via the shared discovery helper, returning a single tenant
/// card (verbatim if exactly one is registered, aggregated if several).
/// The returned card's `capabilities.extended_agent_card` is set to
/// `true` so a client can confirm the method was reached.
///
/// The endpoint requires the standard A2A `Dispatch` permission grant
/// (enforced at the JSON-RPC entrypoint), so an unauthenticated caller
/// never reaches this method.
///
/// Returns `TaskNotFound` (-32001) when no card is registered for the
/// tenant — the closest spec-defined error for "no resource matches".
/// `InternalError` only on a state-store read failure.
async fn method_agent_get_authenticated_extended_card(
    state: &AppState,
    scope: &TaskScope,
) -> Result<AgentCard, A2aError> {
    use super::a2a_discovery::resolve_tenant_card;
    match resolve_tenant_card(state, &scope.namespace, &scope.tenant).await {
        Ok(Some(card)) => Ok(mark_extended(card)),
        Ok(None) => Err(A2aError::new(
            TASK_NOT_FOUND,
            format!(
                "no agent card published under {}/{}",
                scope.namespace, scope.tenant,
            ),
        )),
        Err(_) => Err(A2aError::internal("internal error")),
    }
}

/// Flag a resolved card as the extended variant before returning it
/// over `agent/getAuthenticatedExtendedCard`. Split out so the
/// capability-flag flip is unit-testable without an `AppState`.
fn mark_extended(mut card: AgentCard) -> AgentCard {
    card.capabilities.extended_agent_card = true;
    card
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

/// A2A methods that drive or mutate task state. A caller whose bound
/// agent has been suspended or banned by an operator is blocked from
/// these (mirroring the bus `is_routable` gate); read methods
/// (`tasks/get`, task events) and discovery stay open so a paused agent
/// can still observe its in-flight work.
fn is_write_method(method: &str) -> bool {
    matches!(method, "message/send" | "tasks/cancel")
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
async fn enforce_agent_routable(
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
) -> Result<Value, A2aError> {
    match method {
        "message/send" => {
            let p = serde_json::from_value::<MessageSendParams>(params)
                .map_err(|e| A2aError::invalid_params(format!("invalid params: {e}")))?;
            to_value(&method_message_send(engine, scope, p).await?)
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
        Ok(value) => success_response(id, value),
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
    if let Err((status, body)) =
        enforce_agent_routable(&state, &identity, &namespace, &tenant).await
    {
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

/// `GET /a2a/{namespace}/{tenant}/v1/tasks/{id}/events` — SSE stream
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
    Path((namespace, tenant, task_id)): Path<(String, String, String)>,
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

    #[test]
    fn mark_extended_sets_capability_flag() {
        use acteon_core::AgentCard;
        let card = AgentCard::new("a1", "agents", "demo", "Card", "1.0");
        assert!(
            !card.capabilities.extended_agent_card,
            "default card must not advertise the extended capability"
        );
        let marked = mark_extended(card);
        assert!(
            marked.capabilities.extended_agent_card,
            "mark_extended must flip the capability flag on the returned card"
        );
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
        assert!(!is_write_method("tasks/get"));
        assert!(!is_write_method("agent/getAuthenticatedExtendedCard"));
        assert!(!is_write_method("unknown"));
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
