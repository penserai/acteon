//! Source-side authority for one exact outbound federation delivery.
//!
//! The exporter constructs signatures from a host-qualified profile, persists
//! the exact signed intent, registers a current local permit/budget attempt,
//! and only then lets the transport cross the network boundary.

use std::sync::Arc;

use acteon_core::{PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef, TaskMessage};
use acteon_governance::{
    AttemptStatus, AuthorityCoordinator, CoordinationError, RootBudgetLimits, StartRegistration,
    context::{AcceptedEffect, VerifiedExecutionContext},
    federation::{
        FederationEnvelope, FederationEnvelopeSigner, FederationImportReference,
        FederationInvocation,
    },
    permit::{PermitReference, PermittedAttempt, matches_effect, permitted_attempt_digest},
};
use acteon_time::Clock;

use crate::federation_transport::{
    DurableFederationTransport, FederationDeliveryEvidence, FederationDeliveryIntent,
    FederationDeliveryReceipt, FederationDeliveryStatus, FederationTransportError,
};

const MAX_EFFECTS: usize = 128;
const MAX_LIFETIME_MS: i64 = 24 * 60 * 60 * 1_000;

/// Immutable operator-qualified export route. It is deliberately not
/// deserializable; public requests and model output cannot install one.
pub struct QualifiedFederationExport {
    endpoint: String,
    issuer_domain: String,
    audience: String,
    trust_id: String,
    trust_revision: u64,
    key_id: String,
    verifying_key: String,
    issuer_epoch: u64,
    target: PrincipalIdentity,
    agent_resource: ResourceRef,
    binding_digest: String,
    skill: String,
    ingress_effect: AcceptedEffect,
    effects: Vec<AcceptedEffect>,
    limits: RootBudgetLimits,
    max_depth: usize,
    max_lifetime_ms: i64,
    local_export_effect: AcceptedEffect,
}

impl QualifiedFederationExport {
    #[allow(clippy::too_many_arguments)]
    pub fn new_trusted(
        endpoint: impl Into<String>,
        issuer_domain: impl Into<String>,
        audience: impl Into<String>,
        trust_id: impl Into<String>,
        trust_revision: u64,
        key_id: impl Into<String>,
        verifying_key: impl Into<String>,
        issuer_epoch: u64,
        target: PrincipalIdentity,
        agent_resource: ResourceRef,
        binding_digest: impl Into<String>,
        skill: impl Into<String>,
        ingress_effect: AcceptedEffect,
        effects: Vec<AcceptedEffect>,
        limits: RootBudgetLimits,
        max_depth: usize,
        max_lifetime_ms: i64,
        local_export_effect: AcceptedEffect,
    ) -> Result<Self, FederationExportError> {
        let candidate = Self {
            endpoint: endpoint.into(),
            issuer_domain: issuer_domain.into(),
            audience: audience.into(),
            trust_id: trust_id.into(),
            trust_revision,
            key_id: key_id.into(),
            verifying_key: verifying_key.into(),
            issuer_epoch,
            target,
            agent_resource,
            binding_digest: binding_digest.into(),
            skill: skill.into(),
            ingress_effect,
            effects,
            limits,
            max_depth,
            max_lifetime_ms,
            local_export_effect,
        };
        candidate.validate()?;
        Ok(candidate)
    }

