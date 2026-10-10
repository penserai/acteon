//! Agent-facing entry to host-qualified cross-domain exports.
//!
//! Public callers select only an installed profile and provide a message. The
//! source context, permits, endpoint, credentials, signer, trust revision and
//! budget ceiling remain host-owned. Stable issuance is retained before the
//! signed delivery journal so process restarts reconstruct identical bytes.

use super::{AgentPeerTransportError, ExecutionAuthorityRuntime};
use crate::auth::projection::AuthenticatedExecutionConfiguration;
use acteon_core::TaskMessage;
use acteon_executor::{
    federation_export::{FederationExportError, FederationExportReceipt, FederationExportRequest},
    federation_transport::FederationTransportError,
};
use acteon_governance::{CoordinationError, RootBudgetLimits};
use acteon_state::{KeyKind, StateKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const ISSUANCE_KIND: &str = "federation_export_issuance";
const ISSUANCE_SCHEMA: u32 = 2;
const MAX_ISSUANCE_BYTES: usize = 16 * 1024;

/// Agent-facing export request. Routing, credentials, signing keys, foreign
/// grants, timestamps, IDs and budget limits are deliberately absent.
pub struct AgentFederationExportRequest<'a> {
    pub namespace: &'a str,
    pub tenant: &'a str,
    pub source_agent_id: &'a str,
    pub source_task_id: uuid::Uuid,
    pub export_profile_id: &'a str,
    pub message: &'a TaskMessage,
    pub authentication: &'a AuthenticatedExecutionConfiguration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FederationExportRuntimeError {
    #[error("invalid federation export request")]
    Invalid,
    #[error("federation export is not authorized")]
    Forbidden,
    #[error("federation export source or profile was not found")]
    NotFound,
    #[error("federation export conflicts with retained intent")]
    Conflict,
    #[error("federation export runtime is unavailable")]
    Unavailable,
}

enum Operation {
    Submit,
    Observe,
    ReplayIdempotent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportIssuance {
    schema: u32,
    source_agent_id: String,
    source_task_id: uuid::Uuid,
    profile_id: String,
    profile_digest: String,
    issuer_epoch: u64,
    epoch_digest: String,
    parent_execution_id: uuid::Uuid,
    message_id: String,
    message_digest: String,
    export_id: String,
    invocation_nonce: String,
    issued_at_ms: i64,
    expires_at_ms: i64,
    limits: RootBudgetLimits,
    max_depth: usize,
}

impl ExecutionAuthorityRuntime {
    /// Submit one exact cross-domain invocation. Repeating the same source
    /// task, profile and message ID observes the original durable intent.
    pub async fn submit_federation_export(
        &self,
        request: AgentFederationExportRequest<'_>,
    ) -> Result<FederationExportReceipt, FederationExportRuntimeError> {
        self.federation_export_operation(request, Operation::Submit)
            .await
    }

    /// Observe retained export and accounting evidence without network calls.
    pub async fn observe_federation_export(
        &self,
        request: AgentFederationExportRequest<'_>,
    ) -> Result<FederationExportReceipt, FederationExportRuntimeError> {
        self.federation_export_operation(request, Operation::Observe)
            .await
    }

    /// Explicitly replay only a transport qualified for exact idempotency.
    pub async fn replay_federation_export_idempotent(
        &self,
        request: AgentFederationExportRequest<'_>,
    ) -> Result<FederationExportReceipt, FederationExportRuntimeError> {
        self.federation_export_operation(request, Operation::ReplayIdempotent)
            .await
    }

