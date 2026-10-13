//! Explicit individual-agent ingress; tenant-level A2A remains independent.
use super::{AppState, a2a::A2A_PROTOCOL_VERSION, schemas::ErrorResponse};
use crate::execution_authority::{
    AgentServiceAuthorizationOpen, AgentServiceAuthorizationResolve, AgentServiceContinuation,
    AgentServiceInitialPushConfig, AgentServiceListPosition, AgentServiceListRequest,
    AgentServiceParent,
};
use crate::{
    auth::{
        identity::CallerIdentity, projection::AuthenticatedExecutionConfiguration, role::Permission,
    },
    execution_authority::{AgentServiceError, AgentServiceObservation, AgentServiceRequest},
};
use acteon_core::{
    ExecutionContextReference, PushAuthentication, StreamEvent, StreamEventType, Task, TaskMessage,
    TaskPushNotificationConfig, TaskState,
};
use acteon_executor::delegation::{
    PeerAuthorizationStatus, PeerCancelStatus, PeerContinuationStatus, PeerSendStatus,
};
use acteon_gateway::TaskScope;
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{convert::Infallible, sync::Arc, time::Duration};
use tokio::sync::broadcast;

use super::stream::ConnectionGuard;

const SOURCE_CONTEXT_HEADER: &str = "x-acteon-agent-source-context";
const EXECUTION_CONTEXT_HEADER: &str = "x-acteon-execution-context";

/// New service tasks use a fixed, operator-qualified runtime. Original context
/// injection for delegated host tools is a separate trusted-host operation.
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentMessageSend {
    pub message: TaskMessage,
}

#[derive(Serialize)]
struct AgentJsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<AgentJsonRpcError>,
    id: Value,
}

#[derive(Serialize)]
struct AgentJsonRpcError {
    code: i64,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

#[derive(Debug)]
struct AgentRpcFailure {
    code: i64,
    message: String,
}

impl AgentRpcFailure {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
        }
    }

    fn unsupported(message: impl Into<String>) -> Self {
        Self {
            code: -32004,
            message: message.into(),
        }
    }

    fn task_not_found(id: &str) -> Self {
        Self {
            code: -32001,
            message: format!("task '{id}' not found"),
        }
    }

    fn task_not_cancelable(id: &str) -> Self {
        Self {
            code: -32002,
            message: format!("task '{id}' is terminal and cannot be canceled"),
        }
    }

    fn push_not_supported(message: impl Into<String>) -> Self {
        Self {
            code: -32003,
            message: message.into(),
        }
    }

    fn extended_card_not_configured(message: impl Into<String>) -> Self {
        Self {
            code: -32007,
            message: message.into(),
        }
    }

    fn internal() -> Self {
        Self {
            code: -32603,
            message: "internal error".into(),
        }
    }
}

impl From<AgentServiceError> for AgentRpcFailure {
    fn from(cause: AgentServiceError) -> Self {
        match cause {
            AgentServiceError::Invalid | AgentServiceError::Conflict => Self::invalid(
                "the governed agent request is invalid or conflicts with durable state",
            ),
            AgentServiceError::NotFound => Self::task_not_found("unavailable"),
            AgentServiceError::Forbidden => Self {
                code: -32010,
                message: "current governed agent-service authority is required".into(),
            },
            AgentServiceError::Limits => Self {
                code: -32011,
                message: "governed agent-service capacity or budget is exhausted".into(),
            },
            AgentServiceError::Unavailable => Self::internal(),
        }
    }
}

fn agent_rpc_success(id: Value, result: Value) -> AgentJsonRpcResponse {
    AgentJsonRpcResponse {
        jsonrpc: "2.0",
        result: Some(result),
        error: None,
        id,
    }
}

fn agent_rpc_error(id: Value, failure: AgentRpcFailure) -> AgentJsonRpcResponse {
    let reason = match failure.code {
        -32001 => Some("TASK_NOT_FOUND"),
        -32002 => Some("TASK_NOT_CANCELABLE"),
        -32003 => Some("PUSH_NOTIFICATION_NOT_SUPPORTED"),
        -32004 => Some("UNSUPPORTED_OPERATION"),
        -32007 => Some("EXTENDED_AGENT_CARD_NOT_CONFIGURED"),
        -32009 => Some("VERSION_NOT_SUPPORTED"),
        _ => None,
    };
    AgentJsonRpcResponse {
        jsonrpc: "2.0",
        result: None,
        error: Some(AgentJsonRpcError {
            code: failure.code,
            message: failure.message,
            data: reason.map(|reason| {
                serde_json::json!([{
                    "@type": "type.googleapis.com/google.rpc.ErrorInfo",
                    "reason": reason,
                    "domain": "a2a-protocol.org",
                }])
            }),
        }),
        id,
    }
}

fn agent_rpc_response(response: AgentJsonRpcResponse, source: Option<&str>) -> Response {
    let mut response = (
        StatusCode::OK,
        [
            ("a2a-version", A2A_PROTOCOL_VERSION),
            ("cache-control", "no-store"),
        ],
        Json(response),
    )
        .into_response();
    if let Some(source) = source
        && let Ok(value) = HeaderValue::from_str(source)
    {
        response.headers_mut().insert(SOURCE_CONTEXT_HEADER, value);
    }
    response
}