    fn validate(&self) -> Result<(), FederationExportError> {
        let url = url::Url::parse(&self.endpoint).map_err(|_| FederationExportError::Invalid)?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !matches!(url.path(), "" | "/")
            || !valid_text(&self.issuer_domain, 255)
            || !valid_text(&self.audience, 255)
            || self.issuer_domain == self.audience
            || !valid_text(&self.trust_id, 1_024)
            || self.trust_revision == 0
            || !valid_text(&self.key_id, 1_024)
            || !valid_digest(&self.verifying_key)
            || self.issuer_epoch == 0
            || self.target.kind() != PrincipalKind::Agent
            || self.agent_resource.kind() != ResourceKind::Agent
            || !valid_digest(&self.binding_digest)
            || !valid_text(&self.skill, 120)
            || self.ingress_effect.operation != "agent.invoke"
            || !valid_effects(std::slice::from_ref(&self.ingress_effect))
            || !self.ingress_effect.resources.contains(&self.agent_resource)
            || self.ingress_effect.resources.iter().any(|resource| {
                resource.namespace() != self.agent_resource.namespace()
                    || resource.tenant() != self.agent_resource.tenant()
            })
            || !valid_effects(&self.effects)
            || self.effects.iter().any(|effect| {
                effect
                    .resources
                    .iter()
                    .any(|resource| !self.ingress_effect.resources.contains(resource))
            })
            || self.limits.max_units == 0
            || self.limits.max_concurrent == 0
            || self.limits.deadline_ms <= 0
            || self.max_depth == 0
            || self.max_depth >= acteon_governance::MAX_BUDGET_DEPTH
            || !(1..=MAX_LIFETIME_MS).contains(&self.max_lifetime_ms)
            || self.local_export_effect.operation != "federation.export"
            || !valid_effects(std::slice::from_ref(&self.local_export_effect))
            || self.local_export_effect.resources.iter().any(|resource| {
                !matches!(
                    resource.kind(),
                    ResourceKind::ExternalService | ResourceKind::Endpoint
                )
            })
        {
            return Err(FederationExportError::Invalid);
        }
        Ok(())
    }

    fn covers(&self, limits: &RootBudgetLimits, max_depth: usize, expires_at_ms: i64) -> bool {
        limits.max_units > 0
            && limits.max_units <= self.limits.max_units
            && limits.max_concurrent > 0
            && limits.max_concurrent <= self.limits.max_concurrent
            && limits.deadline_ms <= self.limits.deadline_ms
            && limits.deadline_ms <= expires_at_ms
            && max_depth > 0
            && max_depth <= self.max_depth
    }
}

pub struct FederationExportDependencies {
    pub coordinator: AuthorityCoordinator,
    pub clock: Arc<dyn Clock>,
    pub transport: Arc<DurableFederationTransport>,
}

pub struct GovernedFederationExporter {
    dependencies: FederationExportDependencies,
    signer: FederationEnvelopeSigner,
    profile: QualifiedFederationExport,
}

pub struct FederationExportRequest<'a> {
    pub context: &'a VerifiedExecutionContext,
    pub permits: &'a [PermitReference],
    pub export_id: &'a str,
    pub invocation_nonce: &'a str,
    pub message: &'a TaskMessage,
    pub issued_at_ms: i64,
    pub expires_at_ms: i64,
    pub limits: RootBudgetLimits,
    pub max_depth: usize,
}

#[derive(Debug, Clone)]
pub struct FederationExportReceipt {
    pub attempt_id: String,
    pub attempt_status: AttemptStatus,
    pub delivery: FederationDeliveryReceipt,
}

