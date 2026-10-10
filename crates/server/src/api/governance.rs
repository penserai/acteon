//! Operator governance with original private authentication at the write boundary.
use super::AppState;
use crate::{
    auth::projection::AuthenticatedExecutionConfiguration, execution_authority::ManagementError,
};
use acteon_core::{
    GovernanceChangeReceipt, GovernanceInterventionRequest, GovernanceScopeView,
    ProviderExecutionHistory, PublishFederationTrustRequest, PublishGovernancePermitRequest,
    RevokeFederationTrustRequest,
};
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct GovernanceQuery {
    pub namespace: String,
    pub tenant: String,
}

#[derive(Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct FederationDeliveryEvidenceQuery {
    pub namespace: String,
    pub tenant: String,
    pub after: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FederationDeliveryEvidencePage {
    pub deliveries: Vec<FederationDeliveryEvidence>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FederationDeliveryEvidence {
    pub delivery_id: String,
    pub parent_execution_id: String,
    pub source: acteon_core::PrincipalIdentity,
    pub agent_id: String,
    pub message_id: String,
    pub trust_id: String,
    pub trust_revision: u64,
    pub key_id: String,
    pub issuer_epoch: u64,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    pub expired: bool,
    pub exact_replay_idempotent: bool,
    pub status: FederationDeliveryEvidenceStatus,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FederationDeliveryEvidenceStatus {
    Registered,
    Importing { claim_expires_at_ms: i64 },
    ImportUncertain,
    Imported,
    Invoking { claim_expires_at_ms: i64 },
    InvokeUncertain,
    Accepted,
    Rejected { stage: String, code: String },
}

impl From<acteon_executor::federation_transport::FederationDeliveryEvidence>
    for FederationDeliveryEvidence
{
    fn from(evidence: acteon_executor::federation_transport::FederationDeliveryEvidence) -> Self {
        use acteon_executor::federation_transport::{
            FederationDeliveryEvidenceStatus as Status, FederationDeliveryStage,
        };
        let status = match evidence.status {
            Status::Registered => FederationDeliveryEvidenceStatus::Registered,
            Status::Importing {
                claim_expires_at_ms,
            } => FederationDeliveryEvidenceStatus::Importing {
                claim_expires_at_ms,
            },
            Status::ImportUncertain => FederationDeliveryEvidenceStatus::ImportUncertain,
            Status::Imported => FederationDeliveryEvidenceStatus::Imported,
            Status::Invoking {
                claim_expires_at_ms,
            } => FederationDeliveryEvidenceStatus::Invoking {
                claim_expires_at_ms,
            },
            Status::InvokeUncertain => FederationDeliveryEvidenceStatus::InvokeUncertain,
            Status::Accepted => FederationDeliveryEvidenceStatus::Accepted,
            Status::Rejected { stage, code } => FederationDeliveryEvidenceStatus::Rejected {
                stage: match stage {
                    FederationDeliveryStage::Import => "import",
                    FederationDeliveryStage::Invoke => "invoke",
                }
                .into(),
                code,
            },
        };
        Self {
            delivery_id: evidence.delivery_id.to_string(),
            parent_execution_id: evidence.parent_execution_id.to_string(),
            source: evidence.source,
            agent_id: evidence.agent_id,
            message_id: evidence.message_id,
            trust_id: evidence.trust_id,
            trust_revision: evidence.trust_revision,
            key_id: evidence.key_id,
            issuer_epoch: evidence.issuer_epoch,
            created_at_ms: evidence.created_at_ms,
            expires_at_ms: evidence.expires_at_ms,
            expired: evidence.expired,
            exact_replay_idempotent: evidence.exact_replay_idempotent,
            status,
        }
    }
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

#[utoipa::path(post, path = "/v1/governance/federation/trusts", tag = "Governance", request_body = PublishFederationTrustRequest,
    responses((status = 200, body = GovernanceChangeReceipt), (status = 400, description = "Invalid trust"), (status = 403, description = "Trust exceeds configured management ceiling"), (status = 409, description = "Revision or authority conflict"), (status = 503, description = "Governance unavailable")))]
pub async fn publish_federation_trust(
    State(state): State<AppState>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Json(request): Json<PublishFederationTrustRequest>,
) -> Result<Json<GovernanceChangeReceipt>, GovernanceApiError> {
    let proof = authentication(proof)?;
    Ok(Json(
        runtime(&state)?
            .publish_federation_trust(request, &proof)
            .await?,
    ))
}

#[utoipa::path(post, path = "/v1/governance/federation/trusts/revoke", tag = "Governance", request_body = RevokeFederationTrustRequest,
    responses((status = 200, body = GovernanceChangeReceipt), (status = 400, description = "Invalid revocation"), (status = 403, description = "Trust exceeds configured management ceiling"), (status = 409, description = "Revision or authority conflict"), (status = 503, description = "Governance unavailable")))]
