//! Explicit individual-agent ingress; tenant-level A2A remains independent.
use super::{AppState, a2a::A2A_PROTOCOL_VERSION, schemas::ErrorResponse};
use crate::execution_authority::AgentServiceParent;
use crate::{
    auth::{
        identity::CallerIdentity, projection::AuthenticatedExecutionConfiguration, role::Permission,
    },
    execution_authority::{AgentServiceError, AgentServiceObservation, AgentServiceRequest},
};
use acteon_core::TaskMessage;
use acteon_executor::delegation::PeerSendStatus;
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

const SOURCE_CONTEXT_HEADER: &str = "x-acteon-agent-source-context";
const EXECUTION_CONTEXT_HEADER: &str = "x-acteon-execution-context";

/// New service tasks use a fixed, operator-qualified runtime. Original context
/// injection for delegated host tools is a separate trusted-host operation.
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentMessageSend {
    pub message: TaskMessage,
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
    match runtime
        .accept_agent_service(AgentServiceRequest {
            namespace: &namespace,
            tenant: &tenant,
            agent_id: &agent,
            message: &request.message,
            authentication: &proof,
            auth_provider: authentication,
            parent,
        })
        .await
    {
        Ok(accepted) => {
            let Ok(source) = serde_json::to_vec(&accepted.source_context) else {
                return error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "source_context_unavailable",
                );
            };
            (
                StatusCode::OK,
                [
                    ("a2a-version", A2A_PROTOCOL_VERSION.to_string()),
                    ("cache-control", "no-store".to_string()),
                    (SOURCE_CONTEXT_HEADER, URL_SAFE_NO_PAD.encode(source)),
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
        Ok(task) => (
            StatusCode::OK,
            [
                ("a2a-version", A2A_PROTOCOL_VERSION),
                ("cache-control", "no-store"),
            ],
            Json(task),
        )
            .into_response(),
        Err(cause) => service_error(cause),
    }
}

/// Durable restriction acknowledgement. Task state remains provider evidence;
/// stopping future starts does not certify cancellation of an existing effect.
#[derive(Serialize, utoipa::ToSchema)]
pub struct AgentServiceStopResponse {
    pub task: acteon_core::Task,
    pub future_starts_blocked: bool,
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
    match runtime
        .stop_agent_service(AgentServiceObservation {
            namespace: &namespace,
            tenant: &tenant,
            agent_id: &agent,
            task_id,
            authentication: &proof,
            source_context: source.as_ref(),
        })
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
}
