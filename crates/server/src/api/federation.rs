//! Authenticated cross-domain envelope admission.

use super::{
    AppState, a2a::A2A_PROTOCOL_VERSION, governance::GovernanceApiError, schemas::ErrorResponse,
};
use crate::{
    auth::projection::AuthenticatedExecutionConfiguration,
    auth::{identity::CallerIdentity, role::Permission},
    execution_authority::{
        AgentFederationExportRequest, FederationAgentInvocation, FederationExportRuntimeError,
        FederationImportAdmission,
    },
};
use acteon_core::{
    FederationImportReceipt, ImportFederationEnvelopeRequest, InvokeFederatedAgentRequest,
};
use acteon_governance::federation::{SignedFederationEnvelope, SignedFederationInvocation};
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FederationExportMessage {
    pub message: acteon_core::TaskMessage,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FederationExportApiReceipt {
    pub attempt_id: String,
    pub attempt_status: String,
    pub delivery_id: String,
    pub delivery: FederationExportApiStatus,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FederationExportApiStatus {
    Registered,
    ImportUncertain,
    Imported {
        receipt: Box<FederationImportReceipt>,
    },
    InvokeUncertain {
        receipt: Box<FederationImportReceipt>,
    },
    Accepted {
        receipt: Box<FederationImportReceipt>,
        task: Box<acteon_core::Task>,
    },
    Rejected {
        stage: String,
        code: String,
    },
}

impl From<acteon_executor::federation_export::FederationExportReceipt>
    for FederationExportApiReceipt
{
    fn from(receipt: acteon_executor::federation_export::FederationExportReceipt) -> Self {
        use acteon_executor::federation_transport::{
            FederationDeliveryStage, FederationDeliveryStatus,
        };
        let delivery = match receipt.delivery.status {
            FederationDeliveryStatus::Registered => FederationExportApiStatus::Registered,
            FederationDeliveryStatus::ImportUncertain => FederationExportApiStatus::ImportUncertain,
            FederationDeliveryStatus::Imported { receipt } => {
                FederationExportApiStatus::Imported { receipt }
            }
            FederationDeliveryStatus::InvokeUncertain { receipt } => {
                FederationExportApiStatus::InvokeUncertain { receipt }
            }
            FederationDeliveryStatus::Accepted { receipt, task } => {
                FederationExportApiStatus::Accepted { receipt, task }
            }
            FederationDeliveryStatus::Rejected { stage, code } => {
                FederationExportApiStatus::Rejected {
                    stage: match stage {
                        FederationDeliveryStage::Import => "import",
                        FederationDeliveryStage::Invoke => "invoke",
                    }
                    .into(),
                    code,
                }
            }
        };
        Self {
            attempt_id: receipt.attempt_id,
            attempt_status: match receipt.attempt_status {
                acteon_governance::AttemptStatus::InFlight => "in_flight",
                acteon_governance::AttemptStatus::Settled => "settled",
                acteon_governance::AttemptStatus::Uncertain => "uncertain",
            }
            .into(),
            delivery_id: receipt.delivery.delivery_id.to_string(),
            delivery,
        }
    }
}

#[derive(Clone, Copy)]
enum ExportOperation {
    Submit,
    Observe,
    Replay,
}

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

#[utoipa::path(
    post,
    path = "/a2a/{namespace}/{tenant}/agents/{agent}/v1/tasks/{task_id}/federation/exports/{profile}/message:send",
    tag = "Governance",
    params(("namespace" = String, Path), ("tenant" = String, Path), ("agent" = String, Path),
        ("task_id" = String, Path), ("profile" = String, Path)),
    request_body = FederationExportMessage,
    responses(
        (status = 200, body = FederationExportApiReceipt, description = "Durable cross-domain export receipt; uncertain is not rejection"),
        (status = 400, description = "Invalid message"),
        (status = 403, description = "Current source identity, permit, or export authority required"),
        (status = 404, description = "Source task or export profile unavailable"),
        (status = 409, description = "Request conflicts with retained signed intent"),
        (status = 503, description = "Federation export runtime unavailable")
    )
)]
pub async fn export_message(
    State(state): State<AppState>,
    Extension(identity): Extension<CallerIdentity>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path((namespace, tenant, agent, task_id, profile)): Path<(
        String,
        String,
        String,
        uuid::Uuid,
        String,
    )>,
    headers: HeaderMap,
    Json(request): Json<FederationExportMessage>,
) -> Response {
    export_operation(
        &state,
        &identity,
        proof.as_ref().map(|Extension(proof)| proof),
        &namespace,
        &tenant,
        &agent,
        task_id,
        &profile,
        &headers,
        &request.message,
        ExportOperation::Submit,
    )
    .await
}