pub async fn revoke_federation_trust(
    State(state): State<AppState>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Json(request): Json<RevokeFederationTrustRequest>,
) -> Result<Json<GovernanceChangeReceipt>, GovernanceApiError> {
    let proof = authentication(proof)?;
    Ok(Json(
        runtime(&state)?
            .revoke_federation_trust(request, &proof)
            .await?,
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

/// List sanitized outbound federation evidence without exposing messages,
/// signed payloads, endpoints, credentials or signing material.
#[utoipa::path(get, path = "/v1/governance/federation/deliveries", tag = "Governance",
    params(FederationDeliveryEvidenceQuery),
    responses((status = 200, body = FederationDeliveryEvidencePage),
        (status = 400, description = "Invalid cursor or page size"),
        (status = 401, description = "Private authentication required"),
        (status = 403, description = "Independent reconciliation permission required"),
        (status = 409, description = "Authority or retained evidence conflict"),
        (status = 503, description = "Federation evidence unavailable")))]
pub async fn federation_delivery_evidence(
    State(state): State<AppState>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Query(query): Query<FederationDeliveryEvidenceQuery>,
) -> Result<impl IntoResponse, GovernanceApiError> {
    let proof = authentication(proof)?;
    let after = query
        .after
        .as_deref()
        .map(uuid::Uuid::parse_str)
        .transpose()
        .map_err(|_| ManagementError::Invalid)?;
    let page = runtime(&state)?
        .list_federation_delivery_evidence(
            &query.namespace,
            &query.tenant,
            after,
            query.limit.unwrap_or(50),
            &proof,
        )
        .await?;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(FederationDeliveryEvidencePage {
            deliveries: page.deliveries.into_iter().map(Into::into).collect(),
            next_cursor: page.next_cursor.map(|cursor| cursor.to_string()),
        }),
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

#[derive(Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct RegistryProjectionQuery {
    pub namespace: String,
    pub tenant: String,
    pub projection: acteon_core::GovernanceRegistryProjection,
}

#[utoipa::path(get, path = "/v1/governance/registry/{agent_id}", tag = "Governance", params(("agent_id" = String, Path), RegistryProjectionQuery),
    responses((status = 200, body = acteon_core::GovernanceRegistryProjectionView), (status = 403, description = "Exact current agent management authority required"), (status = 503, description = "Registry authority unavailable")))]
pub async fn registry_projection(
    State(state): State<AppState>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path(agent_id): Path<String>,
    Query(query): Query<RegistryProjectionQuery>,
) -> Result<Response, GovernanceApiError> {
    let proof = authentication(proof)?;
    let view = runtime(&state)?
        .inspect_registry_projection(
            &query.namespace,
            &query.tenant,
            &agent_id,
            query.projection,
            &proof,
        )
        .await?;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(view),
    )
        .into_response())
}

#[utoipa::path(post, path = "/v1/governance/registry", tag = "Governance", request_body = acteon_core::GovernanceRegistryMutationRequest,
    responses((status = 200, body = acteon_core::GovernanceRegistryMutationReceipt), (status = 400, description = "Invalid metadata or intent"), (status = 403, description = "Exact current agent management authority required"), (status = 409, description = "Changed metadata or qualification"), (status = 503, description = "Unavailable or unresolved control delivery")))]
pub async fn mutate_registry(
    State(state): State<AppState>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Json(request): Json<acteon_core::GovernanceRegistryMutationRequest>,
) -> Result<Response, GovernanceApiError> {
    let proof = authentication(proof)?;
    let receipt = runtime(&state)?
        .mutate_registry_projection(request, &proof)
        .await?;
    state
        .a2a_discovery_cache
        .invalidate(&receipt.namespace, &receipt.tenant)
        .await;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(receipt),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_governance::CoordinationError;

    #[tokio::test]
    async fn unresolved_registry_delivery_is_unavailable_and_not_authority_denied() {
        for (error, expected_status, expected_code) in [
            (
                CoordinationError::RegistryMutationUnresolved,
                StatusCode::SERVICE_UNAVAILABLE,
                "governance_unavailable",
            ),
            (
                CoordinationError::Restricted,
                StatusCode::FORBIDDEN,
                "governance_authority_denied",
            ),
            (
                CoordinationError::Conflict,
                StatusCode::CONFLICT,
                "governance_authority_changed",
            ),
        ] {
            let response = GovernanceApiError::from(ManagementError::from(error)).into_response();
            assert_eq!(response.status(), expected_status);
            assert_eq!(
                response.headers()[axum::http::header::CACHE_CONTROL],
                "no-store"
            );
            let body = axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(json["code"], expected_code);
            assert_eq!(json["error"], expected_code);
        }
    }
}