fn validate_agent_rpc_tenant(params: &Value, tenant: &str) -> Result<(), AgentRpcFailure> {
    if params
        .get("tenant")
        .and_then(Value::as_str)
        .is_some_and(|selected| !selected.is_empty() && selected != tenant)
    {
        return Err(AgentRpcFailure::invalid(
            "tenant does not match the selected agent interface",
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GovernedListTasksParams {
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
    status_timestamp_after: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    include_artifacts: bool,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GovernedListTasksCursor {
    schema: u32,
    updated_at: chrono::DateTime<chrono::Utc>,
    task_id: String,
    visibility_binding: String,
    page_size: usize,
    context_id: Option<String>,
    status: Option<String>,
    history_length: Option<usize>,
    status_timestamp_after: Option<chrono::DateTime<chrono::Utc>>,
    include_artifacts: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GovernedPushAuthentication {
    scheme: String,
    #[serde(default)]
    credentials: Option<String>,
    #[serde(default)]
    credentials_ref: Option<acteon_core::SecretRef>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GovernedPushConfigInput {
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
    authentication: Option<GovernedPushAuthentication>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GovernedInlinePushConfig {
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
    authentication: Option<GovernedPushAuthentication>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GovernedPushConfigIdParams {
    #[serde(default)]
    tenant: Option<String>,
    task_id: String,
    id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GovernedPushConfigListParams {
    #[serde(default)]
    tenant: Option<String>,
    task_id: String,
    #[serde(default)]
    page_size: Option<usize>,
    #[serde(default)]
    page_token: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GovernedPushConfigCursor {
    schema: u32,
    task_id: String,
    page_size: usize,
    last_id: String,
    visibility_binding: String,
}

fn parse_governed_task_state(value: &str) -> Result<Option<TaskState>, AgentRpcFailure> {
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
        _ => Err(AgentRpcFailure::invalid(format!(
            "unsupported TaskState '{value}'"
        ))),
    }
}

fn decode_governed_list_cursor(value: &str) -> Result<GovernedListTasksCursor, AgentRpcFailure> {
    if value.len() > 8192 {
        return Err(AgentRpcFailure::invalid("invalid pageToken"));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| AgentRpcFailure::invalid("invalid pageToken"))?;
    let cursor: GovernedListTasksCursor = serde_json::from_slice(&bytes)
        .map_err(|_| AgentRpcFailure::invalid("invalid pageToken"))?;
    if cursor.schema != 1 || uuid::Uuid::parse_str(&cursor.task_id).is_err() {
        return Err(AgentRpcFailure::invalid("invalid pageToken"));
    }
    Ok(cursor)
}

fn encode_governed_list_cursor(
    cursor: &GovernedListTasksCursor,
) -> Result<String, AgentRpcFailure> {
    serde_json::to_vec(cursor)
        .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        .map_err(|_| AgentRpcFailure::internal())
}

fn governed_list_visibility_binding(
    context: &AgentRpcContext<'_>,
    source: Option<&ExecutionContextReference>,
) -> Result<String, AgentRpcFailure> {
    let scoped = context
        .proof
        .scope(context.namespace, context.tenant)
        .map_err(|_| AgentRpcFailure::from(AgentServiceError::Forbidden))?;
    let bytes = serde_json::to_vec(&(
        context.namespace,
        context.tenant,
        context.agent,
        scoped.authentication_source().principal(),
        scoped.credential_reference(),
        scoped.authentication_source().auth_method(),
        source,
    ))
    .map_err(|_| AgentRpcFailure::internal())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn ensure_governed_push_supported(context: &AgentRpcContext<'_>) -> Result<(), AgentRpcFailure> {
    let card =
        context
            .runtime()?
            .agent_service_card(context.namespace, context.tenant, context.agent)?;
    if !card.capabilities.push_notifications {
        return Err(AgentRpcFailure::push_not_supported(
            "the selected agent does not publish push notification support",
        ));
    }
    Ok(())
}

fn validate_governed_push_tenant(
    tenant: Option<&str>,
    context: &AgentRpcContext<'_>,
) -> Result<(), AgentRpcFailure> {
    if tenant.is_some_and(|tenant| !tenant.is_empty() && tenant != context.tenant) {
        return Err(AgentRpcFailure::invalid(
            "tenant does not match the selected agent interface",
        ));
    }
    Ok(())
}

fn governed_push_input(
    id: Option<String>,
    url: String,
    token: Option<String>,
    token_ref: Option<acteon_core::SecretRef>,
    authentication: Option<GovernedPushAuthentication>,
) -> Result<super::a2a_push::SetPushConfigInput, AgentRpcFailure> {
    if authentication
        .as_ref()
        .is_some_and(|authentication| authentication.scheme.trim().is_empty())
    {
        return Err(AgentRpcFailure::invalid(
            "authentication.scheme must not be empty",
        ));
    }
    Ok(super::a2a_push::SetPushConfigInput {
        id: id.filter(|id| !id.is_empty()),
        url,
        token,
        token_ref,
        authentication: authentication.map(|authentication| PushAuthentication {
            schemes: vec![authentication.scheme],
            credentials: authentication.credentials,
            credentials_ref: authentication.credentials_ref,
        }),
    })
}

fn project_governed_push_config(
    config: &TaskPushNotificationConfig,
) -> Result<Value, AgentRpcFailure> {
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
            serde_json::to_value(token_ref).map_err(|_| AgentRpcFailure::internal())?;
    }
    if let Some(authentication) = &config.authentication {
        let [scheme] = authentication.schemes.as_slice() else {
            return Err(AgentRpcFailure::internal());
        };
        let mut authentication_value = serde_json::json!({"scheme": scheme});
        if let Some(credentials) = &authentication.credentials {
            authentication_value["credentials"] = Value::String(credentials.clone());
        }
        if let Some(credentials_ref) = &authentication.credentials_ref {
            authentication_value["credentialsRef"] =
                serde_json::to_value(credentials_ref).map_err(|_| AgentRpcFailure::internal())?;
        }
        value["authentication"] = authentication_value;
    }
    Ok(value)
}

fn governed_push_error(error: super::a2a_push::PushConfigError) -> AgentRpcFailure {
    match error {
        super::a2a_push::PushConfigError::TaskNotFound(task_id) => {
            AgentRpcFailure::task_not_found(&task_id)
        }
        super::a2a_push::PushConfigError::ConfigNotFound { config_id, .. } => {
            AgentRpcFailure::task_not_found(&config_id)
        }
        super::a2a_push::PushConfigError::Invalid(message) => AgentRpcFailure::invalid(message),
        super::a2a_push::PushConfigError::Internal => AgentRpcFailure::internal(),
    }
}

async fn authorize_governed_push_task(
    context: &AgentRpcContext<'_>,
    task_id: &str,
) -> Result<Option<ExecutionContextReference>, AgentRpcFailure> {
    let parsed =
        uuid::Uuid::parse_str(task_id).map_err(|_| AgentRpcFailure::task_not_found(task_id))?;
    let source = parse_source_context(context.headers).map_err(AgentRpcFailure::invalid)?;
    context
        .runtime()?
        .observe_agent_service(context.observation(parsed, source.as_ref()))
        .await?;
    Ok(source)
}

fn agent_rpc_return_immediately(params: &Value) -> Result<bool, AgentRpcFailure> {
    let Some(configuration) = params.get("configuration") else {
        return Ok(false);
    };
    if configuration.is_null() {
        return Ok(false);
    }
    let Some(configuration) = configuration.as_object() else {
        return Err(AgentRpcFailure::invalid("configuration must be an object"));
    };
    match configuration.get("returnImmediately") {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(AgentRpcFailure::invalid(
            "configuration.returnImmediately must be a boolean",
        )),
    }
}

fn governed_rpc_requires_management(method: &str, params: &Value) -> bool {
    matches!(
        method,
        "CreateTaskPushNotificationConfig"
            | "GetTaskPushNotificationConfig"
            | "ListTaskPushNotificationConfigs"
            | "DeleteTaskPushNotificationConfig"
    ) || matches!(method, "SendMessage" | "SendStreamingMessage")
        && params
            .pointer("/configuration/taskPushNotificationConfig")
            .is_some()
}

fn governed_inline_push_input(
    context: &AgentRpcContext<'_>,
    params: &Value,
) -> Result<Option<super::a2a_push::SetPushConfigInput>, AgentRpcFailure> {
    let Some(value) = params.pointer("/configuration/taskPushNotificationConfig") else {
        return Ok(None);
    };
    ensure_governed_push_supported(context)?;
    let input: GovernedInlinePushConfig =
        serde_json::from_value(value.clone()).map_err(|error| {
            AgentRpcFailure::invalid(format!("invalid inline push configuration: {error}"))
        })?;
    validate_governed_push_tenant(input.tenant.as_deref(), context)?;
    if input
        .task_id
        .as_deref()
        .is_some_and(|task_id| !task_id.is_empty())
    {
        return Err(AgentRpcFailure::invalid(
            "an inline push configuration taskId must be empty",
        ));
    }
    let converted = governed_push_input(
        input.id,
        input.url,
        input.token,
        input.token_ref,
        input.authentication,
    )?;
    let mut candidate = TaskPushNotificationConfig::new(
        converted.id.as_deref().unwrap_or("pending-config"),
        "pending-task",
        context.namespace,
        context.tenant,
        &converted.url,
    );
    candidate.token.clone_from(&converted.token);
    candidate.token_ref.clone_from(&converted.token_ref);
    candidate
        .authentication
        .clone_from(&converted.authentication);
    candidate
        .validate()
        .map_err(|error| AgentRpcFailure::invalid(error.to_string()))?;
    super::a2a_ssrf::check_url_literal(&candidate.url)
        .map_err(|error| AgentRpcFailure::invalid(format!("push url rejected: {error}")))?;
    Ok(Some(converted))
}

fn decode_agent_rpc_message(params: &Value) -> Result<TaskMessage, AgentRpcFailure> {
    let mut message = params
        .get("message")
        .cloned()
        .ok_or_else(|| AgentRpcFailure::invalid("params.message is required"))?;
    if let Some(role) = message
        .as_object_mut()
        .and_then(|message| message.get_mut("role"))
        && let Some(role) = role.as_str()
        && let Some(role) = role.strip_prefix("ROLE_")
    {
        *message
            .as_object_mut()
            .expect("role was read from an object")
            .get_mut("role")
            .expect("role still exists") = Value::String(role.to_ascii_lowercase());
    }
    serde_json::from_value(message)
        .map_err(|error| AgentRpcFailure::invalid(format!("invalid params.message: {error}")))
}

fn project_governed_v1_task(task: &Task) -> Result<Value, AgentRpcFailure> {
    let mut projected =
        super::a2a::project_v1_task(task).map_err(|_| AgentRpcFailure::internal())?;
    if let Some(metadata) = projected.get_mut("metadata").and_then(Value::as_object_mut) {
        metadata.remove(acteon_gateway::agent_runtime::GOVERNED_TASK_METADATA_KEY);
        if metadata.is_empty() {
            projected
                .as_object_mut()
                .expect("a projected task is an object")
                .remove("metadata");
        }
    }
    Ok(projected)
}

fn project_governed_v1_task_with_cancellation(
    task: &Task,
    requested: bool,
    confirmed: bool,
    provider_abort: Option<&acteon_executor::governed::abort::ProviderAbortStatus>,
) -> Result<Value, AgentRpcFailure> {
    let mut projected = project_governed_v1_task(task)?;
    if !requested {
        return Ok(projected);
    }
    let object = projected
        .as_object_mut()
        .ok_or_else(AgentRpcFailure::internal)?;
    let metadata = object
        .entry("metadata")
        .or_insert_with(|| Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .ok_or_else(AgentRpcFailure::internal)?;
    let mut cancellation = serde_json::json!({
        "state": if confirmed { "confirmed" } else { "requested" },
        "futureStartsBlocked": true,
    });
    if let Some(status) = provider_abort {
        cancellation
            .as_object_mut()
            .expect("cancellation projection is an object")
            .insert(
                "providerAbort".into(),
                serde_json::to_value(status).map_err(|_| AgentRpcFailure::internal())?,
            );
    }
    metadata.insert("acteonCancellation".into(), cancellation);
    Ok(projected)
}

fn encode_source_context(
    source: Option<&ExecutionContextReference>,
) -> Result<Option<String>, AgentRpcFailure> {
    source
        .map(serde_json::to_vec)
        .transpose()
        .map_err(|_| AgentRpcFailure::internal())
        .map(|raw| raw.map(|raw| URL_SAFE_NO_PAD.encode(raw)))
}

async fn await_governed_agent_task(
    runtime: &crate::execution_authority::ExecutionAuthorityRuntime,
    namespace: &str,
    tenant: &str,
    agent: &str,
    task_id: uuid::Uuid,
    proof: &AuthenticatedExecutionConfiguration,
    source: Option<&acteon_core::ExecutionContextReference>,
) -> Result<Task, AgentRpcFailure> {
    loop {
        let observed = runtime
            .observe_agent_service(AgentServiceObservation {
                namespace,
                tenant,
                agent_id: agent,
                task_id,
                authentication: proof,
                source_context: source,
            })
            .await?;
        if observed.task.status.state.is_terminal() || observed.task.status.state.is_interrupt() {
            return Ok(observed.task);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

struct AgentRpcContext<'a> {
    state: &'a AppState,
    proof: &'a AuthenticatedExecutionConfiguration,
    namespace: &'a str,
    tenant: &'a str,
    agent: &'a str,
    headers: &'a HeaderMap,
}

impl AgentRpcContext<'_> {
    fn runtime(
        &self,
    ) -> Result<&crate::execution_authority::ExecutionAuthorityRuntime, AgentRpcFailure> {
        self.state
            .execution_authority
            .as_deref()
            .ok_or_else(AgentRpcFailure::internal)
    }

    fn observation<'a>(
        &'a self,
        task_id: uuid::Uuid,
        source: Option<&'a acteon_core::ExecutionContextReference>,
    ) -> AgentServiceObservation<'a> {
        AgentServiceObservation {
            namespace: self.namespace,
            tenant: self.tenant,
            agent_id: self.agent,
            task_id,
            authentication: self.proof,
            source_context: source,
        }
    }
}

async fn submit_agent_rpc(
    context: &AgentRpcContext<'_>,
    params: &Value,
) -> Result<(Task, Option<ExecutionContextReference>), AgentRpcFailure> {
    let runtime = context.runtime()?;
    let inline_push = governed_inline_push_input(context, params)?;
    let source = parse_source_context(context.headers).map_err(AgentRpcFailure::invalid)?;
    let mut message = decode_agent_rpc_message(params)?;
    let outcome = if let Some(task_id) = message.task_id.clone() {
        let task_id = uuid::Uuid::parse_str(&task_id)
            .map_err(|_| AgentRpcFailure::invalid("message.taskId is invalid"))?;
        let observed = runtime
            .observe_agent_service(context.observation(task_id, source.as_ref()))
            .await?;
        if message.context_id.is_none() {
            message.context_id.clone_from(&observed.task.context_id);
        } else if message.context_id != observed.task.context_id {
            return Err(AgentRpcFailure::invalid(
                "message.contextId must exactly match the existing task",
            ));
        }
        if observed.task.status.state != TaskState::InputRequired {
            return Err(AgentRpcFailure::invalid(
                "only an InputRequired task accepts an A2A continuation message",
            ));
        }
        let challenge_id = message
            .metadata
            .get(acteon_core::TASK_CHALLENGE_ID_METADATA_KEY)
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AgentRpcFailure::invalid(
                    "an InputRequired response must include metadata.acteon.challengeId",
                )
            })?;
        if let Some(mut input) = inline_push {
            if input.id.is_none() {
                input.id =
                    Some(uuid::Uuid::new_v5(&task_id, b"acteon:a2a:inline-push").to_string());
            }
            let scope = TaskScope::new(context.namespace, context.tenant);
            super::a2a_push::save_authorized_config(
                context.state,
                &scope,
                &task_id.to_string(),
                input,
            )
            .await
            .map_err(governed_push_error)?;
        }
        let observed = runtime
            .continue_agent_service(AgentServiceContinuation {
                observation: context.observation(task_id, source.as_ref()),
                challenge_id,
                response: &message,
            })
            .await?;
        (observed.task, source)
    } else {
        let authentication = context
            .state
            .auth
            .as_ref()
            .ok_or_else(AgentRpcFailure::internal)?;
        let parent_input = parse_parent(context.headers).map_err(AgentRpcFailure::invalid)?;
        let parent = parent_input
            .as_ref()
            .map(|(context, permits)| AgentServiceParent { context, permits });
        let initial_push_configs: Vec<_> = inline_push
            .into_iter()
            .map(|input| AgentServiceInitialPushConfig {
                id: input.id,
                url: input.url,
                token: input.token,
                token_ref: input.token_ref,
                authentication: input.authentication,
            })
            .collect();
        let accepted = Box::pin(runtime.accept_agent_service(AgentServiceRequest {
            namespace: context.namespace,
            tenant: context.tenant,
            agent_id: context.agent,
            message: &message,
            authentication: context.proof,
            auth_provider: authentication,
            parent,
            initial_push_configs: &initial_push_configs,
        }))
        .await?;
        (accepted.task, Some(accepted.source_context))
    };
    Ok(outcome)
}

async fn send_agent_rpc(
    context: &AgentRpcContext<'_>,
    params: &Value,
) -> Result<(Value, Option<String>), AgentRpcFailure> {
    let runtime = context.runtime()?;
    let return_immediately = agent_rpc_return_immediately(params)?;
    let (task, source_context) = Box::pin(submit_agent_rpc(context, params)).await?;
    let task_id = uuid::Uuid::parse_str(&task.id).map_err(|_| AgentRpcFailure::internal())?;
    let task = if return_immediately {
        task
    } else {
        await_governed_agent_task(
            runtime,
            context.namespace,
            context.tenant,
            context.agent,
            task_id,
            context.proof,
            source_context.as_ref(),
        )
        .await?
    };
    let encoded_source = encode_source_context(source_context.as_ref())?;
    Ok((
        serde_json::json!({"task": project_governed_v1_task(&task)?}),
        encoded_source,
    ))
}

fn ensure_agent_streaming_supported(context: &AgentRpcContext<'_>) -> Result<(), AgentRpcFailure> {
    let card =
        context
            .runtime()?
            .agent_service_card(context.namespace, context.tenant, context.agent)?;
    if !card.capabilities.streaming {
        return Err(AgentRpcFailure::unsupported(
            "the selected agent does not publish streaming support",
        ));
    }
    Ok(())
}

fn agent_stream_event(id: &Value, result: Value) -> Option<Event> {
    serde_json::to_string(&agent_rpc_success(id.clone(), result))
        .ok()
        .map(|data| Event::default().data(data))
}

fn agent_stream_error_event(id: &Value, failure: AgentRpcFailure) -> Option<Event> {
    serde_json::to_string(&agent_rpc_error(id.clone(), failure))
        .ok()
        .map(|data| Event::default().data(data))
}

struct GovernedAgentStreamState {
    rx: broadcast::Receiver<StreamEvent>,
    access: GovernedAgentStreamAccess,
    task_id: uuid::Uuid,
    task_id_text: String,
    source: Option<ExecutionContextReference>,
    request_id: Value,
    initial: Option<Value>,
    done: bool,
    done_after_initial: bool,
    close_on_interrupt: bool,
    _guard: ConnectionGuard,
}

struct GovernedAgentStreamAccess {
    runtime: Arc<crate::execution_authority::ExecutionAuthorityRuntime>,
    proof: AuthenticatedExecutionConfiguration,
    namespace: String,
    tenant: String,
    agent: String,
}

struct GovernedAgentStreamSeed {
    task: Task,
    source: Option<ExecutionContextReference>,
    request_id: Value,
    close_on_interrupt: bool,
}

fn observe_governed_stream(state: &GovernedAgentStreamState) -> AgentServiceObservation<'_> {
    AgentServiceObservation {
        namespace: &state.access.namespace,
        tenant: &state.access.tenant,
        agent_id: &state.access.agent,
        task_id: state.task_id,
        authentication: &state.access.proof,
        source_context: state.source.as_ref(),
    }
}

fn agent_stream_closes(task: &Task, close_on_interrupt: bool) -> bool {
    task.status.state.is_terminal() || (close_on_interrupt && task.status.state.is_interrupt())
}

fn make_governed_agent_stream(
    rx: broadcast::Receiver<StreamEvent>,
    access: GovernedAgentStreamAccess,
    seed: GovernedAgentStreamSeed,
    guard: ConnectionGuard,
) -> impl Stream<Item = Result<Event, Infallible>> {
    let task_id = uuid::Uuid::parse_str(&seed.task.id).expect("governed task ids are UUIDs");
    let done_after_initial = agent_stream_closes(&seed.task, seed.close_on_interrupt);
    let initial = project_governed_v1_task(&seed.task)
        .ok()
        .map(|task| serde_json::json!({"task": task}));
    let state = GovernedAgentStreamState {
        rx,
        access,
        task_id,
        task_id_text: seed.task.id,
        source: seed.source,
        request_id: seed.request_id,
        initial,
        done: false,
        done_after_initial,
        close_on_interrupt: seed.close_on_interrupt,
        _guard: guard,
    };

    stream::unfold(state, |mut state| async move {
        if state.done {
            return None;
        }
        if let Some(initial) = state.initial.take() {
            state.done = state.done_after_initial;
            let frame = agent_stream_event(&state.request_id, initial)?;
            return Some((Ok(frame), state));
        }

        loop {
            let event = match state.rx.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Closed) => return None,
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let observed = match state
                        .access
                        .runtime
                        .observe_agent_service(observe_governed_stream(&state))
                        .await
                    {
                        Ok(observed) => observed,
                        Err(error) => {
                            state.done = true;
                            let frame = agent_stream_error_event(&state.request_id, error.into())?;
                            return Some((Ok(frame), state));
                        }
                    };
                    state.done = agent_stream_closes(&observed.task, state.close_on_interrupt);
                    let task = project_governed_v1_task(&observed.task).ok()?;
                    let frame =
                        agent_stream_event(&state.request_id, serde_json::json!({"task": task}))?;
                    return Some((Ok(frame), state));
                }
            };
            if event.namespace != state.access.namespace
                || event.tenant != state.access.tenant
                || event.action_type.as_deref() != Some("a2a.task")
                || event.action_id.as_deref() != Some(state.task_id_text.as_str())
            {
                continue;
            }
            let observed = match state
                .access
                .runtime
                .observe_agent_service(observe_governed_stream(&state))
                .await
            {
                Ok(observed) => observed,
                Err(error) => {
                    state.done = true;
                    let frame = agent_stream_error_event(&state.request_id, error.into())?;
                    return Some((Ok(frame), state));
                }
            };
            let (result, closes_stream) = match &event.event_type {
                StreamEventType::TaskTransitioned { to, .. } => (
                    super::a2a::project_v1_status_update(&observed.task, *to, event.timestamp),
                    to.is_terminal() || (state.close_on_interrupt && to.is_interrupt()),
                ),
                StreamEventType::TaskArtifactUpdated {
                    artifact_id,
                    last_chunk,
                    ..
                } => (
                    super::a2a::project_v1_artifact_update(
                        &observed.task,
                        artifact_id,
                        *last_chunk,
                    ),
                    false,
                ),
                _ => (None, false),
            };
            let Some(result) = result else {
                continue;
            };
            state.done = closes_stream;
            let frame = agent_stream_event(&state.request_id, result)?;
            return Some((Ok(frame), state));
        }
    })
}

async fn prepare_agent_stream(
    context: &AgentRpcContext<'_>,
    runtime: &crate::execution_authority::ExecutionAuthorityRuntime,
    method: &str,
    params: &Value,
) -> Result<(Task, Option<ExecutionContextReference>, bool), AgentRpcFailure> {
    match method {
        "SendStreamingMessage" => {
            agent_rpc_return_immediately(params)?;
            Box::pin(submit_agent_rpc(context, params))
                .await
                .map(|(task, source)| (task, source, true))
        }
        "SubscribeToTask" => {
            let id = params
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| AgentRpcFailure::invalid("params.id is required"))?;
            let task_id =
                uuid::Uuid::parse_str(id).map_err(|_| AgentRpcFailure::task_not_found(id))?;
            let source = parse_source_context(context.headers).map_err(AgentRpcFailure::invalid)?;
            let observed = runtime
                .observe_agent_service(context.observation(task_id, source.as_ref()))
                .await?;
            if observed.task.status.state.is_terminal() {
                return Err(AgentRpcFailure::unsupported(
                    "a terminal task cannot be subscribed to",
                ));
            }
            Ok((observed.task, source, false))
        }
        _ => unreachable!("only streaming methods enter the streaming handler"),
    }
}

async fn handle_agent_streaming_rpc(
    context: &AgentRpcContext<'_>,
    request: ParsedAgentRpc,
) -> Response {
    if request.is_notification {
        return invalid_agent_rpc_request(
            Value::Null,
            "streaming requests require jsonrpc \"2.0\" and an id",
        );
    }
    if let Err(failure) = validate_agent_rpc_tenant(&request.params, context.tenant) {
        return agent_rpc_response(agent_rpc_error(request.id, failure), None);
    }
    if let Err(failure) = ensure_agent_streaming_supported(context) {
        return agent_rpc_response(agent_rpc_error(request.id, failure), None);
    }
    let Some(registry) = context.state.connection_registry.as_ref() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "sse_streaming_not_enabled");
    };
    let bucket = format!("{}:{}", context.namespace, context.tenant);
    let Some(guard) = registry.try_acquire(&bucket).await else {
        return error(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_tenant_sse_connections",
        );
    };
    let rx = {
        let gateway = context.state.gateway.read().await;
        gateway.stream_tx().subscribe()
    };
    let Some(runtime) = context.state.execution_authority.clone() else {
        return agent_rpc_response(
            agent_rpc_error(request.id, AgentRpcFailure::internal()),
            None,
        );
    };

    let returns_source = request.method == "SendStreamingMessage";
    let outcome = Box::pin(prepare_agent_stream(
        context,
        runtime.as_ref(),
        &request.method,
        &request.params,
    ))
    .await;
    let (task, source, close_on_interrupt) = match outcome {
        Ok(outcome) => outcome,
        Err(failure) => return agent_rpc_response(agent_rpc_error(request.id, failure), None),
    };
    let encoded_source = if returns_source {
        match encode_source_context(source.as_ref()) {
            Ok(source) => source,
            Err(failure) => return agent_rpc_response(agent_rpc_error(request.id, failure), None),
        }
    } else {
        None
    };
    let event_stream = make_governed_agent_stream(
        rx,
        GovernedAgentStreamAccess {
            runtime,
            proof: context.proof.clone(),
            namespace: context.namespace.to_owned(),
            tenant: context.tenant.to_owned(),
            agent: context.agent.to_owned(),
        },
        GovernedAgentStreamSeed {
            task,
            source,
            request_id: request.id,
            close_on_interrupt,
        },
        guard,
    );
    let mut response = Sse::new(event_stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("ping"),
        )
        .into_response();
    response.headers_mut().insert(
        "a2a-version",
        HeaderValue::from_static(A2A_PROTOCOL_VERSION),
    );
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    if let Some(source) = encoded_source
        && let Ok(value) = HeaderValue::from_str(&source)
    {
        response.headers_mut().insert(SOURCE_CONTEXT_HEADER, value);
    }
    response
}

async fn get_agent_rpc(
    context: &AgentRpcContext<'_>,
    params: &Value,
) -> Result<(Value, Option<String>), AgentRpcFailure> {
    let id = params
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| AgentRpcFailure::invalid("params.id is required"))?;
    let task_id = uuid::Uuid::parse_str(id).map_err(|_| AgentRpcFailure::task_not_found(id))?;
    let source = parse_source_context(context.headers).map_err(AgentRpcFailure::invalid)?;
    let observed = context
        .runtime()?
        .observe_agent_service(context.observation(task_id, source.as_ref()))
        .await?;
    let mut task = observed.task;
    if let Some(history_length) = params.get("historyLength") {
        let history_length = history_length.as_u64().ok_or_else(|| {
            AgentRpcFailure::invalid("params.historyLength must be a non-negative integer")
        })?;
        let history_length = usize::try_from(history_length)
            .map_err(|_| AgentRpcFailure::invalid("params.historyLength is too large"))?;
        if task.history.len() > history_length {
            task.history.drain(0..task.history.len() - history_length);
        }
    }
    Ok((
        project_governed_v1_task_with_cancellation(
            &task,
            observed.future_starts_blocked,
            task.status.state == TaskState::Canceled,
            None,
        )?,
        None,
    ))
}