    async fn federation_export_operation(
        &self,
        request: AgentFederationExportRequest<'_>,
        operation: Operation,
    ) -> Result<FederationExportReceipt, FederationExportRuntimeError> {
        if request.namespace.is_empty()
            || request.tenant.is_empty()
            || request.source_agent_id.is_empty()
            || request.export_profile_id.is_empty()
            || request.message.validate().is_err()
        {
            return Err(FederationExportRuntimeError::Invalid);
        }
        let parent = self
            .agent_peer_tool_parent(
                request.namespace,
                request.tenant,
                request.source_agent_id,
                request.source_task_id,
                request.authentication,
            )
            .await
            .map_err(map_source_error)?;
        let scope = self
            .scopes
            .get(&(request.namespace.into(), request.tenant.into()))
            .ok_or(FederationExportRuntimeError::NotFound)?;
        let source = scope
            .prepared
            .agents
            .get(request.source_agent_id)
            .ok_or(FederationExportRuntimeError::NotFound)?;
        let direct = source
            .binding
            .service_plan()
            .ok_or(FederationExportRuntimeError::Unavailable)?
            .direct_effects();
        scope
            .coordinator
            .verify_service_runtime_binding(&parent, source.binding.digest(), direct)
            .await
            .map_err(|error| map_coordination_error(&error))?;
        let installed = scope
            .federation_exports
            .get(&(
                request.source_agent_id.into(),
                request.export_profile_id.into(),
            ))
            .ok_or(FederationExportRuntimeError::NotFound)?;
        if !matches!(operation, Operation::Observe) {
            let local_effect = installed
                .declaration
                .local_effect(request.namespace, request.tenant)
                .map_err(|_| FederationExportRuntimeError::Invalid)?;
            scope
                .coordinator
                .recheck_existing_effect_authority(
                    &parent,
                    &source.declaration.recipient_permits,
                    &local_effect,
                    self.clock.as_ref(),
                )
                .await
                .map_err(|error| map_coordination_error(&error))?;
        }
        let issuance = self
            .federation_export_issuance(
                request.namespace,
                request.tenant,
                request.source_agent_id,
                request.source_task_id,
                request.message,
                &parent,
                &installed.declaration,
                &installed.profile_digest,
                matches!(operation, Operation::Submit),
            )
            .await?;
        let exporter = installed
            .exporters
            .get(&issuance.issuer_epoch)
            .ok_or(FederationExportRuntimeError::Conflict)?;
        let export = FederationExportRequest {
            context: &parent,
            permits: &source.declaration.recipient_permits,
            export_id: &issuance.export_id,
            invocation_nonce: &issuance.invocation_nonce,
            message: request.message,
            issued_at_ms: issuance.issued_at_ms,
            expires_at_ms: issuance.expires_at_ms,
            limits: issuance.limits,
            max_depth: issuance.max_depth,
        };
        match operation {
            Operation::Submit => exporter.submit(export).await,
            Operation::Observe => exporter.observe(export).await,
            Operation::ReplayIdempotent => exporter.replay_idempotent(export).await,
        }
        .map_err(map_export_error)
    }

