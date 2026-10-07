//! Explicit individual-agent ingress; tenant-level A2A remains independent.
use super::{AppState, a2a::A2A_PROTOCOL_VERSION, schemas::ErrorResponse};
use crate::{
    auth::{
        identity::CallerIdentity, projection::AuthenticatedExecutionConfiguration, role::Permission,
    },
    execution_authority::{AgentServiceError, AgentServiceObservation, AgentServiceRequest},
};
use acteon_core::TaskMessage;
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

const SOURCE_CONTEXT_HEADER: &str = "x-acteon-agent-source-context";

/// New service tasks use a fixed, operator-qualified runtime. Original context
/// injection for delegated host tools is a separate trusted-host operation.
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentMessageSend {
    pub message: TaskMessage,
}

#[utoipa::path(
    post, path = "/a2a/{namespace}/{tenant}/agents/{agent}/v1/message:send", tag = "Governance",
    params(("namespace" = String, Path), ("tenant" = String, Path), ("agent" = String, Path)),
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
    match runtime
        .accept_agent_service(AgentServiceRequest {
            namespace: &namespace,
            tenant: &tenant,
            agent_id: &agent,
            message: &request.message,
            authentication: &proof,
            auth_provider: authentication,
            parent: None,
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