#[allow(clippy::too_many_lines)] // Keep one schema-closed protocol method auditable in one place.
async fn list_agent_rpc(
    context: &AgentRpcContext<'_>,
    params: &Value,
) -> Result<(Value, Option<String>), AgentRpcFailure> {
    let params: GovernedListTasksParams = serde_json::from_value(params.clone())
        .map_err(|error| AgentRpcFailure::invalid(format!("invalid ListTasks params: {error}")))?;
    if params
        .tenant
        .as_deref()
        .is_some_and(|tenant| !tenant.is_empty() && tenant != context.tenant)
    {
        return Err(AgentRpcFailure::invalid(
            "tenant does not match the selected agent interface",
        ));
    }
    let page_size = params.page_size.unwrap_or(50);
    if !(1..=100).contains(&page_size) {
        return Err(AgentRpcFailure::invalid(
            "pageSize must be between 1 and 100",
        ));
    }
    if params
        .history_length
        .is_some_and(|length| length > i32::MAX as usize)
    {
        return Err(AgentRpcFailure::invalid(
            "historyLength exceeds the A2A int32 range",
        ));
    }
    let status = params
        .status
        .as_deref()
        .map(parse_governed_task_state)
        .transpose()?
        .flatten();
    let source = parse_source_context(context.headers).map_err(AgentRpcFailure::invalid)?;
    let visibility_binding = governed_list_visibility_binding(context, source.as_ref())?;
    let cursor = params
        .page_token
        .as_deref()
        .map(decode_governed_list_cursor)
        .transpose()?;
    if let Some(cursor) = &cursor
        && (cursor.visibility_binding != visibility_binding
            || cursor.page_size != page_size
            || cursor.context_id != params.context_id
            || cursor.status != params.status
            || cursor.history_length != params.history_length
            || cursor.status_timestamp_after != params.status_timestamp_after
            || cursor.include_artifacts != params.include_artifacts)
    {
        return Err(AgentRpcFailure::invalid(
            "pageToken does not match the current caller or filters",
        ));
    }
    let after = cursor.as_ref().map(|cursor| AgentServiceListPosition {
        updated_at: cursor.updated_at,
        task_id: cursor.task_id.clone(),
    });
    let page = context
        .runtime()?
        .list_agent_services(AgentServiceListRequest {
            namespace: context.namespace,
            tenant: context.tenant,
            agent_id: context.agent,
            authentication: context.proof,
            source_context: source.as_ref(),
            context_id: params.context_id.as_deref(),
            status,
            status_timestamp_after: params.status_timestamp_after,
            after: after.as_ref(),
            page_size,
        })
        .await?;
    let total_size = i32::try_from(page.total_size).map_err(|_| AgentRpcFailure::internal())?;
    let next_page_token = if page.has_more {
        let last = page.tasks.last().ok_or_else(AgentRpcFailure::internal)?;
        encode_governed_list_cursor(&GovernedListTasksCursor {
            schema: 1,
            updated_at: last.task.updated_at,
            task_id: last.task.id.clone(),
            visibility_binding,
            page_size,
            context_id: params.context_id.clone(),
            status: params.status.clone(),
            history_length: params.history_length,
            status_timestamp_after: params.status_timestamp_after,
            include_artifacts: params.include_artifacts,
        })?
    } else {
        String::new()
    };
    let tasks = page
        .tasks
        .into_iter()
        .map(|mut observed| {
            if let Some(history_length) = params.history_length
                && observed.task.history.len() > history_length
            {
                observed
                    .task
                    .history
                    .drain(0..observed.task.history.len() - history_length);
            }
            if !params.include_artifacts {
                observed.task.artifacts.clear();
            }
            let confirmed = observed.task.status.state == TaskState::Canceled;
            project_governed_v1_task_with_cancellation(
                &observed.task,
                observed.future_starts_blocked,
                confirmed,
                None,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((
        serde_json::json!({
            "tasks": tasks,
            "nextPageToken": next_page_token,
            "pageSize": page_size,
            "totalSize": total_size,
        }),
        None,
    ))
}

async fn create_governed_push_config_rpc(
    context: &AgentRpcContext<'_>,
    params: &Value,
) -> Result<(Value, Option<String>), AgentRpcFailure> {
    ensure_governed_push_supported(context)?;
    let params: GovernedPushConfigInput = serde_json::from_value(params.clone())
        .map_err(|error| AgentRpcFailure::invalid(format!("invalid push config: {error}")))?;
    validate_governed_push_tenant(params.tenant.as_deref(), context)?;
    if params.task_id.is_empty() {
        return Err(AgentRpcFailure::invalid("taskId must not be empty"));
    }
    authorize_governed_push_task(context, &params.task_id).await?;
    let input = governed_push_input(
        params.id,
        params.url,
        params.token,
        params.token_ref,
        params.authentication,
    )?;
    let scope = TaskScope::new(context.namespace, context.tenant);
    let config =
        super::a2a_push::save_authorized_config(context.state, &scope, &params.task_id, input)
            .await
            .map_err(governed_push_error)?;
    Ok((project_governed_push_config(&config)?, None))
}

async fn get_governed_push_config_rpc(
    context: &AgentRpcContext<'_>,
    params: &Value,
) -> Result<(Value, Option<String>), AgentRpcFailure> {
    ensure_governed_push_supported(context)?;
    let params: GovernedPushConfigIdParams = serde_json::from_value(params.clone())
        .map_err(|error| AgentRpcFailure::invalid(format!("invalid push config query: {error}")))?;
    validate_governed_push_tenant(params.tenant.as_deref(), context)?;
    if params.task_id.is_empty() || params.id.is_empty() {
        return Err(AgentRpcFailure::invalid("taskId and id must not be empty"));
    }
    authorize_governed_push_task(context, &params.task_id).await?;
    let scope = TaskScope::new(context.namespace, context.tenant);
    let config =
        super::a2a_push::load_authorized_config(context.state, &scope, &params.task_id, &params.id)
            .await
            .map_err(governed_push_error)?;
    Ok((project_governed_push_config(&config)?, None))
}

async fn list_governed_push_configs_rpc(
    context: &AgentRpcContext<'_>,
    params: &Value,
) -> Result<(Value, Option<String>), AgentRpcFailure> {
    ensure_governed_push_supported(context)?;
    let params: GovernedPushConfigListParams = serde_json::from_value(params.clone())
        .map_err(|error| AgentRpcFailure::invalid(format!("invalid push config query: {error}")))?;
    validate_governed_push_tenant(params.tenant.as_deref(), context)?;
    if params.task_id.is_empty() {
        return Err(AgentRpcFailure::invalid("taskId must not be empty"));
    }
    let page_size = params.page_size.unwrap_or(50);
    if !(1..=100).contains(&page_size) {
        return Err(AgentRpcFailure::invalid(
            "pageSize must be between 1 and 100",
        ));
    }
    let source = authorize_governed_push_task(context, &params.task_id).await?;
    let visibility_binding = governed_list_visibility_binding(context, source.as_ref())?;
    let cursor = params
        .page_token
        .as_deref()
        .map(|token| {
            if token.len() > 8192 {
                return Err(AgentRpcFailure::invalid("invalid pageToken"));
            }
            URL_SAFE_NO_PAD
                .decode(token)
                .map_err(|_| AgentRpcFailure::invalid("invalid pageToken"))
                .and_then(|bytes| {
                    serde_json::from_slice::<GovernedPushConfigCursor>(&bytes)
                        .map_err(|_| AgentRpcFailure::invalid("invalid pageToken"))
                })
        })
        .transpose()?;
    if cursor.as_ref().is_some_and(|cursor| {
        cursor.schema != 1
            || cursor.task_id != params.task_id
            || cursor.page_size != page_size
            || cursor.visibility_binding != visibility_binding
    }) {
        return Err(AgentRpcFailure::invalid(
            "pageToken does not match the current caller, task, or page size",
        ));
    }
    let scope = TaskScope::new(context.namespace, context.tenant);
    let mut configs =
        super::a2a_push::list_authorized_configs(context.state, &scope, &params.task_id)
            .await
            .map_err(governed_push_error)?;
    configs.sort_by(|left, right| left.id.cmp(&right.id));
    if let Some(cursor) = &cursor {
        configs.retain(|config| config.id > cursor.last_id);
    }
    let has_more = configs.len() > page_size;
    configs.truncate(page_size);
    let projected = configs
        .iter()
        .map(project_governed_push_config)
        .collect::<Result<Vec<_>, _>>()?;
    let next_page_token = if has_more {
        let cursor = GovernedPushConfigCursor {
            schema: 1,
            task_id: params.task_id,
            page_size,
            last_id: configs
                .last()
                .ok_or_else(AgentRpcFailure::internal)?
                .id
                .clone(),
            visibility_binding,
        };
        URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&cursor).map_err(|_| AgentRpcFailure::internal())?)
    } else {
        String::new()
    };
    Ok((
        serde_json::json!({"configs": projected, "nextPageToken": next_page_token}),
        None,
    ))
}

async fn delete_governed_push_config_rpc(
    context: &AgentRpcContext<'_>,
    params: &Value,
) -> Result<(Value, Option<String>), AgentRpcFailure> {
    ensure_governed_push_supported(context)?;
    let params: GovernedPushConfigIdParams = serde_json::from_value(params.clone())
        .map_err(|error| AgentRpcFailure::invalid(format!("invalid push config query: {error}")))?;
    validate_governed_push_tenant(params.tenant.as_deref(), context)?;
    if params.task_id.is_empty() || params.id.is_empty() {
        return Err(AgentRpcFailure::invalid("taskId and id must not be empty"));
    }
    authorize_governed_push_task(context, &params.task_id).await?;
    let scope = TaskScope::new(context.namespace, context.tenant);
    match super::a2a_push::delete_authorized_config(
        context.state,
        &scope,
        &params.task_id,
        &params.id,
    )
    .await
    {
        Ok(()) | Err(super::a2a_push::PushConfigError::ConfigNotFound { .. }) => {}
        Err(error) => return Err(governed_push_error(error)),
    }
    Ok((serde_json::json!({}), None))
}

async fn cancel_agent_rpc(
    context: &AgentRpcContext<'_>,
    params: &Value,
) -> Result<(Value, Option<String>), AgentRpcFailure> {
    let id = params
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| AgentRpcFailure::invalid("params.id is required"))?;
    let task_id = uuid::Uuid::parse_str(id).map_err(|_| AgentRpcFailure::task_not_found(id))?;
    let source = parse_source_context(context.headers).map_err(AgentRpcFailure::invalid)?;
    let runtime = context.runtime()?;
    let before = runtime
        .observe_agent_service(context.observation(task_id, source.as_ref()))
        .await?;
    if before.task.status.state.is_terminal() {
        if before.task.status.state == TaskState::Canceled {
            return Ok((
                project_governed_v1_task_with_cancellation(&before.task, true, true, None)?,
                None,
            ));
        }
        return Err(AgentRpcFailure::task_not_cancelable(id));
    }
    let receipt =
        Box::pin(runtime.stop_agent_service(context.observation(task_id, source.as_ref()))).await?;
    Ok((
        project_governed_v1_task_with_cancellation(
            &receipt.task,
            true,
            receipt.cancellation_confirmed,
            receipt.provider_abort.as_ref(),
        )?,
        None,
    ))
}

