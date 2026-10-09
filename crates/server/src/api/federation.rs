//! Authenticated cross-domain envelope admission.

use super::{AppState, governance::GovernanceApiError};
use crate::{
    auth::projection::AuthenticatedExecutionConfiguration,
    execution_authority::FederationImportAdmission,
};
use acteon_core::{FederationImportReceipt, ImportFederationEnvelopeRequest};
use acteon_governance::federation::SignedFederationEnvelope;
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