    #[allow(clippy::too_many_arguments)]
    async fn federation_export_issuance(
        &self,
        namespace: &str,
        tenant: &str,
        source_agent_id: &str,
        source_task_id: uuid::Uuid,
        message: &TaskMessage,
        parent: &acteon_governance::context::VerifiedExecutionContext,
        profile: &crate::config::FederationExportConfig,
        profile_digest: &str,
        create: bool,
    ) -> Result<ExportIssuance, FederationExportRuntimeError> {
        let message_digest = digest_json(message)?;
        let key = issuance_key(
            namespace,
            tenant,
            source_task_id,
            &profile.id,
            &message.message_id,
        );
        if self
            .state
            .get(&key)
            .await
            .map_err(|_| FederationExportRuntimeError::Unavailable)?
            .is_none()
        {
            if !create {
                return Err(FederationExportRuntimeError::NotFound);
            }
            let epoch = profile
                .active_epoch()
                .map_err(|_| FederationExportRuntimeError::Invalid)?;
            let epoch_digest = digest_json(epoch)?;
            let now_ms = self.clock.now().timestamp_millis();
            let expires_at_ms = now_ms
                .checked_add(profile.max_lifetime_ms)
                .map(|candidate| {
                    candidate
                        .min(parent.deadline_ms())
                        .min(epoch.grant.limits.deadline_ms)
                })
                .filter(|deadline| *deadline > now_ms)
                .ok_or(FederationExportRuntimeError::Forbidden)?;
            let candidate = ExportIssuance {
                schema: ISSUANCE_SCHEMA,
                source_agent_id: source_agent_id.into(),
                source_task_id,
                profile_id: profile.id.clone(),
                profile_digest: profile_digest.into(),
                issuer_epoch: epoch.issuer_epoch,
                epoch_digest,
                parent_execution_id: parent.execution_id(),
                message_id: message.message_id.clone(),
                message_digest: message_digest.clone(),
                export_id: uuid::Uuid::new_v4().to_string(),
                invocation_nonce: uuid::Uuid::new_v4().to_string(),
                issued_at_ms: now_ms,
                expires_at_ms,
                limits: RootBudgetLimits {
                    max_units: epoch.grant.limits.max_units,
                    max_concurrent: epoch.grant.limits.max_concurrent,
                    deadline_ms: expires_at_ms,
                },
                max_depth: epoch.grant.max_depth,
            };
            let encoded = self.encode_export_issuance(&candidate)?;
            self.state
                .check_and_set(&key, &encoded, None)
                .await
                .map_err(|_| FederationExportRuntimeError::Unavailable)?;
        }
        let raw = self
            .state
            .get(&key)
            .await
            .map_err(|_| FederationExportRuntimeError::Unavailable)?
            .ok_or(FederationExportRuntimeError::Unavailable)?;
        let issuance = self.decode_export_issuance(&raw)?;
        let epoch = profile
            .epochs
            .iter()
            .find(|epoch| epoch.issuer_epoch == issuance.issuer_epoch)
            .ok_or(FederationExportRuntimeError::Conflict)?;
        let epoch_digest = digest_json(epoch)?;
        if issuance.schema != ISSUANCE_SCHEMA
            || issuance.source_agent_id != source_agent_id
            || issuance.source_task_id != source_task_id
            || issuance.profile_id != profile.id
            || issuance.profile_digest != profile_digest
            || issuance.issuer_epoch == 0
            || issuance.epoch_digest != epoch_digest
            || issuance.parent_execution_id != parent.execution_id()
            || issuance.message_id != message.message_id
            || issuance.message_digest != message_digest
            || issuance.issued_at_ms < 0
            || issuance.expires_at_ms <= issuance.issued_at_ms
            || issuance.limits.deadline_ms != issuance.expires_at_ms
            || issuance.limits.max_units == 0
            || issuance.limits.max_units > epoch.grant.limits.max_units
            || issuance.limits.max_concurrent == 0
            || issuance.limits.max_concurrent > epoch.grant.limits.max_concurrent
            || issuance.max_depth == 0
            || issuance.max_depth > epoch.grant.max_depth
        {
            return Err(FederationExportRuntimeError::Conflict);
        }
        Ok(issuance)
    }

    fn encode_export_issuance(
        &self,
        issuance: &ExportIssuance,
    ) -> Result<String, FederationExportRuntimeError> {
        let raw =
            serde_json::to_string(issuance).map_err(|_| FederationExportRuntimeError::Invalid)?;
        if raw.len() > MAX_ISSUANCE_BYTES {
            return Err(FederationExportRuntimeError::Invalid);
        }
        self.encryptor
            .as_ref()
            .map_or(Ok(raw.clone()), |encryptor| {
                encryptor
                    .encrypt_str(&raw)
                    .map_err(|_| FederationExportRuntimeError::Unavailable)
            })
    }