fn extended_agent_card_rpc(
    context: &AgentRpcContext<'_>,
) -> Result<(Value, Option<String>), AgentRpcFailure> {
    let card = context
        .runtime()?
        .agent_service_card(context.namespace, context.tenant, context.agent)
        .map_err(|_| {
            AgentRpcFailure::extended_card_not_configured("the agent card is unavailable")
        })?;
    if !card.capabilities.extended_agent_card {
        return Err(AgentRpcFailure::extended_card_not_configured(
            "the agent does not publish an extended card",
        ));
    }
    Ok((super::a2a_discovery::project_v1_card(&card), None))
}

async fn dispatch_agent_rpc(
    context: &AgentRpcContext<'_>,
    method: &str,
    params: Value,
) -> Result<(Value, Option<String>), AgentRpcFailure> {
    validate_agent_rpc_tenant(&params, context.tenant)?;
    match method {
        "SendMessage" => Box::pin(send_agent_rpc(context, &params)).await,
        "GetTask" => get_agent_rpc(context, &params).await,
        "GetExtendedAgentCard" => extended_agent_card_rpc(context),
        "CancelTask" => Box::pin(cancel_agent_rpc(context, &params)).await,
        "ListTasks" => Box::pin(list_agent_rpc(context, &params)).await,
        "CreateTaskPushNotificationConfig" => {
            create_governed_push_config_rpc(context, &params).await
        }
        "GetTaskPushNotificationConfig" => get_governed_push_config_rpc(context, &params).await,
        "ListTaskPushNotificationConfigs" => list_governed_push_configs_rpc(context, &params).await,
        "DeleteTaskPushNotificationConfig" => {
            delete_governed_push_config_rpc(context, &params).await
        }
        _ => Err(AgentRpcFailure {
            code: -32601,
            message: format!("method '{method}' is not supported"),
        }),
    }
}

#[derive(Debug)]
struct ParsedAgentRpc {
    id: Value,
    method: String,
    params: Value,
    is_notification: bool,
}

fn invalid_agent_rpc_request(id: Value, message: impl Into<String>) -> Response {
    agent_rpc_response(
        agent_rpc_error(
            id,
            AgentRpcFailure {
                code: -32600,
                message: message.into(),
            },
        ),
        None,
    )
}

fn parse_agent_rpc(body: &str) -> Result<ParsedAgentRpc, Box<Response>> {
    let parsed: Value = serde_json::from_str(body).map_err(|error| {
        Box::new(agent_rpc_response(
            agent_rpc_error(
                Value::Null,
                AgentRpcFailure {
                    code: -32700,
                    message: format!("parse error: {error}"),
                },
            ),
            None,
        ))
    })?;
    let object = parsed.as_object().ok_or_else(|| {
        Box::new(invalid_agent_rpc_request(
            Value::Null,
            "request must be a JSON object",
        ))
    })?;
    let is_notification = !object.contains_key("id");
    let id = object.get("id").cloned().unwrap_or(Value::Null);
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(Box::new(if is_notification {
            StatusCode::NO_CONTENT.into_response()
        } else {
            invalid_agent_rpc_request(id, "the jsonrpc field must be \"2.0\"")
        }));
    }
    let method = object
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            if is_notification {
                Box::new(StatusCode::NO_CONTENT.into_response())
            } else {
                Box::new(invalid_agent_rpc_request(
                    id.clone(),
                    "missing or non-string method",
                ))
            }
        })?
        .to_owned();
    let params = object
        .get("params")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    Ok(ParsedAgentRpc {
        id,
        method,
        params,
        is_notification,
    })
}

