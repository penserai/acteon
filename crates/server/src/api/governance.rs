//! Operator governance with original private authentication at the write boundary.
use super::AppState;
use crate::{
    auth::projection::AuthenticatedExecutionConfiguration, execution_authority::ManagementError,
};
use acteon_core::{
    GovernanceChangeReceipt, GovernanceInterventionRequest, GovernanceScopeView,
    ProviderExecutionHistory, PublishGovernancePermitRequest,
};
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
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
            ManagementError::NotFound => Self(StatusCode::NOT_FOUND, "provider_history_not_found"),
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
            [(axum::http::header::CACHE_CONTROL, "no-store")],
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

/// Read historical provider receipts without invoking or settling any provider.
#[utoipa::path(get, path = "/v1/governance/executions/{execution_id}", tag = "Governance",
    params(("execution_id" = String, Path, description = "Provider execution UUID"), GovernanceQuery),
    responses((status = 200, body = ProviderExecutionHistory),
        (status = 401, description = "Private authentication required"),
        (status = 403, description = "Current history permission required"),
        (status = 404, description = "No accessible provider history"),
        (status = 409, description = "Authority or evidence conflict"),
        (status = 503, description = "Evidence unavailable")))]
pub async fn provider_history(
    State(state): State<AppState>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path(execution_id): Path<uuid::Uuid>,
    Query(query): Query<GovernanceQuery>,
) -> Result<Json<ProviderExecutionHistory>, GovernanceApiError> {
    let proof = authentication(proof)?;
    Ok(Json(
        runtime(&state)?
            .inspect_provider_history(&query.namespace, &query.tenant, execution_id, &proof)
            .await?,
    ))
}

/// Privileged correlation for a qualified external finality source.
#[utoipa::path(get, path = "/v1/governance/executions/{execution_id}/attempts/{ordinal}/correlation", tag = "Governance",
    params(("execution_id" = String, Path), ("ordinal" = u32, Path), GovernanceQuery),
    responses((status = 200, body = acteon_core::ProviderReconciliationCorrelation),
        (status = 401, description = "Private authentication required"),
        (status = 403, description = "Independent reconciliation permission required"),
        (status = 404, description = "No accessible retained execution"),
        (status = 409, description = "Authority or evidence conflict"),
        (status = 503, description = "Qualified reconciliation unavailable")))]
pub async fn reconciliation_correlation(
    State(state): State<AppState>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path((execution_id, ordinal)): Path<(uuid::Uuid, u32)>,
    Query(query): Query<GovernanceQuery>,
) -> Result<impl IntoResponse, GovernanceApiError> {
    let proof = authentication(proof)?;
    let runtime = runtime(&state)?;
    let context = runtime
        .provider_reconciliation_context(&query.namespace, &query.tenant, execution_id, &proof)
        .await?;
    let correlation = runtime
        .provider_reconciliation_attempt(&context, ordinal, &proof)
        .await?;
    let body: acteon_core::ProviderReconciliationCorrelation = serde_json::from_value(
        serde_json::to_value(correlation).map_err(|_| ManagementError::Unavailable)?,
    )
    .map_err(|_| ManagementError::Unavailable)?;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(body),
    ))
}

/// Accept qualified finality without invoking or retrying a provider.
#[utoipa::path(post, path = "/v1/governance/executions/{execution_id}/attempts/{ordinal}/reconciliation", tag = "Governance",
    params(("execution_id" = String, Path), ("ordinal" = u32, Path), GovernanceQuery),
    request_body = acteon_core::ProviderReconciliationRequest,
    responses((status = 200, body = acteon_core::ProviderHistoryReceipt),
        (status = 400, description = "Invalid finality evidence"),
        (status = 401, description = "Private authentication required"),
        (status = 403, description = "Independent reconciliation permission required"),
        (status = 404, description = "No accessible retained execution"),
        (status = 409, description = "Authority or evidence conflict"),
        (status = 503, description = "Qualified reconciliation unavailable")))]
pub async fn accept_reconciliation(
    State(state): State<AppState>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path((execution_id, ordinal)): Path<(uuid::Uuid, u32)>,
    Query(query): Query<GovernanceQuery>,
    Json(request): Json<acteon_core::ProviderReconciliationRequest>,
) -> Result<impl IntoResponse, GovernanceApiError> {
    use base64::Engine;
    let proof = authentication(proof)?;
    let runtime = runtime(&state)?;
    let context = runtime
        .provider_reconciliation_context(&query.namespace, &query.tenant, execution_id, &proof)
        .await?;
    if request.proof_base64.is_empty() || request.proof_base64.len() > 87_384 {
        return Err(ManagementError::Invalid.into());
    }
    let evidence = base64::engine::general_purpose::STANDARD
        .decode(&request.proof_base64)
        .map_err(|_| ManagementError::Invalid)?;
    if evidence.is_empty() || evidence.len() > 65_536 {
        return Err(ManagementError::Invalid.into());
    }
    let receipt = runtime
        .reconcile_provider_attempt(&context, ordinal, &evidence, &proof)
        .await?;
    let body: acteon_core::ProviderHistoryReceipt = serde_json::from_value(
        serde_json::to_value(receipt).map_err(|_| ManagementError::Unavailable)?,
    )
    .map_err(|_| ManagementError::Unavailable)?;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(body),
    ))
}