#[derive(Debug, thiserror::Error)]
pub enum FederationExportError {
    #[error("invalid outbound federation export")]
    Invalid,
    #[error("outbound federation export conflicts with retained intent")]
    Conflict,
    #[error(transparent)]
    Authority(#[from] CoordinationError),
    #[error(transparent)]
    Transport(#[from] FederationTransportError),
}

struct PreparedExport {
    parent: acteon_core::ExecutionContextReference,
    envelope: acteon_governance::federation::SignedFederationEnvelope,
    invocation: acteon_governance::federation::SignedFederationInvocation,
}

impl GovernedFederationExporter {
    pub fn new_trusted(
        dependencies: FederationExportDependencies,
        signer_secret: [u8; 32],
        profile: QualifiedFederationExport,
    ) -> Result<Self, FederationExportError> {
        profile.validate()?;
        let signer = FederationEnvelopeSigner::from_secret(signer_secret);
        if signer.verifying_key_hex() != profile.verifying_key {
            return Err(FederationExportError::Invalid);
        }
        Ok(Self {
            dependencies,
            signer,
            profile,
        })
    }

    /// Bounded, sanitized delivery evidence for independently authorized
    /// operator reconciliation. This grants no replay or delivery authority.
    pub async fn list_delivery_evidence(
        &self,
        namespace: &str,
        tenant: &str,
        after: Option<uuid::Uuid>,
        limit: usize,
    ) -> Result<Vec<FederationDeliveryEvidence>, FederationExportError> {
        self.dependencies
            .transport
            .list_evidence(namespace, tenant, after, limit)
            .await
            .map_err(Into::into)
    }

    /// Persist, reserve and deliver one exact export. Repeating the same stable
    /// IDs and timestamps reconstructs identical signed bytes and observes the
    /// same source attempt and destination delivery.
    pub async fn submit(
        &self,
        request: FederationExportRequest<'_>,
    ) -> Result<FederationExportReceipt, FederationExportError> {
        self.authorize(&request).await?;
        self.deliver_authorized(request).await
    }

    /// Persist the signed intent and atomically reserve its current local
    /// permit and budget authority without issuing network traffic.
    pub async fn authorize(
        &self,
        request: &FederationExportRequest<'_>,
    ) -> Result<FederationExportReceipt, FederationExportError> {
        let prepared = self.prepare(request, true)?;
        let intent = self.intent(request, &prepared);
        let delivery = self.dependencies.transport.register(intent).await?;
        let (attempt_id, _, attempt_status) = self.reserve(request, &delivery).await?;
        Ok(FederationExportReceipt {
            attempt_id,
            attempt_status,
            delivery,
        })
    }

    /// Recheck current authority before any registered stage that has not yet
    /// crossed its network boundary, then deliver and settle local accounting.
    pub async fn deliver_authorized(
        &self,
        request: FederationExportRequest<'_>,
    ) -> Result<FederationExportReceipt, FederationExportError> {
        let prepared = self.prepare(&request, true)?;
        let intent = self.intent(&request, &prepared);
        let observed = self.dependencies.transport.observe(intent).await?;
        let (attempt_id, token) = self.existing_attempt(&observed, &request).await?;
        if matches!(
            observed.status,
            FederationDeliveryStatus::Registered | FederationDeliveryStatus::Imported { .. }
        ) && let Err(error) = self
            .dependencies
            .coordinator
            .recheck_existing_effect_authority(
                request.context,
                request.permits,
                &self.profile.local_export_effect,
                self.dependencies.clock.as_ref(),
            )
            .await
        {
            if !definitive_authority_denial(&error) {
                return Err(error.into());
            }
            let delivery = self
                .dependencies
                .transport
                .refuse_registered(intent, "source_authority_denied")
                .await?;
            return self.settle(&attempt_id, &token, delivery).await;
        }
        let delivery = self
            .dependencies
            .transport
            .deliver_registered(intent)
            .await?;
        self.settle(&attempt_id, &token, delivery).await
    }

    /// Inspect and repair accounting from retained delivery evidence without
    /// network traffic or a fresh authority grant.
    pub async fn observe(
        &self,
        request: FederationExportRequest<'_>,
    ) -> Result<FederationExportReceipt, FederationExportError> {
        let prepared = self.prepare(&request, false)?;
        let delivery = self
            .dependencies
            .transport
            .observe(self.intent(&request, &prepared))
            .await?;
        let (attempt_id, token) = self.existing_attempt(&delivery, &request).await?;
        self.settle(&attempt_id, &token, delivery).await
    }

    /// Explicit exact replay. Current source authority is rechecked before the
    /// qualified transport may redeliver an ambiguous stage.
    pub async fn replay_idempotent(
        &self,
        request: FederationExportRequest<'_>,
    ) -> Result<FederationExportReceipt, FederationExportError> {
        let prepared = self.prepare(&request, true)?;
        self.dependencies
            .coordinator
            .recheck_existing_effect_authority(
                request.context,
                request.permits,
                &self.profile.local_export_effect,
                self.dependencies.clock.as_ref(),
            )
            .await?;
        let intent = self.intent(&request, &prepared);
        let observed = self.dependencies.transport.observe(intent).await?;
        let (attempt_id, token) = self.existing_attempt(&observed, &request).await?;
        let delivery = self
            .dependencies
            .transport
            .replay_idempotent(intent)
            .await?;
        self.settle(&attempt_id, &token, delivery).await
    }

    fn prepare(
        &self,
        request: &FederationExportRequest<'_>,
        require_live: bool,
    ) -> Result<PreparedExport, FederationExportError> {
        let parent = request
            .context
            .reference()
            .map_err(|_| FederationExportError::Invalid)?;
        let now_ms = self.dependencies.clock.now().timestamp_millis();
        if !valid_text(request.export_id, 1_024)
            || !valid_text(request.invocation_nonce, 1_024)
            || request.message.task_id.is_some()
            || request.message.validate().is_err()
            || request.issued_at_ms < 0
            || request.expires_at_ms <= request.issued_at_ms
            || request.expires_at_ms - request.issued_at_ms > self.profile.max_lifetime_ms
            || request.expires_at_ms > request.context.deadline_ms()
            || !self
                .profile
                .covers(&request.limits, request.max_depth, request.expires_at_ms)
            || (require_live && (now_ms < request.issued_at_ms || now_ms >= request.expires_at_ms))
        {
            return Err(FederationExportError::Invalid);
        }
        let envelope = self.signer.sign(&FederationEnvelope::new(
            request.export_id,
            &self.profile.trust_id,
            self.profile.trust_revision,
            &self.profile.issuer_domain,
            &self.profile.audience,
            &self.profile.key_id,
            self.profile.issuer_epoch,
            request.issued_at_ms,
            request.issued_at_ms,
            request.expires_at_ms,
            request.context.principal().clone(),
            self.profile.target.clone(),
            self.profile.agent_resource.clone(),
            self.profile.binding_digest.clone(),
            self.profile.skill.clone(),
            self.profile.ingress_effect.clone(),
            self.profile.effects.clone(),
            request.limits.clone(),
            request.max_depth,
        ))?;
        let invocation = self.signer.sign_invocation(&FederationInvocation::new(
            request.invocation_nonce,
            FederationImportReference {
                id: request.export_id.into(),
                envelope_digest: envelope.digest()?,
            },
            request.message.clone(),
            request.issued_at_ms,
            request.expires_at_ms,
        ))?;
        Ok(PreparedExport {
            parent,
            envelope,
            invocation,
        })
    }

    fn intent<'a>(
        &'a self,
        request: &'a FederationExportRequest<'_>,
        prepared: &'a PreparedExport,
    ) -> FederationDeliveryIntent<'a> {
        FederationDeliveryIntent {
            parent: &prepared.parent,
            reconciliation_resources: &self.profile.local_export_effect.resources,
            endpoint: &self.profile.endpoint,
            agent_id: self.profile.agent_resource.id(),
            envelope: &prepared.envelope,
            invocation: &prepared.invocation,
            message: request.message,
        }
    }