/// Official A2A 1.0 JSON-RPC bound to one operator-qualified agent service.
/// The target is carried by the route and can never be selected from a tenant
/// aggregate card or from model-controlled request data.
pub async fn rpc(
    State(state): State<AppState>,
    Extension(identity): Extension<CallerIdentity>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path((namespace, tenant, agent)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: String,
) -> Response {
    if headers
        .get("a2a-version")
        .is_some_and(|value| value != A2A_PROTOCOL_VERSION)
    {
        return agent_rpc_response(
            agent_rpc_error(
                Value::Null,
                AgentRpcFailure {
                    code: -32009,
                    message: format!(
                        "A2A-Version is not supported; this server speaks {A2A_PROTOCOL_VERSION}"
                    ),
                },
            ),
            None,
        );
    }
    let Some(Extension(proof)) = proof else {
        return error(StatusCode::FORBIDDEN, "private_authentication_required");
    };
    let request = match parse_agent_rpc(&body) {
        Ok(request) => request,
        Err(response) => return *response,
    };
    if governed_rpc_requires_management(&request.method, &request.params)
        && !identity.role.has_permission(Permission::OperationsManage)
    {
        return error(
            StatusCode::FORBIDDEN,
            "agent_service_push_requires_operations_manage",
        );
    }
    // Observation remains available to the authenticated original source after
    // role offboarding. Every other method must retain current invocation
    // authority; method-level runtime checks remain authoritative as well.
    if !matches!(request.method.as_str(), "GetTask" | "SubscribeToTask")
        && (!identity.role.has_permission(Permission::Dispatch)
            || !identity.is_authorized(&tenant, &namespace, &format!("agent.{agent}"), "invoke"))
    {
        return error(StatusCode::FORBIDDEN, "agent_service_authority_required");
    }
    if matches!(
        request.method.as_str(),
        "SendMessage" | "SendStreamingMessage"
    ) && let Err((status, body)) =
        super::a2a::enforce_agent_routable(&state, &identity, &namespace, &tenant).await
    {
        return (status, [("a2a-version", A2A_PROTOCOL_VERSION)], body).into_response();
    }
    let context = AgentRpcContext {
        state: &state,
        proof: &proof,
        namespace: &namespace,
        tenant: &tenant,
        agent: &agent,
        headers: &headers,
    };
    if matches!(
        request.method.as_str(),
        "SendStreamingMessage" | "SubscribeToTask"
    ) {
        return Box::pin(handle_agent_streaming_rpc(&context, request)).await;
    }
    let outcome = Box::pin(dispatch_agent_rpc(
        &context,
        &request.method,
        request.params,
    ))
    .await;
    if request.is_notification {
        return StatusCode::NO_CONTENT.into_response();
    }
    match outcome {
        Ok((result, source)) => {
            agent_rpc_response(agent_rpc_success(request.id, result), source.as_deref())
        }
        Err(failure) => agent_rpc_response(agent_rpc_error(request.id, failure), None),
    }
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentAuthorizationOpen {
    /// Opaque verifier-owned handle. Credentials must never be sent here.
    pub authorization_request_id: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentAuthorizationResolve {
    pub challenge_id: String,
}

/// Model/tool input for a configured peer. Authority comes from the accepted
/// source task and private caller authentication, never this body.
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentPeerSend {
    pub message: TaskMessage,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct AgentPeerSendReceipt {
    pub submission_id: String,
    pub status: AgentPeerSendStatus,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct AgentPeerContinuationReceipt {
    pub submission_id: String,
    pub continuation_id: String,
    pub status: AgentPeerContinuationStatus,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct AgentPeerAuthorizationReceipt {
    pub submission_id: String,
    pub authorization_id: String,
    pub status: AgentPeerAuthorizationStatus,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AgentPeerAuthorizationStatus {
    Uncertain,
    Resolved {
        task: Box<acteon_core::Task>,
        progress_cursor: String,
    },
    Rejected {
        code: String,
    },
}

impl From<acteon_executor::delegation::PeerAuthorizationReceipt> for AgentPeerAuthorizationReceipt {
    fn from(receipt: acteon_executor::delegation::PeerAuthorizationReceipt) -> Self {
        Self {
            submission_id: receipt.submission_id.to_string(),
            authorization_id: receipt.authorization_id.to_string(),
            status: match receipt.status {
                PeerAuthorizationStatus::Uncertain => AgentPeerAuthorizationStatus::Uncertain,
                PeerAuthorizationStatus::Resolved {
                    task,
                    progress_cursor,
                } => AgentPeerAuthorizationStatus::Resolved {
                    task,
                    progress_cursor,
                },
                PeerAuthorizationStatus::Rejected { code } => {
                    AgentPeerAuthorizationStatus::Rejected { code }
                }
            },
        }
    }
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AgentPeerContinuationStatus {
    Uncertain,
    Accepted {
        task: Box<acteon_core::Task>,
        progress_cursor: String,
    },
    Rejected {
        code: String,
    },
}

impl From<acteon_executor::delegation::PeerContinuationReceipt> for AgentPeerContinuationReceipt {
    fn from(receipt: acteon_executor::delegation::PeerContinuationReceipt) -> Self {
        Self {
            submission_id: receipt.submission_id.to_string(),
            continuation_id: receipt.continuation_id.to_string(),
            status: match receipt.status {
                PeerContinuationStatus::Uncertain => AgentPeerContinuationStatus::Uncertain,
                PeerContinuationStatus::Accepted {
                    task,
                    progress_cursor,
                } => AgentPeerContinuationStatus::Accepted {
                    task,
                    progress_cursor,
                },
                PeerContinuationStatus::Rejected { code } => {
                    AgentPeerContinuationStatus::Rejected { code }
                }
            },
        }
    }
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AgentPeerSendStatus {
    Uncertain,
    Accepted { task: Box<acteon_core::Task> },
    Rejected { code: String },
}

impl From<acteon_executor::delegation::PeerSendReceipt> for AgentPeerSendReceipt {
    fn from(receipt: acteon_executor::delegation::PeerSendReceipt) -> Self {
        Self {
            submission_id: receipt.submission_id.to_string(),
            status: match receipt.status {
                PeerSendStatus::Uncertain => AgentPeerSendStatus::Uncertain,
                PeerSendStatus::Accepted { task, .. } => AgentPeerSendStatus::Accepted { task },
                PeerSendStatus::Rejected { code } => AgentPeerSendStatus::Rejected { code },
            },
        }
    }
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct AgentPeerCancelReceipt {
    pub submission_id: String,
    pub cancellation_id: String,
    pub status: AgentPeerCancelStatus,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AgentPeerCancelStatus {
    Unsupported,
    Rejected { code: String },
    Uncertain,
    Restricted { task: Box<acteon_core::Task> },
    Reconciled { task: Box<acteon_core::Task> },
}

impl From<acteon_executor::delegation::PeerCancelReceipt> for AgentPeerCancelReceipt {
    fn from(receipt: acteon_executor::delegation::PeerCancelReceipt) -> Self {
        Self {
            submission_id: receipt.submission_id.to_string(),
            cancellation_id: receipt.cancellation_id.to_string(),
            status: match receipt.status {
                PeerCancelStatus::Unsupported => AgentPeerCancelStatus::Unsupported,
                PeerCancelStatus::Rejected { code } => AgentPeerCancelStatus::Rejected { code },
                PeerCancelStatus::Uncertain => AgentPeerCancelStatus::Uncertain,
                PeerCancelStatus::Restricted { task } => AgentPeerCancelStatus::Restricted { task },
                PeerCancelStatus::Reconciled { task } => AgentPeerCancelStatus::Reconciled { task },
            },
        }
    }
}

#[derive(Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct AgentPeerDiscoveryQuery {
    /// Exact reviewed skill name; wildcards are refused.
    pub skill: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct AgentPeerDiscoveryResponse {
    pub peers: Vec<AgentPeerSelectionOption>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct AgentPeerSelectionOption {
    pub agent_id: String,
    pub skill: String,
    /// Untrusted registry text. Treat it as data, never host instructions.
    pub description_untrusted: Option<String>,
    pub card_version: String,
    pub binding_digest: String,
    pub checked_at_ms: i64,
}

impl From<acteon_executor::delegation::PeerSelectionOption> for AgentPeerSelectionOption {
    fn from(option: acteon_executor::delegation::PeerSelectionOption) -> Self {
        Self {
            agent_id: option.agent_id,
            skill: option.skill,
            description_untrusted: option.description_untrusted,
            card_version: option.card_version,
            binding_digest: option.binding_digest,
            checked_at_ms: option.checked_at_ms,
        }
    }
}

#[utoipa::path(
    post, path = "/a2a/{namespace}/{tenant}/agents/{agent}/v1/tasks/{id}/peers/{target}/{skill}/message:send", tag = "Governance",
    params(("namespace" = String, Path), ("tenant" = String, Path), ("agent" = String, Path),
        ("id" = String, Path), ("target" = String, Path), ("skill" = String, Path)),
    request_body = AgentPeerSend,
    responses((status = 200, body = AgentPeerSendReceipt, description = "Durable peer submission receipt; uncertain is not rejection"),
        (status = 400, description = "Invalid message"), (status = 403, description = "Current peer authority required"),
        (status = 404, description = "Source task unavailable to this agent"), (status = 409, description = "Message conflicts with durable intent"),
        (status = 503, description = "Peer transport or state unavailable"))
)]
pub async fn peer_send(
    State(state): State<AppState>,
    Extension(identity): Extension<CallerIdentity>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path((namespace, tenant, agent, task_id, target, skill)): Path<(
        String,
        String,
        String,
        uuid::Uuid,
        String,
        String,
    )>,
    headers: HeaderMap,
    Json(request): Json<AgentPeerSend>,
) -> Response {
    if headers
        .get("a2a-version")
        .is_some_and(|value| value != A2A_PROTOCOL_VERSION)
    {
        return error(StatusCode::BAD_REQUEST, "unsupported_a2a_version");
    }
    if !identity.role.has_permission(Permission::Dispatch)
        || !identity.is_authorized(&tenant, &namespace, &format!("agent.{target}"), "invoke")
    {
        return error(StatusCode::FORBIDDEN, "agent_peer_authority_required");
    }
    let Some(Extension(proof)) = proof else {
        return error(StatusCode::FORBIDDEN, "private_authentication_required");
    };
    let Some(runtime) = &state.execution_authority else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "agent_peer_unavailable");
    };
    match runtime
        .submit_agent_peer_tool(crate::execution_authority::AgentPeerToolRequest {
            namespace: &namespace,
            tenant: &tenant,
            source_agent_id: &agent,
            source_task_id: task_id,
            target_agent_id: &target,
            skill: &skill,
            message: &request.message,
            authentication: &proof,
        })
        .await
    {
        Ok(receipt) => (
            StatusCode::OK,
            [
                ("a2a-version", A2A_PROTOCOL_VERSION),
                ("cache-control", "no-store"),
            ],
            Json(AgentPeerSendReceipt::from(receipt)),
        )
            .into_response(),
        Err(cause) => peer_error(cause),
    }
}

#[utoipa::path(
    get, path = "/a2a/{namespace}/{tenant}/agents/{agent}/v1/tasks/{id}/peers", tag = "Governance",
    params(("namespace" = String, Path), ("tenant" = String, Path), ("agent" = String, Path),
        ("id" = String, Path), AgentPeerDiscoveryQuery),
    responses((status = 200, body = AgentPeerDiscoveryResponse, description = "Current safe peer-selection options; registry descriptions are untrusted"),
        (status = 400, description = "Invalid exact skill query"), (status = 403, description = "Current source authority required"),
        (status = 404, description = "Source task unavailable to this agent"),
        (status = 503, description = "Peer registry, authority, or state unavailable"))
)]
pub async fn peer_discover(
    State(state): State<AppState>,
    Extension(identity): Extension<CallerIdentity>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path((namespace, tenant, agent, task_id)): Path<(String, String, String, uuid::Uuid)>,
    Query(query): Query<AgentPeerDiscoveryQuery>,
    headers: HeaderMap,
) -> Response {
    if headers
        .get("a2a-version")
        .is_some_and(|value| value != A2A_PROTOCOL_VERSION)
    {
        return error(StatusCode::BAD_REQUEST, "unsupported_a2a_version");
    }
    if !identity.role.has_permission(Permission::Dispatch) {
        return error(StatusCode::FORBIDDEN, "agent_peer_authority_required");
    }
    let Some(Extension(proof)) = proof else {
        return error(StatusCode::FORBIDDEN, "private_authentication_required");
    };
    let Some(runtime) = &state.execution_authority else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "agent_peer_unavailable");
    };
    let target_authorized = |target: &str| {
        identity.is_authorized(&tenant, &namespace, &format!("agent.{target}"), "invoke")
    };
    match runtime
        .discover_agent_peers_tool(crate::execution_authority::AgentPeerDiscoveryRequest {
            namespace: &namespace,
            tenant: &tenant,
            source_agent_id: &agent,
            source_task_id: task_id,
            skill: &query.skill,
            authentication: &proof,
            target_authorized: &target_authorized,
        })
        .await
    {
        Ok(options) => {
            let peers = options
                .into_iter()
                .map(AgentPeerSelectionOption::from)
                .collect();
            (
                StatusCode::OK,
                [
                    ("a2a-version", A2A_PROTOCOL_VERSION),
                    ("cache-control", "no-store"),
                ],
                Json(AgentPeerDiscoveryResponse { peers }),
            )
                .into_response()
        }
        Err(cause) => peer_error(cause),
    }
}

/// Axum treats an entire path segment as a parameter, so A2A-style action
/// suffixes (`{id}:refresh`) are split here instead of in the route pattern.
pub async fn peer_submission_action(
    State(state): State<AppState>,
    Extension(identity): Extension<CallerIdentity>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path((namespace, tenant, agent, task_id, target, skill, submission_action)): Path<(
        String,
        String,
        String,
        uuid::Uuid,
        String,
        String,
        String,
    )>,
    headers: HeaderMap,
) -> Response {
    // Preserve the resource-authorization ordering of the logical handlers so
    // malformed action suffixes do not become a target-discovery side channel.
    if headers
        .get("a2a-version")
        .is_some_and(|value| value != A2A_PROTOCOL_VERSION)
    {
        return error(StatusCode::BAD_REQUEST, "unsupported_a2a_version");
    }
    if !identity.role.has_permission(Permission::Dispatch)
        || !identity.is_authorized(&tenant, &namespace, &format!("agent.{target}"), "invoke")
    {
        return error(StatusCode::FORBIDDEN, "agent_peer_authority_required");
    }
    if proof.is_none() {
        return error(StatusCode::FORBIDDEN, "private_authentication_required");
    }
    let Some((submission_id, action)) = parse_peer_submission_action(&submission_action) else {
        return error(StatusCode::BAD_REQUEST, "invalid_agent_peer_request");
    };
    let path = Path((
        namespace,
        tenant,
        agent,
        task_id,
        target,
        skill,
        submission_id,
    ));
    match action {
        PeerSubmissionAction::Refresh => {
            peer_refresh(State(state), Extension(identity), proof, path, headers).await
        }
        PeerSubmissionAction::Cancel => {
            peer_cancel(State(state), Extension(identity), proof, path, headers).await
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum PeerSubmissionAction {
    Refresh,
    Cancel,
}

fn parse_peer_submission_action(value: &str) -> Option<(uuid::Uuid, PeerSubmissionAction)> {
    let (submission, action) = if let Some(submission) = value.strip_suffix(":refresh") {
        (submission, PeerSubmissionAction::Refresh)
    } else {
        let submission = value.strip_suffix(":cancel")?;
        (submission, PeerSubmissionAction::Cancel)
    };
    uuid::Uuid::parse_str(submission)
        .ok()
        .map(|submission| (submission, action))
}

#[utoipa::path(
    post, path = "/a2a/{namespace}/{tenant}/agents/{agent}/v1/tasks/{id}/peers/{target}/{skill}/submissions/{submission}:refresh", tag = "Governance",
    params(("namespace" = String, Path), ("tenant" = String, Path), ("agent" = String, Path),
        ("id" = String, Path), ("target" = String, Path), ("skill" = String, Path),
        ("submission" = String, Path, description = "Stable peer submission UUID")),
    responses((status = 200, body = AgentPeerSendReceipt, description = "Last durable peer receipt after an authorized remote observation"),
        (status = 400, description = "Invalid lifecycle request"), (status = 403, description = "Current peer authority required"),
        (status = 404, description = "Source task unavailable to this agent"), (status = 409, description = "Submission does not match its durable binding"),
        (status = 503, description = "Peer transport, remote task, or state unavailable"))
)]
pub async fn peer_refresh(
    State(state): State<AppState>,
    Extension(identity): Extension<CallerIdentity>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path((namespace, tenant, agent, task_id, target, skill, submission_id)): Path<(
        String,
        String,
        String,
        uuid::Uuid,
        String,
        String,
        uuid::Uuid,
    )>,
    headers: HeaderMap,
) -> Response {
    if headers
        .get("a2a-version")
        .is_some_and(|value| value != A2A_PROTOCOL_VERSION)
    {
        return error(StatusCode::BAD_REQUEST, "unsupported_a2a_version");
    }
    if !identity.role.has_permission(Permission::Dispatch)
        || !identity.is_authorized(&tenant, &namespace, &format!("agent.{target}"), "invoke")
    {
        return error(StatusCode::FORBIDDEN, "agent_peer_authority_required");
    }
    let Some(Extension(proof)) = proof else {
        return error(StatusCode::FORBIDDEN, "private_authentication_required");
    };
    let Some(runtime) = &state.execution_authority else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "agent_peer_unavailable");
    };
    match runtime
        .refresh_agent_peer_tool(crate::execution_authority::AgentPeerRefreshRequest {
            namespace: &namespace,
            tenant: &tenant,
            source_agent_id: &agent,
            source_task_id: task_id,
            target_agent_id: &target,
            skill: &skill,
            submission_id,
            authentication: &proof,
        })
        .await
    {
        Ok(receipt) => (
            StatusCode::OK,
            [
                ("a2a-version", A2A_PROTOCOL_VERSION),
                ("cache-control", "no-store"),
            ],
            Json(AgentPeerSendReceipt::from(receipt)),
        )
            .into_response(),
        Err(cause) => peer_error(cause),
    }
}

#[utoipa::path(
    post, path = "/a2a/{namespace}/{tenant}/agents/{agent}/v1/tasks/{id}/peers/{target}/{skill}/submissions/{submission}:cancel", tag = "Governance",
    params(("namespace" = String, Path), ("tenant" = String, Path), ("agent" = String, Path),
        ("id" = String, Path), ("target" = String, Path), ("skill" = String, Path),
        ("submission" = String, Path, description = "Stable peer submission UUID")),
    responses((status = 200, body = AgentPeerCancelReceipt, description = "Durable at-most-once remote cancellation receipt"),
        (status = 400, description = "Invalid lifecycle request"), (status = 403, description = "Current peer authority required"),
        (status = 404, description = "Source task unavailable to this agent"), (status = 409, description = "Submission does not match its durable binding"),
        (status = 503, description = "Peer transport or state unavailable"))
)]
pub async fn peer_cancel(
    State(state): State<AppState>,
    Extension(identity): Extension<CallerIdentity>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path((namespace, tenant, agent, task_id, target, skill, submission_id)): Path<(
        String,
        String,
        String,
        uuid::Uuid,
        String,
        String,
        uuid::Uuid,
    )>,
    headers: HeaderMap,
) -> Response {
    if headers
        .get("a2a-version")
        .is_some_and(|value| value != A2A_PROTOCOL_VERSION)
    {
        return error(StatusCode::BAD_REQUEST, "unsupported_a2a_version");
    }
    if !identity.role.has_permission(Permission::Dispatch)
        || !identity.is_authorized(&tenant, &namespace, &format!("agent.{target}"), "invoke")
    {
        return error(StatusCode::FORBIDDEN, "agent_peer_authority_required");
    }
    let Some(Extension(proof)) = proof else {
        return error(StatusCode::FORBIDDEN, "private_authentication_required");
    };
    let Some(runtime) = &state.execution_authority else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "agent_peer_unavailable");
    };
    match runtime
        .cancel_agent_peer_tool(crate::execution_authority::AgentPeerCancelRequest {
            namespace: &namespace,
            tenant: &tenant,
            source_agent_id: &agent,
            source_task_id: task_id,
            target_agent_id: &target,
            skill: &skill,
            submission_id,
            authentication: &proof,
        })
        .await
    {
        Ok(receipt) => (
            StatusCode::OK,
            [
                ("a2a-version", A2A_PROTOCOL_VERSION),
                ("cache-control", "no-store"),
            ],
            Json(AgentPeerCancelReceipt::from(receipt)),
        )
            .into_response(),
        Err(cause) => peer_error(cause),
    }
}

