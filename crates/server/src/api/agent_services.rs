//! Explicit individual-agent ingress; tenant-level A2A remains independent.
use super::{AppState, a2a::A2A_PROTOCOL_VERSION, schemas::ErrorResponse};
use crate::execution_authority::{AgentServiceContinuation, AgentServiceParent};
use crate::{
    auth::{
        identity::CallerIdentity, projection::AuthenticatedExecutionConfiguration, role::Permission,
    },
    execution_authority::{AgentServiceError, AgentServiceObservation, AgentServiceRequest},
};
use acteon_core::TaskMessage;
use acteon_executor::delegation::{PeerCancelStatus, PeerContinuationStatus, PeerSendStatus};
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
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
pub struct AgentPeerContinuationReceipt {
    pub submission_id: String,
    pub continuation_id: String,
    pub status: AgentPeerContinuationStatus,
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
}