    fn decode_export_issuance(
        &self,
        raw: &str,
    ) -> Result<ExportIssuance, FederationExportRuntimeError> {
        if raw.len() > MAX_ISSUANCE_BYTES * 4 {
            return Err(FederationExportRuntimeError::Conflict);
        }
        let decoded = self.encryptor.as_ref().map_or_else(
            || Ok(raw.to_owned()),
            |encryptor| {
                encryptor
                    .decrypt_str(raw)
                    .map_err(|_| FederationExportRuntimeError::Unavailable)
            },
        )?;
        serde_json::from_str(&decoded).map_err(|_| FederationExportRuntimeError::Conflict)
    }
}

fn issuance_key(
    namespace: &str,
    tenant: &str,
    source_task_id: uuid::Uuid,
    profile_id: &str,
    message_id: &str,
) -> StateKey {
    let mut digest = Sha256::new();
    digest.update(b"acteon.federation.export.issuance.v1");
    digest.update(source_task_id.as_bytes());
    for value in [profile_id, message_id] {
        digest.update(value.len().to_be_bytes());
        digest.update(value.as_bytes());
    }
    StateKey::new(
        namespace,
        tenant,
        KeyKind::Custom(ISSUANCE_KIND.into()),
        format!("{:x}", digest.finalize()),
    )
}

fn digest_json<T: Serialize>(value: &T) -> Result<String, FederationExportRuntimeError> {
    serde_json::to_vec(value)
        .map(|encoded| format!("{:x}", Sha256::digest(encoded)))
        .map_err(|_| FederationExportRuntimeError::Invalid)
}

pub(super) fn profile_binding_digest(
    profile: &crate::config::FederationExportConfig,
    local_audience: &str,
) -> Result<String, FederationExportRuntimeError> {
    digest_json(&(
        "acteon.federation.export.profile.v2",
        &profile.id,
        &profile.source_agent_id,
        &profile.remote_origin,
        &profile.remote_audience,
        local_audience,
        profile.max_lifetime_ms,
        profile.timeout_ms,
        &profile.internal_hosts,
    ))
}

fn map_source_error(error: AgentPeerTransportError) -> FederationExportRuntimeError {
    match error {
        AgentPeerTransportError::Invalid => FederationExportRuntimeError::Invalid,
        AgentPeerTransportError::Forbidden => FederationExportRuntimeError::Forbidden,
        AgentPeerTransportError::NotFound => FederationExportRuntimeError::NotFound,
        AgentPeerTransportError::Conflict => FederationExportRuntimeError::Conflict,
        AgentPeerTransportError::Unavailable => FederationExportRuntimeError::Unavailable,
    }
}

fn map_coordination_error(error: &CoordinationError) -> FederationExportRuntimeError {
    match error {
        CoordinationError::State(_)
        | CoordinationError::Contention
        | CoordinationError::Capacity => FederationExportRuntimeError::Unavailable,
        CoordinationError::Invalid(_) | CoordinationError::Conflict => {
            FederationExportRuntimeError::Conflict
        }
        CoordinationError::StaleAuthority
        | CoordinationError::Restricted
        | CoordinationError::RegistryMutationUnresolved
        | CoordinationError::BudgetExhausted
        | CoordinationError::ConcurrencyExhausted
        | CoordinationError::DeadlineExceeded
        | CoordinationError::PermitDenied(_) => FederationExportRuntimeError::Forbidden,
    }
}

fn map_export_error(error: FederationExportError) -> FederationExportRuntimeError {
    match error {
        FederationExportError::Invalid => FederationExportRuntimeError::Invalid,
        FederationExportError::Conflict => FederationExportRuntimeError::Conflict,
        FederationExportError::Authority(error) => map_coordination_error(&error),
        FederationExportError::Transport(error) => match error {
            FederationTransportError::Invalid => FederationExportRuntimeError::Invalid,
            FederationTransportError::Conflict => FederationExportRuntimeError::Conflict,
            FederationTransportError::Refused => FederationExportRuntimeError::Forbidden,
            FederationTransportError::Unavailable => FederationExportRuntimeError::Unavailable,
        },
    }
}

#[cfg(test)]
mod tests {
    use acteon_core::{PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef};
    use acteon_governance::{
        RootBudgetLimits,
        context::AcceptedEffect,
        federation::{FederationEnvelopeSigner, FederationGrantCeiling},
    };

    use super::{digest_json, profile_binding_digest};
    use crate::config::{FederationExportConfig, FederationExportEpochConfig};

