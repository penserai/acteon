//! Explicit individual-agent ingress; tenant-level A2A remains independent.
use super::{AppState, a2a::A2A_PROTOCOL_VERSION, schemas::ErrorResponse};
use crate::{
    auth::{
        identity::CallerIdentity, projection::AuthenticatedExecutionConfiguration, role::Permission,
    },
    execution_authority::{AgentServiceObservation, AgentServiceRequest},
};
use acteon_core::TaskMessage;
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;

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
        (status = 400, description = "Unsupported A2A version"), (status = 403, description = "Original service authority required"),
        (status = 409, description = "Admission refused"), (status = 503, description = "Service runtime unavailable"))
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
                    (SOURCE_CONTEXT_HEADER, URL_SAFE_NO_PAD.encode(source)),
                ],
                Json(accepted.task),
            )
                .into_response()
        }
        // A refused admission may be an authority conflict or unavailable state.
        // Do not expose private credential or configured recipient details.
        Err(_) => error(StatusCode::CONFLICT, "agent_service_admission_refused"),
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
    let source = match headers.get(SOURCE_CONTEXT_HEADER) {
        None => None,
        Some(value) => {
            let parsed = value
                .to_str()
                .ok()
                .filter(|v| v.len() <= 8192)
                .and_then(|v| URL_SAFE_NO_PAD.decode(v).ok())
                .and_then(|raw| {
                    serde_json::from_slice::<acteon_core::ExecutionContextReference>(&raw).ok()
                });
            let Some(reference) = parsed else {
                return error(StatusCode::BAD_REQUEST, "invalid_source_context");
            };
            Some(reference)
        }
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
            [("a2a-version", A2A_PROTOCOL_VERSION)],
            Json(task),
        )
            .into_response(),
        Err(_) => error(StatusCode::NOT_FOUND, "service_task_unavailable"),
    }
}

fn error(status: StatusCode, code: &str) -> Response {
    (
        status,
        [("a2a-version", A2A_PROTOCOL_VERSION)],
        Json(ErrorResponse { error: code.into() }),
    )
        .into_response()
}