    async fn reserve(
        &self,
        request: &FederationExportRequest<'_>,
        delivery: &FederationDeliveryReceipt,
    ) -> Result<(String, String, AttemptStatus), FederationExportError> {
        let attempt_id = federation_export_attempt_id(delivery.delivery_id);
        let context_reference = request
            .context
            .reference()
            .map_err(|_| FederationExportError::Invalid)?;
        let registration = self
            .dependencies
            .coordinator
            .register_permitted_attempt(PermittedAttempt {
                id: &attempt_id,
                context: request.context,
                permits: request.permits,
                effect: &self.profile.local_export_effect,
                request_digest: context_reference.request_digest(),
                units: request.limits.max_units,
                clock: self.dependencies.clock.as_ref(),
            })
            .await?;
        let record = match registration {
            StartRegistration::New(record) | StartRegistration::Existing(record) => record,
        };
        if record.status == AttemptStatus::Settled
            && !matches!(
                delivery.status,
                FederationDeliveryStatus::Accepted { .. }
                    | FederationDeliveryStatus::Rejected { .. }
                    | FederationDeliveryStatus::Abandoned { .. }
            )
        {
            return Err(FederationExportError::Conflict);
        }
        Ok((attempt_id, record.token, record.status))
    }

    async fn existing_attempt(
        &self,
        delivery: &FederationDeliveryReceipt,
        request: &FederationExportRequest<'_>,
    ) -> Result<(String, String), FederationExportError> {
        let attempt_id = federation_export_attempt_id(delivery.delivery_id);
        let state = self.dependencies.coordinator.snapshot().await?;
        let record = state
            .starts
            .get(&attempt_id)
            .ok_or(FederationExportError::Conflict)?;
        let reference = request
            .context
            .reference()
            .map_err(|_| FederationExportError::Invalid)?;
        let expected_digest = permitted_attempt_digest(
            &reference,
            &self.profile.local_export_effect,
            request.limits.max_units,
            request.permits,
        )?;
        if record.subject != request.context.principal().id()
            || record.request_digest != expected_digest
            || record.reservation.as_ref().is_none_or(|reservation| {
                reservation.root_id != request.context.execution_id().to_string()
                    || reservation.units != request.limits.max_units
            })
            || record.resources
                != request
                    .context
                    .effect_registration_resources(&self.profile.local_export_effect)
                    .map_err(|_| FederationExportError::Invalid)?
                    .into_iter()
                    .collect()
        {
            return Err(FederationExportError::Conflict);
        }
        Ok((attempt_id, record.token.clone()))
    }