    fn profile() -> FederationExportConfig {
        let source = PrincipalIdentity::new("agent/notifier", PrincipalKind::Agent).unwrap();
        let target = PrincipalIdentity::new("agent/remote", PrincipalKind::Agent).unwrap();
        let agent = ResourceRef::new(ResourceKind::Agent, "prod", "acme", "remote").unwrap();
        let endpoint =
            ResourceRef::new(ResourceKind::Endpoint, "prod", "acme", "remote-api").unwrap();
        let grant = FederationGrantCeiling {
            source,
            target,
            agent_resource: agent.clone(),
            binding_digest: "b".repeat(64),
            skill: "triage".into(),
            ingress_effect: AcceptedEffect {
                operation: "agent.invoke".into(),
                resources: vec![agent, endpoint.clone()],
            },
            effects: vec![AcceptedEffect {
                operation: "provider.execute".into(),
                resources: vec![endpoint],
            }],
            limits: RootBudgetLimits {
                max_units: 3,
                max_concurrent: 1,
                deadline_ms: 4_102_444_800_000,
            },
            max_depth: 2,
        };
        FederationExportConfig {
            id: "remote-triage".into(),
            source_agent_id: "notifier".into(),
            remote_origin: "https://remote.example".into(),
            remote_audience: "acteon://remote/prod".into(),
            active_epoch: 1,
            epochs: vec![FederationExportEpochConfig {
                trust_id: "local-prod".into(),
                trust_revision: 1,
                key_id: "local-key-v1".into(),
                verifying_key: FederationEnvelopeSigner::from_secret([11; 32]).verifying_key_hex(),
                signer_key_env: "ACTEON_FEDERATION_SIGNER_V1".into(),
                issuer_epoch: 1,
                credential_env: "ACTEON_REMOTE_CREDENTIAL_V1".into(),
                peer_capability: crate::config::FederationPeerCapabilityConfig {
                    key_id: "remote-capability-v1".into(),
                    verifying_key:
                        acteon_governance::federation::FederationCapabilitySigner::from_secret(
                            [24; 32],
                        )
                        .verifying_key_hex(),
                    minimum_issuer_epoch: 1,
                    max_clock_skew_ms: 30_000,
                    protocol_revision: "acteon-federation-v1".into(),
                    authentication_scheme: "bearer".into(),
                    require_peer_finality: false,
                },
                grant,
            }],
            max_lifetime_ms: 60_000,
            timeout_ms: 10_000,
            internal_hosts: vec![],
        }
    }

    #[test]
    fn rotation_preserves_common_binding_and_isolates_epoch_material() {
        let original = profile();
        let common = profile_binding_digest(&original, "acteon://local/prod").unwrap();
        let old_epoch = digest_json(&original.epochs[0]).unwrap();

        let mut rotated = original.clone();
        let mut next = rotated.epochs[0].clone();
        next.issuer_epoch = 2;
        next.trust_revision = 2;
        next.key_id = "local-key-v2".into();
        next.verifying_key = FederationEnvelopeSigner::from_secret([12; 32]).verifying_key_hex();
        next.signer_key_env = "ACTEON_FEDERATION_SIGNER_V2".into();
        next.credential_env = "ACTEON_REMOTE_CREDENTIAL_V2".into();
        next.grant.binding_digest = "c".repeat(64);
        rotated.epochs.push(next);
        rotated.active_epoch = 2;

        assert_eq!(
            profile_binding_digest(&rotated, "acteon://local/prod").unwrap(),
            common
        );
        assert_eq!(digest_json(&rotated.epochs[0]).unwrap(), old_epoch);
        assert_ne!(
            digest_json(&rotated.epochs[1]).unwrap(),
            old_epoch,
            "a retained issuance must select exactly one epoch"
        );
        rotated.remote_audience = "acteon://other/prod".into();
        assert_ne!(
            profile_binding_digest(&rotated, "acteon://local/prod").unwrap(),
            common
        );
        rotated.remote_audience = original.remote_audience;
        assert_ne!(
            profile_binding_digest(&rotated, "acteon://other/prod").unwrap(),
            common
        );
    }
}
