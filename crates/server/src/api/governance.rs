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

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AbandonFederationDeliveryRequest {
    pub namespace: String,
    pub tenant: String,
    pub disposition_id: String,
    pub reason: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReconcileFederationDeliveryRequest {
    pub namespace: String,
    pub tenant: String,
    pub proof_base64: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FederationFinalityCorrelation {
    pub delivery_id: String,
    pub stage: String,
    pub parent_execution_id: String,
    pub source: acteon_core::PrincipalIdentity,
    pub adapter_revision: String,
    pub binding_digest: String,
    pub envelope_digest: String,
    pub invocation_digest: String,
    pub import_id: String,
    pub invocation_nonce: String,
    pub agent_id: String,
    pub message_id: String,
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
    pub capability: Option<FederationCapabilityEvidence>,
    pub status: FederationDeliveryEvidenceStatus,
    pub reconciliations: Vec<FederationDeliveryFinalityEvidence>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FederationCapabilityEvidence {
    pub card_digest: String,
    pub key_id: String,
    pub issuer_epoch: u64,
    pub expires_at_ms: i64,
    pub protocol_revision: String,
    pub authentication_scheme: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FederationDeliveryFinalityEvidence {
    pub stage: String,
    pub accepted: bool,
    pub source_id: String,
    pub qualification_ref: String,
    pub verifier_revision: String,
    pub proof_digest: String,
    pub operator: acteon_core::PrincipalIdentity,
    pub authority_incarnation: String,
    pub authority_generation: u64,
    pub accepted_at_ms: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FederationDeliveryEvidenceStatus {
    Registered,
    CapabilityNegotiating {
        claim_expires_at_ms: i64,
    },
    Importing {
        claim_expires_at_ms: i64,
    },
    ImportUncertain,
    Imported,
    Invoking {
        claim_expires_at_ms: i64,
    },
    InvokeUncertain,
    Accepted,
    Rejected {
        stage: String,
        code: String,
    },
    Abandoned {
        stage: String,
        abandonment: FederationDeliveryAbandonment,
    },
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FederationDeliveryAbandonment {
    pub disposition_id: String,
    pub operator: acteon_core::PrincipalIdentity,
    pub authority_incarnation: String,
    pub authority_generation: u64,
    pub reason: String,
    pub recorded_at_ms: i64,
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
            Status::CapabilityNegotiating {
                claim_expires_at_ms,
            } => FederationDeliveryEvidenceStatus::CapabilityNegotiating {
                claim_expires_at_ms,
            },
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
            Status::Abandoned { stage, abandonment } => {
                FederationDeliveryEvidenceStatus::Abandoned {
                    stage: match stage {
                        FederationDeliveryStage::Import => "import",
                        FederationDeliveryStage::Invoke => "invoke",
                    }
                    .into(),
                    abandonment: FederationDeliveryAbandonment {
                        disposition_id: abandonment.disposition_id.to_string(),
                        operator: abandonment.operator,
                        authority_incarnation: abandonment.authority.incarnation,
                        authority_generation: abandonment.authority.generation,
                        reason: abandonment.reason,
                        recorded_at_ms: abandonment.recorded_at_ms,
                    },
                }
            }
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
            capability: evidence
                .capability
                .map(|capability| FederationCapabilityEvidence {
                    card_digest: capability.card_digest,
                    key_id: capability.key_id,
                    issuer_epoch: capability.issuer_epoch,
                    expires_at_ms: capability.expires_at_ms,
                    protocol_revision: capability.protocol_revision,
                    authentication_scheme: capability.authentication_scheme,
                }),
            status,
            reconciliations: evidence
                .reconciliations
                .into_iter()
                .map(|entry| FederationDeliveryFinalityEvidence {
                    stage: delivery_stage(entry.stage).into(),
                    accepted: entry.accepted,
                    source_id: entry.source_id,
                    qualification_ref: entry.qualification_ref,
                    verifier_revision: entry.verifier_revision,
                    proof_digest: entry.proof_digest,
                    operator: entry.acceptance.operator,
                    authority_incarnation: entry.acceptance.authority.incarnation,
                    authority_generation: entry.acceptance.authority.generation,
                    accepted_at_ms: entry.acceptance.accepted_at_ms,
                })
                .collect(),
        }
    }
}

fn delivery_stage(
    stage: acteon_executor::federation_transport::FederationDeliveryStage,
) -> &'static str {
    match stage {
        acteon_executor::federation_transport::FederationDeliveryStage::Import => "import",
        acteon_executor::federation_transport::FederationDeliveryStage::Invoke => "invoke",
    }
}

impl From<acteon_executor::federation_finality::FederationFinalityCorrelation>
    for FederationFinalityCorrelation
{
    fn from(
        correlation: acteon_executor::federation_finality::FederationFinalityCorrelation,
    ) -> Self {
        Self {
            delivery_id: correlation.delivery_id.to_string(),
            stage: delivery_stage(correlation.stage).into(),
            parent_execution_id: correlation.parent_execution_id.to_string(),
            source: correlation.source,
            adapter_revision: correlation.adapter_revision,
            binding_digest: correlation.binding_digest,
            envelope_digest: correlation.envelope_digest,
            invocation_digest: correlation.invocation_digest,
            import_id: correlation.import_id,
            invocation_nonce: correlation.invocation_nonce,
            agent_id: correlation.agent_id,
            message_id: correlation.message_id,
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

/// Stop local recovery of an ambiguous outbound delivery without claiming a
/// remote outcome or invoking the peer.
#[utoipa::path(post, path = "/v1/governance/federation/deliveries/{delivery_id}:abandon", tag = "Governance",
    params(("delivery_id" = String, Path)),
    request_body = AbandonFederationDeliveryRequest,
    responses((status = 200, body = FederationDeliveryEvidence),
        (status = 400, description = "Invalid disposition request"),
        (status = 401, description = "Private authentication required"),
        (status = 403, description = "Independent reconciliation permission required"),
        (status = 404, description = "No accessible retained delivery"),
        (status = 409, description = "Delivery is not ambiguous or disposition conflicts"),
        (status = 503, description = "Federation reconciliation unavailable")))]
pub async fn abandon_federation_delivery(
    State(state): State<AppState>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path(delivery_id): Path<uuid::Uuid>,
    Json(request): Json<AbandonFederationDeliveryRequest>,
) -> Result<impl IntoResponse, GovernanceApiError> {
    let proof = authentication(proof)?;
    let disposition_id =
        uuid::Uuid::parse_str(&request.disposition_id).map_err(|_| ManagementError::Invalid)?;
    let evidence = runtime(&state)?
        .abandon_federation_delivery(
            &request.namespace,
            &request.tenant,
            delivery_id,
            disposition_id,
            &request.reason,
            &proof,
        )
        .await?;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(FederationDeliveryEvidence::from(evidence)),
    ))
}

/// Return immutable correlation for an independently qualified finality source.
#[utoipa::path(get, path = "/v1/governance/federation/deliveries/{delivery_id}/correlation", tag = "Governance",
    params(("delivery_id" = String, Path), GovernanceQuery),
    responses((status = 200, body = FederationFinalityCorrelation),
        (status = 401, description = "Private authentication required"),
        (status = 403, description = "Independent reconciliation permission required"),
        (status = 404, description = "No accessible retained delivery"),
        (status = 409, description = "Delivery has no ambiguous stage"),
        (status = 503, description = "Qualified federation finality unavailable")))]
