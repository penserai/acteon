//! Operator governance with original private authentication at the write boundary.
use super::AppState;
use crate::{
    auth::projection::AuthenticatedExecutionConfiguration, execution_authority::ManagementError,
};
use acteon_core::{
    GovernanceChangeReceipt, GovernanceInterventionRequest, GovernanceScopeView,
    PublishGovernancePermitRequest,
};
use axum::{
    Extension, Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;

#[derive(Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct GovernanceQuery {
    pub namespace: String,
    pub tenant: String,
}

pub struct GovernanceApiError(StatusCode, &'static str);
impl From<ManagementError> for GovernanceApiError {
    fn from(error: ManagementError) -> Self {
        match error {
            ManagementError::Forbidden => {
                Self(StatusCode::FORBIDDEN, "governance_authority_denied")
            }
            ManagementError::Invalid => Self(StatusCode::BAD_REQUEST, "invalid_governance_request"),
            ManagementError::Conflict => Self(StatusCode::CONFLICT, "governance_authority_changed"),
            ManagementError::Unavailable => {
                Self(StatusCode::SERVICE_UNAVAILABLE, "governance_unavailable")
            }
        }
    }
}
impl IntoResponse for GovernanceApiError {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(serde_json::json!({"error": self.1, "code": self.1})),
        )
            .into_response()
    }
}
pub(super) fn authentication(
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
) -> Result<AuthenticatedExecutionConfiguration, GovernanceApiError> {
    proof.map(|p| p.0).ok_or(GovernanceApiError(
        StatusCode::UNAUTHORIZED,
        "private_authentication_required",
    ))
}

pub(super) fn runtime(
    state: &AppState,
) -> Result<&crate::execution_authority::ExecutionAuthorityRuntime, GovernanceApiError> {
    state
        .execution_authority
        .as_deref()
        .ok_or(GovernanceApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "governance_not_configured",
        ))
}

#[utoipa::path(get, path = "/v1/governance", tag = "Governance", params(GovernanceQuery),
    responses((status = 200, body = GovernanceScopeView), (status = 403, description = "Current management authority required"), (status = 409, description = "Authority changed"), (status = 503, description = "Governance unavailable")))]
pub async fn inspect(
    State(state): State<AppState>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Query(query): Query<GovernanceQuery>,
) -> Result<Json<GovernanceScopeView>, GovernanceApiError> {
    let proof = authentication(proof)?;
    let runtime = state
        .execution_authority
        .as_ref()
        .ok_or(GovernanceApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "governance_not_configured",
        ))?;
    Ok(Json(
        runtime
            .inspect_governance(&query.namespace, &query.tenant, &proof)
            .await?,
    ))
}

#[utoipa::path(post, path = "/v1/governance/permits", tag = "Governance", request_body = PublishGovernancePermitRequest,
    responses((status = 200, body = GovernanceChangeReceipt), (status = 400, description = "Invalid request"), (status = 403, description = "Publication exceeds management ceiling"), (status = 409, description = "Revision or authority conflict"), (status = 503, description = "Governance unavailable")))]
pub async fn publish_permit(
    State(state): State<AppState>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Json(request): Json<PublishGovernancePermitRequest>,
) -> Result<Json<GovernanceChangeReceipt>, GovernanceApiError> {
    let proof = authentication(proof)?;
    let runtime = state
        .execution_authority
        .as_ref()
        .ok_or(GovernanceApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "governance_not_configured",
        ))?;
    Ok(Json(
        runtime.publish_governance_permit(request, &proof).await?,
    ))
}

#[utoipa::path(post, path = "/v1/governance/changes", tag = "Governance", request_body = GovernanceInterventionRequest,
    responses((status = 200, body = GovernanceChangeReceipt), (status = 400, description = "Invalid request"), (status = 403, description = "Intervention exceeds management ceiling"), (status = 409, description = "Revision or authority conflict"), (status = 503, description = "Governance unavailable")))]
pub async fn intervene(
    State(state): State<AppState>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Json(request): Json<GovernanceInterventionRequest>,
) -> Result<Json<GovernanceChangeReceipt>, GovernanceApiError> {
    let proof = authentication(proof)?;
    let runtime = state
        .execution_authority
        .as_ref()
        .ok_or(GovernanceApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "governance_not_configured",
        ))?;
    Ok(Json(runtime.intervene_governance(request, &proof).await?))
}
