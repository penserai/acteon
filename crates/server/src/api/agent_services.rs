//! Explicit individual-agent ingress; tenant-level A2A remains independent.
use super::{AppState, a2a::A2A_PROTOCOL_VERSION, schemas::ErrorResponse};
use crate::{
    auth::{
        identity::CallerIdentity, projection::AuthenticatedExecutionConfiguration, role::Permission,
    },
    execution_authority::AgentServiceRequest,
};
use acteon_core::TaskMessage;
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;

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
        Ok(task) => (
            StatusCode::OK,
            [("a2a-version", A2A_PROTOCOL_VERSION)],
            Json(task),
        )
            .into_response(),
        // A refused admission may be an authority conflict or unavailable state.
        // Do not expose private credential or configured recipient details.
        Err(_) => error(StatusCode::CONFLICT, "agent_service_admission_refused"),
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