pub async fn federation_finality_correlation(
    State(state): State<AppState>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path(delivery_id): Path<uuid::Uuid>,
    Query(query): Query<GovernanceQuery>,
) -> Result<impl IntoResponse, GovernanceApiError> {
    let proof = authentication(proof)?;
    let correlation = runtime(&state)?
        .federation_finality_correlation(&query.namespace, &query.tenant, delivery_id, &proof)
        .await?;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(FederationFinalityCorrelation::from(correlation)),
    ))
}

/// Accept independently verified peer finality without calling or replaying the peer.
#[utoipa::path(post, path = "/v1/governance/federation/deliveries/{delivery_id}:reconcile", tag = "Governance",
    params(("delivery_id" = String, Path)),
    request_body = ReconcileFederationDeliveryRequest,
    responses((status = 200, body = FederationDeliveryEvidence),
        (status = 400, description = "Invalid finality evidence"),
        (status = 401, description = "Private authentication required"),
        (status = 403, description = "Independent reconciliation permission required or proof untrusted"),
        (status = 404, description = "No accessible retained delivery"),
        (status = 409, description = "Correlation, stage, authority, or evidence conflict"),
        (status = 503, description = "Qualified federation finality unavailable")))]
pub async fn reconcile_federation_delivery(
    State(state): State<AppState>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path(delivery_id): Path<uuid::Uuid>,
    Json(request): Json<ReconcileFederationDeliveryRequest>,
) -> Result<impl IntoResponse, GovernanceApiError> {
    use base64::Engine;
    let authentication = authentication(proof)?;
    if request.proof_base64.is_empty() || request.proof_base64.len() > 87_384 {
        return Err(ManagementError::Invalid.into());
    }
    let proof = base64::engine::general_purpose::STANDARD
        .decode(request.proof_base64)
        .map_err(|_| ManagementError::Invalid)?;
    if proof.is_empty() || proof.len() > 65_536 {
        return Err(ManagementError::Invalid.into());
    }
    let evidence = runtime(&state)?
        .reconcile_federation_delivery(
            &request.namespace,
            &request.tenant,
            delivery_id,
            &proof,
            &authentication,
        )
        .await?;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(FederationDeliveryEvidence::from(evidence)),
    ))
}