#[utoipa::path(
    post, path = "/a2a/{namespace}/{tenant}/agents/{agent}/v1/tasks/{id}/peers/{target}/{skill}/submissions/{submission}/message:send", tag = "Governance",
    params(("namespace" = String, Path), ("tenant" = String, Path), ("agent" = String, Path),
        ("id" = String, Path), ("target" = String, Path), ("skill" = String, Path),
        ("submission" = String, Path, description = "Stable peer submission UUID")),
    request_body = AgentPeerSend,
    responses((status = 200, body = AgentPeerContinuationReceipt, description = "Durable at-most-once response to the accepted peer task's active challenge"),
        (status = 400, description = "Invalid unbound response"), (status = 403, description = "Current peer authority required"),
        (status = 404, description = "Source task unavailable to this agent"), (status = 409, description = "Response conflicts with durable intent"),
        (status = 503, description = "Peer transport or state unavailable"))
)]
pub async fn peer_continue(
    State(state): State<AppState>,
    Extension(identity): Extension<CallerIdentity>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path((namespace, tenant, agent, task_id, target, skill, submission_id)): Path<(
        String,
        String,
        String,
        uuid::Uuid,
        String,
        String,
        uuid::Uuid,
    )>,
    headers: HeaderMap,
    Json(request): Json<AgentPeerSend>,
) -> Response {
    if headers
        .get("a2a-version")
        .is_some_and(|value| value != A2A_PROTOCOL_VERSION)
    {
        return error(StatusCode::BAD_REQUEST, "unsupported_a2a_version");
    }
    if !identity.role.has_permission(Permission::Dispatch)
        || !identity.is_authorized(&tenant, &namespace, &format!("agent.{target}"), "invoke")
    {
        return error(StatusCode::FORBIDDEN, "agent_peer_authority_required");
    }
    let Some(Extension(proof)) = proof else {
        return error(StatusCode::FORBIDDEN, "private_authentication_required");
    };
    let Some(runtime) = &state.execution_authority else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "agent_peer_unavailable");
    };
    match runtime
        .continue_agent_peer_tool(crate::execution_authority::AgentPeerContinuationRequest {
            namespace: &namespace,
            tenant: &tenant,
            source_agent_id: &agent,
            source_task_id: task_id,
            target_agent_id: &target,
            skill: &skill,
            submission_id,
            response: &request.message,
            authentication: &proof,
        })
        .await
    {
        Ok(receipt) => (
            StatusCode::OK,
            [
                ("a2a-version", A2A_PROTOCOL_VERSION),
                ("cache-control", "no-store"),
            ],
            Json(AgentPeerContinuationReceipt::from(receipt)),
        )
            .into_response(),
        Err(cause) => peer_error(cause),
    }
}

#[utoipa::path(
    post, path = "/a2a/{namespace}/{tenant}/agents/{agent}/v1/tasks/{id}/peers/{target}/{skill}/submissions/{submission}/authorization:resolve", tag = "Governance",
    params(("namespace" = String, Path), ("tenant" = String, Path), ("agent" = String, Path),
        ("id" = String, Path), ("target" = String, Path), ("skill" = String, Path),
        ("submission" = String, Path, description = "Stable peer submission UUID")),
    request_body = AgentAuthorizationResolve,
    responses((status = 200, body = AgentPeerAuthorizationReceipt, description = "Durable remote authorization handoff receipt"),
        (status = 400, description = "Invalid challenge selector"), (status = 403, description = "Current peer authority required"),
        (status = 404, description = "Source task unavailable to this agent"), (status = 409, description = "Challenge conflicts with durable peer state"),
        (status = 503, description = "Peer transport, target verifier, or state unavailable"))
)]
pub async fn peer_authorization_resolve(
    State(state): State<AppState>,
    Extension(identity): Extension<CallerIdentity>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path((namespace, tenant, agent, task_id, target, skill, submission_id)): Path<(
        String,
        String,
        String,
        uuid::Uuid,
        String,
        String,
        uuid::Uuid,
    )>,
    headers: HeaderMap,
    Json(request): Json<AgentAuthorizationResolve>,
) -> Response {
    if headers
        .get("a2a-version")
        .is_some_and(|value| value != A2A_PROTOCOL_VERSION)
    {
        return error(StatusCode::BAD_REQUEST, "unsupported_a2a_version");
    }
    if !identity.role.has_permission(Permission::Dispatch)
        || !identity.is_authorized(&tenant, &namespace, &format!("agent.{target}"), "invoke")
    {
        return error(StatusCode::FORBIDDEN, "agent_peer_authority_required");
    }
    let Some(Extension(proof)) = proof else {
        return error(StatusCode::FORBIDDEN, "private_authentication_required");
    };
    let Some(runtime) = &state.execution_authority else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "agent_peer_unavailable");
    };
    match runtime
        .authorize_agent_peer_tool(crate::execution_authority::AgentPeerAuthorizationRequest {
            namespace: &namespace,
            tenant: &tenant,
            source_agent_id: &agent,
            source_task_id: task_id,
            target_agent_id: &target,
            skill: &skill,
            submission_id,
            challenge_id: &request.challenge_id,
            authentication: &proof,
        })
        .await
    {
        Ok(receipt) => (
            StatusCode::OK,
            [
                ("a2a-version", A2A_PROTOCOL_VERSION),
                ("cache-control", "no-store"),
            ],
            Json(AgentPeerAuthorizationReceipt::from(receipt)),
        )
            .into_response(),
        Err(cause) => peer_error(cause),
    }
}

#[utoipa::path(
    post, path = "/a2a/{namespace}/{tenant}/agents/{agent}/v1/message:send", tag = "Governance",
    params(("namespace" = String, Path), ("tenant" = String, Path), ("agent" = String, Path),
        ("x-acteon-execution-context" = Option<String>, Header, description = "URL-safe base64 encoded parent execution-context reference; requires matching private caller authentication"),
        ("x-acteon-execution-permits" = Option<String>, Header, description = "JSON array of the parent context's explicit permit references; required with x-acteon-execution-context")),
    request_body = AgentMessageSend,
    responses((status = 200, body = acteon_core::Task, description = "Durably accepted task; acceptance is not provider execution"),
        (status = 400, description = "Invalid message or unsupported A2A version"), (status = 403, description = "Original service authority required"),
        (status = 404, description = "Service unavailable"), (status = 409, description = "Accepted input or authority conflict"), (status = 429, description = "Capacity or budget exhausted"), (status = 503, description = "Service runtime unavailable"))
)]
pub async fn message_send(
    State(state): State<AppState>,
    Extension(identity): Extension<CallerIdentity>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path((namespace, tenant, agent)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(request): Json<AgentMessageSend>,
) -> Response {
    if headers
        .get("a2a-version")
        .is_some_and(|v| v != A2A_PROTOCOL_VERSION)
    {
        return error(StatusCode::BAD_REQUEST, "unsupported_a2a_version");
    }
    if !identity.role.has_permission(Permission::Dispatch)
        || !identity.is_authorized(&tenant, &namespace, &format!("agent.{agent}"), "invoke")
    {
        return error(StatusCode::FORBIDDEN, "agent_service_authority_required");
    }
    let Some(Extension(proof)) = proof else {
        return error(StatusCode::FORBIDDEN, "private_authentication_required");
    };
    let (Some(runtime), Some(authentication)) = (&state.execution_authority, &state.auth) else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "agent_services_unavailable",
        );
    };
    let parent_input = match parse_parent(&headers) {
        Ok(parent) => parent,
        Err(code) => return error(StatusCode::BAD_REQUEST, code),
    };
    let parent = parent_input
        .as_ref()
        .map(|(context, permits)| AgentServiceParent { context, permits });
    match Box::pin(runtime.accept_agent_service(AgentServiceRequest {
        namespace: &namespace,
        tenant: &tenant,
        agent_id: &agent,
        message: &request.message,
        authentication: &proof,
        auth_provider: authentication,
        parent,
        initial_push_configs: &[],
    }))
    .await
    {
        Ok(accepted) => {
            let Ok(source) = serde_json::to_vec(&accepted.source_context) else {
                return error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "source_context_unavailable",
                );
            };
            let cursor = super::a2a::task_cursor(accepted.task_version, None);
            (
                StatusCode::OK,
                [
                    ("a2a-version", A2A_PROTOCOL_VERSION.to_string()),
                    ("cache-control", "no-store".to_string()),
                    (SOURCE_CONTEXT_HEADER, URL_SAFE_NO_PAD.encode(source)),
                    ("etag", cursor),
                ],
                Json(accepted.task),
            )
                .into_response()
        }
        Err(cause) => service_error(cause),
    }
}