#[utoipa::path(
    post,
    path = "/a2a/{namespace}/{tenant}/agents/{agent}/v1/tasks/{task_id}/federation/exports/{profile}/message:observe",
    tag = "Governance",
    params(("namespace" = String, Path), ("tenant" = String, Path), ("agent" = String, Path),
        ("task_id" = String, Path), ("profile" = String, Path)),
    request_body = FederationExportMessage,
    responses(
        (status = 200, body = FederationExportApiReceipt, description = "Observe retained delivery and accounting evidence without network traffic"),
        (status = 400, description = "Invalid message"), (status = 403, description = "Original source identity required"),
        (status = 404, description = "Source task, issuance, or export profile unavailable"),
        (status = 409, description = "Request conflicts with retained signed intent"),
        (status = 503, description = "Federation export runtime unavailable")
    )
)]
pub async fn observe_export_message(
    State(state): State<AppState>,
    Extension(identity): Extension<CallerIdentity>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path((namespace, tenant, agent, task_id, profile)): Path<(
        String,
        String,
        String,
        uuid::Uuid,
        String,
    )>,
    headers: HeaderMap,
    Json(request): Json<FederationExportMessage>,
) -> Response {
    export_operation(
        &state,
        &identity,
        proof.as_ref().map(|Extension(proof)| proof),
        &namespace,
        &tenant,
        &agent,
        task_id,
        &profile,
        &headers,
        &request.message,
        ExportOperation::Observe,
    )
    .await
}

#[utoipa::path(
    post,
    path = "/a2a/{namespace}/{tenant}/agents/{agent}/v1/tasks/{task_id}/federation/exports/{profile}/message:replay",
    tag = "Governance",
    params(("namespace" = String, Path), ("tenant" = String, Path), ("agent" = String, Path),
        ("task_id" = String, Path), ("profile" = String, Path)),
    request_body = FederationExportMessage,
    responses(
        (status = 200, body = FederationExportApiReceipt, description = "Explicit current-authority replay through an exact-idempotent qualified destination"),
        (status = 400, description = "Invalid message"), (status = 403, description = "Current source identity, permit, or replay authority required"),
        (status = 404, description = "Source task, issuance, or export profile unavailable"),
        (status = 409, description = "Request conflicts with retained signed intent"),
        (status = 503, description = "Federation export runtime unavailable")
    )
)]
pub async fn replay_export_message(
    State(state): State<AppState>,
    Extension(identity): Extension<CallerIdentity>,
    proof: Option<Extension<AuthenticatedExecutionConfiguration>>,
    Path((namespace, tenant, agent, task_id, profile)): Path<(
        String,
        String,
        String,
        uuid::Uuid,
        String,
    )>,
    headers: HeaderMap,
    Json(request): Json<FederationExportMessage>,
) -> Response {
    export_operation(
        &state,
        &identity,
        proof.as_ref().map(|Extension(proof)| proof),
        &namespace,
        &tenant,
        &agent,
        task_id,
        &profile,
        &headers,
        &request.message,
        ExportOperation::Replay,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn export_operation(
    state: &AppState,
    identity: &CallerIdentity,
    proof: Option<&AuthenticatedExecutionConfiguration>,
    namespace: &str,
    tenant: &str,
    agent: &str,
    task_id: uuid::Uuid,
    profile: &str,
    headers: &HeaderMap,
    message: &acteon_core::TaskMessage,
    operation: ExportOperation,
) -> Response {
    if headers
        .get("a2a-version")
        .is_some_and(|value| value != A2A_PROTOCOL_VERSION)
    {
        return export_error(StatusCode::BAD_REQUEST, "unsupported_a2a_version");
    }
    if !identity.role.has_permission(Permission::Dispatch)
        || !identity.is_authorized(tenant, namespace, &format!("agent.{agent}"), "invoke")
    {
        return export_error(
            StatusCode::FORBIDDEN,
            "federation_export_authority_required",
        );
    }
    let Some(proof) = proof else {
        return export_error(StatusCode::FORBIDDEN, "private_authentication_required");
    };
    let Some(runtime) = &state.execution_authority else {
        return export_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "federation_export_unavailable",
        );
    };
    let request = AgentFederationExportRequest {
        namespace,
        tenant,
        source_agent_id: agent,
        source_task_id: task_id,
        export_profile_id: profile,
        message,
        authentication: proof,
    };
    let result = match operation {
        ExportOperation::Submit => runtime.submit_federation_export(request).await,
        ExportOperation::Observe => runtime.observe_federation_export(request).await,
        ExportOperation::Replay => runtime.replay_federation_export_idempotent(request).await,
    };
    match result {
        Ok(receipt) => (
            StatusCode::OK,
            [
                ("a2a-version", A2A_PROTOCOL_VERSION),
                ("cache-control", "no-store"),
            ],
            Json(FederationExportApiReceipt::from(receipt)),
        )
            .into_response(),
        Err(error) => export_runtime_error(error),
    }
}

fn export_runtime_error(error: FederationExportRuntimeError) -> Response {
    let (status, code) = match error {
        FederationExportRuntimeError::Invalid => {
            (StatusCode::BAD_REQUEST, "invalid_federation_export")
        }
        FederationExportRuntimeError::Forbidden => (
            StatusCode::FORBIDDEN,
            "federation_export_authority_required",
        ),
        FederationExportRuntimeError::NotFound => {
            (StatusCode::NOT_FOUND, "federation_export_unavailable")
        }
        FederationExportRuntimeError::Conflict => {
            (StatusCode::CONFLICT, "federation_export_conflict")
        }
        FederationExportRuntimeError::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "federation_export_unavailable",
        ),
    };
    export_error(status, code)
}

fn export_error(status: StatusCode, code: &str) -> Response {
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