/// Axum-compatible dispatcher for the public `{delivery_id}:abandon` and
/// `{delivery_id}:reconcile` protocol operations. The action suffix is parsed
/// before the UUID, so an unknown verb cannot alias a delivery identity.
pub async fn federation_delivery_action(
    State(state): State<AppState>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path(delivery_action): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<impl IntoResponse, GovernanceApiError> {
    let proof = authentication(proof)?;
    let (delivery_id, action) = delivery_action
        .rsplit_once(':')
        .ok_or(ManagementError::Invalid)?;
    let delivery_id = uuid::Uuid::parse_str(delivery_id).map_err(|_| ManagementError::Invalid)?;
    let evidence = match action {
        "abandon" => {
            let request: AbandonFederationDeliveryRequest =
                serde_json::from_value(body).map_err(|_| ManagementError::Invalid)?;
            let disposition_id = uuid::Uuid::parse_str(&request.disposition_id)
                .map_err(|_| ManagementError::Invalid)?;
            runtime(&state)?
                .abandon_federation_delivery(
                    &request.namespace,
                    &request.tenant,
                    delivery_id,
                    disposition_id,
                    &request.reason,
                    &proof,
                )
                .await?
        }
        "reconcile" => {
            use base64::Engine;
            let request: ReconcileFederationDeliveryRequest =
                serde_json::from_value(body).map_err(|_| ManagementError::Invalid)?;
            if request.proof_base64.is_empty() || request.proof_base64.len() > 87_384 {
                return Err(ManagementError::Invalid.into());
            }
            let evidence = base64::engine::general_purpose::STANDARD
                .decode(request.proof_base64)
                .map_err(|_| ManagementError::Invalid)?;
            if evidence.is_empty() || evidence.len() > 65_536 {
                return Err(ManagementError::Invalid.into());
            }
            runtime(&state)?
                .reconcile_federation_delivery(
                    &request.namespace,
                    &request.tenant,
                    delivery_id,
                    &evidence,
                    &proof,
                )
                .await?
        }
        _ => return Err(ManagementError::Invalid.into()),
    };
    Ok((
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(FederationDeliveryEvidence::from(evidence)),
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