fn parse_execution_context(
    headers: &HeaderMap,
) -> Result<Option<acteon_core::ExecutionContextReference>, &'static str> {
    let values: Vec<_> = headers.get_all(EXECUTION_CONTEXT_HEADER).iter().collect();
    if values.is_empty() {
        return Ok(None);
    }
    if values.len() != 1 || values[0].as_bytes().len() > 8192 {
        return Err("invalid_execution_context");
    }
    values[0]
        .to_str()
        .ok()
        .and_then(|value| URL_SAFE_NO_PAD.decode(value).ok())
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .map(Some)
        .ok_or("invalid_execution_context")
}

fn parse_parent(
    headers: &HeaderMap,
) -> Result<
    Option<(
        acteon_core::ExecutionContextReference,
        Vec<acteon_governance::permit::PermitReference>,
    )>,
    &'static str,
> {
    let context = parse_execution_context(headers)?;
    if context.is_none() && headers.contains_key("x-acteon-execution-permits") {
        return Err("execution_context_required");
    }
    let permits = super::dispatch::execution_permits(headers, true, context.is_some())
        .map_err(|_| "invalid_execution_permits")?;
    match context {
        Some(_) if permits.is_empty() => Err("invalid_execution_permits"),
        Some(context) => Ok(Some((context, permits))),
        None => Ok(None),
    }
}

#[utoipa::path(
    get, path = "/a2a/{namespace}/{tenant}/agents/{agent}/v1/tasks/{id}", tag = "Governance",
    params(("namespace" = String, Path), ("tenant" = String, Path), ("agent" = String, Path),
        ("id" = String, Path), ("x-acteon-agent-source-context" = Option<String>, Header, description = "Exact source context returned by admission; mandatory for agent requesters")),
    responses((status = 200, body = acteon_core::Task), (status = 403, description = "Private authentication required"),
        (status = 404, description = "Task unavailable to this requester"), (status = 503, description = "Runtime unavailable"))
)]
pub async fn task_get(
    State(state): State<AppState>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path((namespace, tenant, agent, task_id)): Path<(String, String, String, uuid::Uuid)>,
    headers: HeaderMap,
) -> Response {
    let Some(Extension(proof)) = proof else {
        return error(StatusCode::FORBIDDEN, "private_authentication_required");
    };
    if headers
        .get("a2a-version")
        .is_some_and(|v| v != A2A_PROTOCOL_VERSION)
    {
        return error(StatusCode::BAD_REQUEST, "unsupported_a2a_version");
    }
    let source = match parse_source_context(&headers) {
        Ok(source) => source,
        Err(code) => return error(StatusCode::BAD_REQUEST, code),
    };
    let Some(runtime) = &state.execution_authority else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "agent_services_unavailable",
        );
    };
    match runtime
        .observe_agent_service(AgentServiceObservation {
            namespace: &namespace,
            tenant: &tenant,
            agent_id: &agent,
            task_id,
            authentication: &proof,
            source_context: source.as_ref(),
        })
        .await
    {
        Ok(observed) => {
            let cursor = super::a2a::task_cursor(observed.task_version, None);
            if headers
                .get(axum::http::header::IF_NONE_MATCH)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value == cursor)
            {
                return (
                    StatusCode::NOT_MODIFIED,
                    [
                        ("a2a-version", A2A_PROTOCOL_VERSION.to_string()),
                        ("cache-control", "no-store".to_string()),
                        ("etag", cursor),
                    ],
                )
                    .into_response();
            }
            (
                StatusCode::OK,
                [
                    ("a2a-version", A2A_PROTOCOL_VERSION.to_string()),
                    ("cache-control", "no-store".to_string()),
                    ("etag", cursor),
                ],
                Json(observed.task),
            )
                .into_response()
        }
        Err(cause) => service_error(cause),
    }
}

#[utoipa::path(
    post, path = "/a2a/{namespace}/{tenant}/agents/{agent}/v1/tasks/{id}/message:send", tag = "Governance",
    params(("namespace" = String, Path), ("tenant" = String, Path), ("agent" = String, Path),
        ("id" = String, Path), ("x-acteon-agent-source-context" = Option<String>, Header, description = "Exact source context returned by admission; mandatory for agent requesters")),
    request_body = AgentMessageSend,
    responses((status = 200, body = acteon_core::Task, description = "Task after durable response registration and governed continuation admission"),
        (status = 400, description = "Invalid response or challenge binding"), (status = 403, description = "Private authentication required"),
        (status = 404, description = "Task unavailable to this requester"), (status = 409, description = "Response conflicts with durable task state"),
        (status = 429, description = "Continuation budget or capacity exhausted"), (status = 503, description = "Runtime unavailable"))
)]
pub async fn task_continue(
    State(state): State<AppState>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path((namespace, tenant, agent, task_id)): Path<(String, String, String, uuid::Uuid)>,
    headers: HeaderMap,
    Json(request): Json<AgentMessageSend>,
) -> Response {
    let Some(Extension(proof)) = proof else {
        return error(StatusCode::FORBIDDEN, "private_authentication_required");
    };
    if headers
        .get("a2a-version")
        .is_some_and(|value| value != A2A_PROTOCOL_VERSION)
    {
        return error(StatusCode::BAD_REQUEST, "unsupported_a2a_version");
    }
    let source = match parse_source_context(&headers) {
        Ok(source) => source,
        Err(code) => return error(StatusCode::BAD_REQUEST, code),
    };
    let Some(challenge_id) = request
        .message
        .metadata
        .get(acteon_core::TASK_CHALLENGE_ID_METADATA_KEY)
        .and_then(serde_json::Value::as_str)
    else {
        return error(StatusCode::BAD_REQUEST, "invalid_agent_service_request");
    };
    let Some(runtime) = &state.execution_authority else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "agent_services_unavailable",
        );
    };
    if let Some(expected) = headers.get(axum::http::header::IF_MATCH) {
        let Ok(expected) = expected.to_str() else {
            return error(StatusCode::BAD_REQUEST, "invalid_task_cursor");
        };
        let observed = runtime
            .observe_agent_service(AgentServiceObservation {
                namespace: &namespace,
                tenant: &tenant,
                agent_id: &agent,
                task_id,
                authentication: &proof,
                source_context: source.as_ref(),
            })
            .await;
        let observed = match observed {
            Ok(observed) => observed,
            Err(cause) => return service_error(cause),
        };
        if expected != super::a2a::task_cursor(observed.task_version, None) {
            return error(StatusCode::CONFLICT, "task_cursor_changed");
        }
    }
    match runtime
        .continue_agent_service(AgentServiceContinuation {
            observation: AgentServiceObservation {
                namespace: &namespace,
                tenant: &tenant,
                agent_id: &agent,
                task_id,
                authentication: &proof,
                source_context: source.as_ref(),
            },
            challenge_id,
            response: &request.message,
        })
        .await
    {
        Ok(observed) => (
            StatusCode::OK,
            [
                ("a2a-version", A2A_PROTOCOL_VERSION.to_string()),
                ("cache-control", "no-store".to_string()),
                ("etag", super::a2a::task_cursor(observed.task_version, None)),
            ],
            Json(observed.task),
        )
            .into_response(),
        Err(cause) => service_error(cause),
    }
}

#[utoipa::path(
    post, path = "/a2a/{namespace}/{tenant}/agents/{agent}/v1/tasks/{id}/authorization:request", tag = "Governance",
    params(("namespace" = String, Path), ("tenant" = String, Path), ("agent" = String, Path), ("id" = String, Path)),
    request_body = AgentAuthorizationOpen,
    responses((status = 200, body = acteon_core::Task, description = "Task paused on the host-bound authorization profile"),
        (status = 400, description = "Invalid opaque authorization request"), (status = 403, description = "Configured recipient authentication required"),
        (status = 404, description = "Task unavailable to this recipient"), (status = 409, description = "Task state changed"),
        (status = 503, description = "Runtime unavailable"))
)]
pub async fn task_authorization_request(
    State(state): State<AppState>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path((namespace, tenant, agent, task_id)): Path<(String, String, String, uuid::Uuid)>,
    headers: HeaderMap,
    Json(request): Json<AgentAuthorizationOpen>,
) -> Response {
    let Some(Extension(proof)) = proof else {
        return error(StatusCode::FORBIDDEN, "private_authentication_required");
    };
    if headers
        .get("a2a-version")
        .is_some_and(|value| value != A2A_PROTOCOL_VERSION)
    {
        return error(StatusCode::BAD_REQUEST, "unsupported_a2a_version");
    }
    let Some(runtime) = &state.execution_authority else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "agent_services_unavailable",
        );
    };
    match runtime
        .open_agent_service_authorization(AgentServiceAuthorizationOpen {
            namespace: &namespace,
            tenant: &tenant,
            agent_id: &agent,
            task_id,
            authentication: &proof,
            authorization_request_id: &request.authorization_request_id,
        })
        .await
    {
        Ok(observed) => (
            StatusCode::OK,
            [
                ("a2a-version", A2A_PROTOCOL_VERSION.to_string()),
                ("cache-control", "no-store".to_string()),
                ("etag", super::a2a::task_cursor(observed.task_version, None)),
            ],
            Json(observed.task),
        )
            .into_response(),
        Err(cause) => service_error(cause),
    }
}

#[utoipa::path(
    post, path = "/a2a/{namespace}/{tenant}/agents/{agent}/v1/tasks/{id}/authorization:resolve", tag = "Governance",
    params(("namespace" = String, Path), ("tenant" = String, Path), ("agent" = String, Path),
        ("id" = String, Path), ("x-acteon-agent-source-context" = Option<String>, Header, description = "Original admission source context; mandatory for agent requesters")),
    request_body = AgentAuthorizationResolve,
    responses((status = 200, body = acteon_core::Task, description = "Task after verifier-backed authorization resolution"),
        (status = 400, description = "Invalid challenge binding"), (status = 403, description = "Original requester authentication required or verifier denied"),
        (status = 404, description = "Task unavailable to this requester"), (status = 409, description = "Challenge conflicts with task state"),
        (status = 503, description = "Verifier or runtime unavailable"))
)]
pub async fn task_authorization_resolve(
    State(state): State<AppState>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path((namespace, tenant, agent, task_id)): Path<(String, String, String, uuid::Uuid)>,
    headers: HeaderMap,
    Json(request): Json<AgentAuthorizationResolve>,
) -> Response {
    let Some(Extension(proof)) = proof else {
        return error(StatusCode::FORBIDDEN, "private_authentication_required");
    };
    if headers
        .get("a2a-version")
        .is_some_and(|value| value != A2A_PROTOCOL_VERSION)
    {
        return error(StatusCode::BAD_REQUEST, "unsupported_a2a_version");
    }
    let source = match parse_source_context(&headers) {
        Ok(source) => source,
        Err(code) => return error(StatusCode::BAD_REQUEST, code),
    };
    let Some(runtime) = &state.execution_authority else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "agent_services_unavailable",
        );
    };
    if let Some(expected) = headers.get(axum::http::header::IF_MATCH) {
        let Ok(expected) = expected.to_str() else {
            return error(StatusCode::BAD_REQUEST, "invalid_task_cursor");
        };
        let observed = runtime
            .observe_agent_service(AgentServiceObservation {
                namespace: &namespace,
                tenant: &tenant,
                agent_id: &agent,
                task_id,
                authentication: &proof,
                source_context: source.as_ref(),
            })
            .await;
        let observed = match observed {
            Ok(observed) => observed,
            Err(cause) => return service_error(cause),
        };
        if expected != super::a2a::task_cursor(observed.task_version, None) {
            return error(StatusCode::CONFLICT, "task_cursor_changed");
        }
    }
    match runtime
        .resolve_agent_service_authorization(AgentServiceAuthorizationResolve {
            observation: AgentServiceObservation {
                namespace: &namespace,
                tenant: &tenant,
                agent_id: &agent,
                task_id,
                authentication: &proof,
                source_context: source.as_ref(),
            },
            challenge_id: &request.challenge_id,
        })
        .await
    {
        Ok(observed) => (
            StatusCode::OK,
            [
                ("a2a-version", A2A_PROTOCOL_VERSION.to_string()),
                ("cache-control", "no-store".to_string()),
                ("etag", super::a2a::task_cursor(observed.task_version, None)),
            ],
            Json(observed.task),
        )
            .into_response(),
        Err(cause) => service_error(cause),
    }
}

/// Durable cancellation acknowledgement. `cancellation_confirmed` is true only
/// for a fenced local non-start or qualified provider no-effect finality.
#[derive(Serialize, utoipa::ToSchema)]
pub struct AgentServiceStopResponse {
    pub task: acteon_core::Task,
    pub future_starts_blocked: bool,
    pub cancellation_confirmed: bool,
    /// Provider-side outcome is distinct from the durable future-start fence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_abort: Option<AgentServiceProviderAbort>,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AgentServiceProviderAbort {
    RestrictedOnly,
    Uncertain { attempt_id: String },
    Reconciled { proof_digest: String },
}

