//! Authenticated cross-domain envelope admission.

use super::{AppState, governance::GovernanceApiError};
use crate::{
    auth::projection::AuthenticatedExecutionConfiguration,
    execution_authority::{FederationAgentInvocation, FederationImportAdmission},
};
use acteon_core::{
    FederationImportReceipt, ImportFederationEnvelopeRequest, InvokeFederatedAgentRequest,
};
use acteon_governance::federation::{SignedFederationEnvelope, SignedFederationInvocation};
use axum::{
    Extension, Json,
    extract::{Path, State},
};

#[utoipa::path(
    post,
    path = "/a2a/{namespace}/{tenant}/federation/envelopes:import",
    tag = "Governance",
    params(("namespace" = String, Path), ("tenant" = String, Path)),
    request_body = ImportFederationEnvelopeRequest,
    responses(
        (status = 200, body = FederationImportReceipt, description = "Signed authority retained; no execution started"),
        (status = 400, description = "Invalid signed envelope"),
        (status = 401, description = "Private authentication required"),
        (status = 403, description = "Peer, trust, audience, or service binding denied"),
        (status = 409, description = "Envelope ID or authority conflict"),
        (status = 503, description = "Federation authority unavailable")
    )
)]
pub async fn import_envelope(
    State(state): State<AppState>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path((namespace, tenant)): Path<(String, String)>,
    Json(request): Json<ImportFederationEnvelopeRequest>,
) -> Result<Json<FederationImportReceipt>, GovernanceApiError> {
    let proof = super::governance::authentication(proof)?;
    if request.envelope.payload.is_empty()
        || request.envelope.payload.len() > 65_536
        || request.envelope.signature.len() != 128
        || !request
            .envelope
            .signature
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(crate::execution_authority::ManagementError::Invalid.into());
    }
    let signed = SignedFederationEnvelope {
        payload: request.envelope.payload,
        signature: request.envelope.signature,
    };
    Ok(Json(
        super::governance::runtime(&state)?
            .import_federation_envelope(FederationImportAdmission {
                namespace: &namespace,
                tenant: &tenant,
                signed: &signed,
                authentication: &proof,
            })
            .await?,
    ))
}

#[utoipa::path(
    post,
    path = "/a2a/{namespace}/{tenant}/federation/agents/{agent}/v1/message:send",
    tag = "Governance",
    params(("namespace" = String, Path), ("tenant" = String, Path), ("agent" = String, Path)),
    request_body = InvokeFederatedAgentRequest,
    responses(
        (status = 200, body = acteon_core::Task, description = "Exact signed message executed or replayed through the governed local agent"),
        (status = 400, description = "Invalid invocation or message"),
        (status = 401, description = "Private authentication required"),
        (status = 403, description = "Peer, trust, target, effect, or input denied"),
        (status = 404, description = "Imported service binding unavailable"),
        (status = 409, description = "Invocation, acceptance, or retained evidence conflict"),
        (status = 503, description = "Federated execution unavailable")
    )
)]
pub async fn invoke_agent(
    State(state): State<AppState>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path((namespace, tenant, agent)): Path<(String, String, String)>,
    Json(request): Json<InvokeFederatedAgentRequest>,
) -> Result<Json<acteon_core::Task>, GovernanceApiError> {
    let proof = super::governance::authentication(proof)?;
    if request.invocation.payload.is_empty()
        || request.invocation.payload.len() > 65_536
        || request.invocation.signature.len() != 128
        || !request
            .invocation
            .signature
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(crate::execution_authority::ManagementError::Invalid.into());
    }
    let signed = SignedFederationInvocation {
        payload: request.invocation.payload,
        signature: request.invocation.signature,
    };
    Ok(Json(
        super::governance::runtime(&state)?
            .invoke_federated_agent(FederationAgentInvocation {
                namespace: &namespace,
                tenant: &tenant,
                agent_id: &agent,
                signed: &signed,
                message: &request.message,
                authentication: &proof,
            })
            .await?,
    ))
}