    async fn settle(
        &self,
        attempt_id: &str,
        token: &str,
        delivery: FederationDeliveryReceipt,
    ) -> Result<FederationExportReceipt, FederationExportError> {
        let attempt_status = match delivery.status {
            FederationDeliveryStatus::Accepted { .. }
            | FederationDeliveryStatus::Rejected { .. }
            | FederationDeliveryStatus::Abandoned { .. } => AttemptStatus::Settled,
            FederationDeliveryStatus::ImportUncertain
            | FederationDeliveryStatus::InvokeUncertain { .. } => AttemptStatus::Uncertain,
            FederationDeliveryStatus::Registered | FederationDeliveryStatus::Imported { .. } => {
                AttemptStatus::InFlight
            }
        };
        if attempt_status != AttemptStatus::InFlight {
            self.dependencies
                .coordinator
                .settle(attempt_id, token, attempt_status)
                .await?;
        }
        Ok(FederationExportReceipt {
            attempt_id: attempt_id.into(),
            attempt_status,
            delivery,
        })
    }
}

#[must_use]
pub fn federation_export_attempt_id(delivery_id: uuid::Uuid) -> String {
    format!("federation-export/{delivery_id}")
}

fn valid_effects(effects: &[AcceptedEffect]) -> bool {
    !effects.is_empty()
        && effects.len() <= MAX_EFFECTS
        && effects.iter().enumerate().all(|(index, effect)| {
            valid_text(&effect.operation, 1_024)
                && !effect.resources.is_empty()
                && effect.resources.len() <= 16
                && effect
                    .resources
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    == effect.resources.len()
                && !effects[..index]
                    .iter()
                    .any(|prior| matches_effect(prior, effect))
        })
}

fn valid_text(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.trim() == value
        && value != "*"
        && !value.chars().any(char::is_control)
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn definitive_authority_denial(error: &CoordinationError) -> bool {
    matches!(
        error,
        CoordinationError::Restricted
            | CoordinationError::PermitDenied(_)
            | CoordinationError::BudgetExhausted
            | CoordinationError::ConcurrencyExhausted
            | CoordinationError::DeadlineExceeded
            | CoordinationError::StaleAuthority
    )
}