impl From<acteon_executor::governed::abort::ProviderAbortStatus> for AgentServiceProviderAbort {
    fn from(status: acteon_executor::governed::abort::ProviderAbortStatus) -> Self {
        use acteon_executor::governed::abort::ProviderAbortStatus;
        match status {
            ProviderAbortStatus::RestrictedOnly => Self::RestrictedOnly,
            ProviderAbortStatus::Uncertain { attempt_id } => Self::Uncertain { attempt_id },
            ProviderAbortStatus::Reconciled { proof_digest } => Self::Reconciled { proof_digest },
        }
    }
}

#[utoipa::path(
    post, path = "/a2a/{namespace}/{tenant}/agents/{agent}/v1/tasks/{id}/stop", tag = "Governance",
    params(("namespace" = String, Path), ("tenant" = String, Path), ("agent" = String, Path),
        ("id" = String, Path), ("x-acteon-agent-source-context" = Option<String>, Header, description = "Original admission source context; mandatory for agent requesters")),
    responses((status = 200, body = AgentServiceStopResponse),
        (status = 404, description = "Task unavailable to this requester"),
        (status = 503, description = "Restriction or observation acknowledgement unavailable; retry the same task"))
)]
pub async fn task_stop(
    State(state): State<AppState>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path((namespace, tenant, agent, task_id)): Path<(String, String, String, uuid::Uuid)>,
    headers: HeaderMap,
) -> Response {
    let Some(Extension(proof)) = proof else {
        return error(StatusCode::FORBIDDEN, "private_authentication_required");
    };
    if headers
        .get("a2a-version")
        .is_some_and(|v| v != A2A_PROTOCOL_VERSION)
    {
        return error(StatusCode::BAD_REQUEST, "unsupported_a2a_version");
    }
    let source = match parse_source_context(&headers) {
        Ok(source) => source,
        Err(code) => return error(StatusCode::BAD_REQUEST, code),
    };
    let Some(runtime) = &state.execution_authority else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "agent_services_unavailable",
        );
    };
    match Box::pin(runtime.stop_agent_service(AgentServiceObservation {
        namespace: &namespace,
        tenant: &tenant,
        agent_id: &agent,
        task_id,
        authentication: &proof,
        source_context: source.as_ref(),
    }))
    .await
    {
        Ok(receipt) => (
            StatusCode::OK,
            [
                ("a2a-version", A2A_PROTOCOL_VERSION),
                ("cache-control", "no-store"),
            ],
            Json(AgentServiceStopResponse {
                task: receipt.task,
                future_starts_blocked: receipt.future_starts_blocked,
                cancellation_confirmed: receipt.cancellation_confirmed,
                provider_abort: receipt.provider_abort.map(Into::into),
            }),
        )
            .into_response(),
        Err(cause) => service_error(cause),
    }
}

fn parse_source_context(
    headers: &HeaderMap,
) -> Result<Option<acteon_core::ExecutionContextReference>, &'static str> {
    let Some(value) = headers.get(SOURCE_CONTEXT_HEADER) else {
        return Ok(None);
    };
    value
        .to_str()
        .ok()
        .filter(|v| v.len() <= 8192)
        .and_then(|v| URL_SAFE_NO_PAD.decode(v).ok())
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .map(Some)
        .ok_or("invalid_source_context")
}

fn service_error(cause: AgentServiceError) -> Response {
    let (status, code) = match cause {
        AgentServiceError::Invalid => (StatusCode::BAD_REQUEST, "invalid_agent_service_request"),
        AgentServiceError::Forbidden => (StatusCode::FORBIDDEN, "agent_service_authority_required"),
        AgentServiceError::NotFound => (StatusCode::NOT_FOUND, "service_task_unavailable"),
        AgentServiceError::Conflict => (StatusCode::CONFLICT, "agent_service_conflict"),
        AgentServiceError::Limits => (
            StatusCode::TOO_MANY_REQUESTS,
            "agent_service_limits_exhausted",
        ),
        AgentServiceError::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "agent_services_unavailable",
        ),
    };
    error(status, code)
}

fn peer_error(cause: crate::execution_authority::AgentPeerTransportError) -> Response {
    use crate::execution_authority::AgentPeerTransportError;
    let (status, code) = match cause {
        AgentPeerTransportError::Invalid => (StatusCode::BAD_REQUEST, "invalid_agent_peer_request"),
        AgentPeerTransportError::Forbidden => {
            (StatusCode::FORBIDDEN, "agent_peer_authority_required")
        }
        AgentPeerTransportError::NotFound => {
            (StatusCode::NOT_FOUND, "agent_peer_source_unavailable")
        }
        AgentPeerTransportError::Conflict => {
            (StatusCode::CONFLICT, "agent_peer_submission_conflict")
        }
        AgentPeerTransportError::Unavailable => {
            (StatusCode::SERVICE_UNAVAILABLE, "agent_peer_unavailable")
        }
    };
    error(status, code)
}

fn error(status: StatusCode, code: &str) -> Response {
    (
        status,
        [
            ("a2a-version", A2A_PROTOCOL_VERSION),
            ("cache-control", "no-store"),
        ],
        Json(ErrorResponse { error: code.into() }),
    )
        .into_response()
}

#[cfg(test)]
mod rpc_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn official_message_role_is_normalized_without_accepting_missing_content() {
        let message = decode_agent_rpc_message(&json!({
            "message": {
                "role": "ROLE_USER",
                "messageId": "m-1",
                "parts": [{"kind":"text", "text":"inspect"}]
            }
        }))
        .unwrap();
        assert_eq!(message.role, acteon_core::TaskRole::User);
        assert!(decode_agent_rpc_message(&json!({})).is_err());
    }

    #[test]
    fn governed_rpc_marks_inline_push_as_management_and_preserves_send_options() {
        let params = json!({
            "configuration": {
                "returnImmediately": true,
                "taskPushNotificationConfig": {"url":"https://example.test/push"}
            }
        });
        assert!(agent_rpc_return_immediately(&params).unwrap());
        assert!(governed_rpc_requires_management("SendMessage", &params));
    }

    #[test]
    fn governed_rpc_binds_tenant_to_the_selected_interface() {
        let failure = validate_agent_rpc_tenant(&json!({"tenant":"other"}), "acme").unwrap_err();
        assert_eq!(failure.code, -32602);
        validate_agent_rpc_tenant(&json!({"tenant":"acme"}), "acme").unwrap();
    }

    #[test]
    fn governed_projection_removes_private_runtime_binding_metadata() {
        let mut task = Task::new("11111111-1111-4111-8111-111111111111", "prod", "acme");
        task.metadata.insert(
            acteon_gateway::agent_runtime::GOVERNED_TASK_METADATA_KEY.into(),
            json!({"execution_id":"private", "binding_digest":"private"}),
        );
        task.metadata.insert("public-label".into(), json!("kept"));
        let projected = project_governed_v1_task(&task).unwrap();
        assert!(
            projected
                .pointer("/metadata/acteon_governed_execution")
                .is_none()
        );
        assert_eq!(
            projected.pointer("/metadata/public-label"),
            Some(&json!("kept"))
        );
        assert!(projected.get("namespace").is_none());
    }

    #[test]
    fn exact_agent_rpc_accepts_one_request_and_rejects_batches() {
        let parsed = parse_agent_rpc(
            r#"{"jsonrpc":"2.0","method":"GetTask","params":{"id":"task"},"id":7}"#,
        )
        .unwrap();
        assert_eq!(parsed.method, "GetTask");
        assert_eq!(parsed.id, json!(7));
        assert!(parse_agent_rpc("[]").is_err());
    }

    #[test]
    fn governed_rpc_uses_canonical_typed_capability_errors() {
        let response = agent_rpc_error(
            json!(3),
            AgentRpcFailure::extended_card_not_configured("missing"),
        );
        let encoded = serde_json::to_value(response).unwrap();
        assert_eq!(encoded["error"]["code"], -32007);
        assert_eq!(
            encoded.pointer("/error/data/0/reason"),
            Some(&json!("EXTENDED_AGENT_CARD_NOT_CONFIGURED"))
        );
    }

    #[test]
    fn governed_list_cursor_is_schema_closed_and_filter_bound() {
        let cursor = GovernedListTasksCursor {
            schema: 1,
            updated_at: "2026-10-10T12:00:00Z".parse().unwrap(),
            task_id: "11111111-1111-4111-8111-111111111111".into(),
            visibility_binding: "a".repeat(64),
            page_size: 25,
            context_id: Some("incident-42".into()),
            status: Some("TASK_STATE_WORKING".into()),
            history_length: Some(2),
            status_timestamp_after: Some("2026-10-10T11:00:00Z".parse().unwrap()),
            include_artifacts: true,
        };
        let encoded = encode_governed_list_cursor(&cursor).unwrap();
        assert_eq!(decode_governed_list_cursor(&encoded).unwrap(), cursor);
        assert!(decode_governed_list_cursor("not-base64!").is_err());

        let invalid = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({
                "schema":2,
                "updatedAt":"2026-10-10T12:00:00Z",
                "taskId":"11111111-1111-4111-8111-111111111111",
                "visibilityBinding":"binding",
                "pageSize":25,
                "contextId":null,"status":null,"historyLength":null,
                "statusTimestampAfter":null,"includeArtifacts":false
            }))
            .unwrap(),
        );
        assert!(decode_governed_list_cursor(&invalid).is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_context_header_is_single_bounded_and_typed() {
        let context = serde_json::json!({
            "context_id":"11111111-1111-4111-8111-111111111111",
            "execution_id":"22222222-2222-4222-8222-222222222222",
            "namespace":"prod", "tenant":"acme",
            "principal":{"id":"caller-agent","kind":"agent"},
            "request_digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        });
        let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&context).unwrap());
        let mut headers = HeaderMap::new();
        headers.insert(EXECUTION_CONTEXT_HEADER, encoded.parse().unwrap());
        assert!(parse_execution_context(&headers).unwrap().is_some());
        headers.append(EXECUTION_CONTEXT_HEADER, encoded.parse().unwrap());
        assert_eq!(
            parse_execution_context(&headers),
            Err("invalid_execution_context")
        );
        let mut malformed = HeaderMap::new();
        malformed.insert(EXECUTION_CONTEXT_HEADER, "bm90LWpzb24".parse().unwrap());
        assert_eq!(
            parse_execution_context(&malformed),
            Err("invalid_execution_context")
        );

        let mut context_only = HeaderMap::new();
        context_only.insert(EXECUTION_CONTEXT_HEADER, encoded.parse().unwrap());
        assert!(matches!(
            parse_parent(&context_only),
            Err("invalid_execution_permits")
        ));
        let mut permits_only = HeaderMap::new();
        permits_only.insert("x-acteon-execution-permits", "[]".parse().unwrap());
        assert!(matches!(
            parse_parent(&permits_only),
            Err("execution_context_required")
        ));
        let mut paired = HeaderMap::new();
        paired.insert(EXECUTION_CONTEXT_HEADER, encoded.parse().unwrap());
        paired.insert(
            "x-acteon-execution-permits",
            "[{\"id\":\"p\",\"accepted_revision\":1}]".parse().unwrap(),
        );
        assert!(matches!(parse_parent(&paired), Ok(Some(_))));
    }

    #[test]
    fn peer_submission_actions_require_a_uuid_and_known_suffix() {
        let id = "11111111-1111-4111-8111-111111111111";
        assert_eq!(
            parse_peer_submission_action(&format!("{id}:refresh")),
            Some((
                uuid::Uuid::parse_str(id).unwrap(),
                PeerSubmissionAction::Refresh
            ))
        );
        assert_eq!(
            parse_peer_submission_action(&format!("{id}:cancel")),
            Some((
                uuid::Uuid::parse_str(id).unwrap(),
                PeerSubmissionAction::Cancel
            ))
        );
        assert_eq!(parse_peer_submission_action(id), None);
        assert_eq!(parse_peer_submission_action(&format!("{id}:unknown")), None);
        assert_eq!(parse_peer_submission_action("not-a-uuid:refresh"), None);
    }

    #[test]
    fn cancellation_projection_distinguishes_request_from_confirmed_finality() {
        let mut task = Task::new("job-1", "prod", "acme");
        let pending = project_governed_v1_task_with_cancellation(
            &task,
            true,
            false,
            Some(&acteon_executor::governed::abort::ProviderAbortStatus::RestrictedOnly),
        )
        .unwrap();
        assert_eq!(pending["status"]["state"], "TASK_STATE_SUBMITTED");
        assert_eq!(
            pending["metadata"]["acteonCancellation"],
            serde_json::json!({
                "state":"requested", "futureStartsBlocked":true,
                "providerAbort":{"state":"restricted_only"}
            })
        );

        task.transition_to(TaskState::Canceled, None).unwrap();
        let confirmed =
            project_governed_v1_task_with_cancellation(&task, true, true, None).unwrap();
        assert_eq!(confirmed["status"]["state"], "TASK_STATE_CANCELED");
        assert_eq!(
            confirmed["metadata"]["acteonCancellation"]["state"],
            "confirmed"
        );
        let error = agent_rpc_error(Value::Null, AgentRpcFailure::task_not_cancelable("job-1"));
        assert_eq!(
            error.error.unwrap().data.unwrap()[0]["reason"],
            "TASK_NOT_CANCELABLE"
        );
    }
}
