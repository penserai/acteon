//! Workforce management requires original private authentication and independent bounds.
use super::{
    AppState,
    governance::{GovernanceApiError, GovernanceQuery},
};
use crate::auth::projection::AuthenticatedExecutionConfiguration;
use acteon_core::{
    GovernanceChangeReceipt,
    workforce::{WorkforceChangeRequest, WorkforceScopeView},
};
use axum::{
    Extension, Json,
    extract::{Query, State},
};

#[utoipa::path(get, path = "/v1/workforce", tag = "Governance", params(GovernanceQuery),
    responses((status = 200, body = WorkforceScopeView), (status = 403, description = "Current workforce management authority required"),
    (status = 409, description = "Authority changed"), (status = 503, description = "Workforce unavailable")))]
pub async fn inspect(
    State(state): State<AppState>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Query(query): Query<GovernanceQuery>,
) -> Result<Json<WorkforceScopeView>, GovernanceApiError> {
    let proof = super::governance::authentication(proof)?;
    let runtime = super::governance::runtime(&state)?;
    Ok(Json(
        runtime
            .inspect_workforce(&query.namespace, &query.tenant, &proof)
            .await?,
    ))
}

#[utoipa::path(post, path = "/v1/workforce/changes", tag = "Governance", request_body = WorkforceChangeRequest,
    responses((status = 200, body = GovernanceChangeReceipt), (status = 400, description = "Invalid request"),
    (status = 403, description = "Change exceeds workforce management ceiling"), (status = 409, description = "Revision or authority conflict"),
    (status = 503, description = "Workforce unavailable")))]
pub async fn change(
    State(state): State<AppState>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Json(request): Json<WorkforceChangeRequest>,
) -> Result<Json<GovernanceChangeReceipt>, GovernanceApiError> {
    let proof = super::governance::authentication(proof)?;
    let runtime = super::governance::runtime(&state)?;
    Ok(Json(runtime.change_workforce(request, &proof).await?))
}
