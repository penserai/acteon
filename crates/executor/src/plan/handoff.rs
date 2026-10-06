//! Durable host work provenance through the configured state backend.
//!
//! The job ID must come from the authoritative host work record. These records
//! preserve provenance; they never confer permission to start an effect.
use super::{
    PlanCallSite, PlanError, QualifiedChainInvocation, QualifiedChainPlan, matches_effect,
};
use crate::{catalog::QualifiedProviderCatalog, governed::governed_provider_input_digest};
use acteon_core::{Action, ChainConfig, ExecutionContextReference};
use acteon_crypto::PayloadEncryptor;
use acteon_governance::{
    RootBudgetLimits,
    context::{
        ContextError, ExecutionContextHandle, TrustedContextStore, VerifiedExecutionContext,
    },
    permit::{PermitReference, permit_revision_tag},
};
use acteon_provider::DynProvider;
use acteon_state::{KeyKind, StateError, StateKey, StateStore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Arc};
use uuid::Uuid;

const FORMAT: u32 = 1;
const MAX_RECORD_BYTES: usize = 512 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum HandoffError {
    #[error(transparent)]
    State(#[from] StateError),
    #[error(transparent)]
    Context(#[from] ContextError),
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error("pinned job missing, corrupt or bound to different work")]
    Binding,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PinnedJob {
    format: u32,
    job_id: Uuid,
    entry: String,
    plan_digest: String,
    request_digest: String,
    origin: Action,
    definitions: BTreeMap<String, ChainConfig>,
    root: ExecutionContextReference,
    permits: Vec<PermitReference>,
}

/// The host must establish ownership of the job before recovering it. A public
/// reference or user-chosen job ID is not proof of ownership or authority.
pub struct PlanHandoffStore {
    state: Arc<dyn StateStore>,
    encryptor: Option<Arc<PayloadEncryptor>>,
    namespace: String,
    tenant: String,
}

/// Reconstructed from pinned definitions and the current qualified provider
/// catalog, with independently verified signed root provenance. No Deserialize.
pub struct RecoveredPlanJob {
    job_id: Uuid,
    invocation: QualifiedChainInvocation,
    root: VerifiedExecutionContext,
    permits: Vec<PermitReference>,
    origin: Action,
}
impl RecoveredPlanJob {
    #[must_use]
    pub const fn job_id(&self) -> Uuid {
        self.job_id
    }
    #[must_use]
    pub fn invocation(&self) -> &QualifiedChainInvocation {
        &self.invocation
    }
    #[must_use]
    pub fn root(&self) -> &VerifiedExecutionContext {
        &self.root
    }
    #[must_use]
    pub fn permits(&self) -> &[PermitReference] {
        &self.permits
    }
    #[must_use]
    pub fn origin(&self) -> &Action {
        &self.origin
    }
}
impl PlanHandoffStore {
    pub fn new(
        state: Arc<dyn StateStore>,
        namespace: &str,
        tenant: &str,
    ) -> Result<Self, HandoffError> {
        // Reuse the platform's scope validation rather than accepting raw keys.
        acteon_core::ResourceRef::new(
            acteon_core::ResourceKind::Chain,
            namespace,
            tenant,
            "handoff",
        )
        .map_err(|_| HandoffError::Binding)?;
        Ok(Self {
            state,
            encryptor: None,
            namespace: namespace.into(),
            tenant: tenant.into(),
        })
    }
    /// Use the deployment's payload encryption and key rotation policy for
    /// original inputs and pinned work records, just as for execution receipts.
    #[must_use]
    pub fn with_encryptor(mut self, encryptor: Arc<PayloadEncryptor>) -> Self {
        self.encryptor = Some(encryptor);
        self
    }
    fn encode<T: Serialize>(&self, value: &T) -> Result<String, HandoffError> {
        let raw = serde_json::to_string(value).map_err(|_| HandoffError::Binding)?;
        if raw.len() > MAX_RECORD_BYTES {
            return Err(PlanError::Capacity.into());
        }
        match &self.encryptor {
            Some(encryptor) => encryptor
                .encrypt_str(&raw)
                .map_err(|_| HandoffError::Binding),
            None => Ok(raw),
        }
    }
    fn decode<T: serde::de::DeserializeOwned>(&self, encoded: &str) -> Result<T, HandoffError> {
        if encoded.len() > MAX_RECORD_BYTES * 2 {
            return Err(HandoffError::Binding);
        }
        let raw = match &self.encryptor {
            Some(encryptor) => encryptor
                .decrypt_str(encoded)
                .map_err(|_| HandoffError::Binding)?,
            None => encoded.to_owned(),
        };
        if raw.len() > MAX_RECORD_BYTES {
            return Err(HandoffError::Binding);
        }
        serde_json::from_str(&raw).map_err(|_| HandoffError::Binding)
    }
    fn key(&self, job_id: Uuid) -> Result<StateKey, HandoffError> {
        if job_id.is_nil() {
            return Err(HandoffError::Binding);
        }
        Ok(StateKey::new(
            self.namespace.clone(),
            self.tenant.clone(),
            KeyKind::Custom("governed_plan_job".into()),
            job_id.to_string(),
        ))
    }
    fn validate_scope(&self, record: &PinnedJob, job_id: Uuid) -> Result<(), HandoffError> {
        if record.format != FORMAT
            || record.job_id != job_id
            || record.root.namespace() != self.namespace
            || record.root.tenant() != self.tenant
            || record.origin.namespace.as_str() != self.namespace
            || record.origin.tenant.as_str() != self.tenant
            || record.permits.is_empty()
            || record.permits.len() > 128
        {
            return Err(HandoffError::Binding);
        }
        Ok(())
    }
    async fn read(&self, job_id: Uuid) -> Result<PinnedJob, HandoffError> {
        let raw = self
            .state
            .get(&self.key(job_id)?)
            .await?
            .ok_or(HandoffError::Binding)?;
        let record: PinnedJob = self.decode(&raw)?;
        self.validate_scope(&record, job_id)?;
        Ok(record)
    }
    /// Persist once before making a job discoverable. Repeating after a lost
    /// acknowledgement observes the original binding; it never replaces it.
    /// No TTL: dropping provenance while durable work remains is unsafe.
    pub async fn persist(
        &self,
        job_id: Uuid,
        invocation: &QualifiedChainInvocation,
        origin: &Action,
        root: &VerifiedExecutionContext,
        permits: &[PermitReference],
    ) -> Result<(), HandoffError> {
        invocation.verify_root(root)?;
        let rebound = invocation.plan.bind_input(origin)?;
        if rebound.request_digest != invocation.request_digest
            || permit_revision_tag(permits).map_err(|_| HandoffError::Binding)?
                != root.accepted_ceiling_revision()
        {
            return Err(HandoffError::Binding);
        }
        let record = PinnedJob {
            format: FORMAT,
            job_id,
            entry: invocation.plan.entry.clone(),
            plan_digest: invocation.plan.digest.clone(),
            request_digest: invocation.request_digest.clone(),
            origin: origin.clone(),
            definitions: invocation.plan.definitions.clone(),
            root: root.reference()?,
            permits: permits.to_vec(),
        };
        self.validate_scope(&record, job_id)?;
        let raw = self.encode(&record)?;
        if !self
            .state
            .check_and_set(&self.key(job_id)?, &raw, None)
            .await?
        {
            let previous = self.read(job_id).await?;
            // UUID, trace and transport fields do not establish semantic input.
            // Retain the first full action; compare the qualified binding only.
            if previous.root != record.root
                || previous.plan_digest != record.plan_digest
                || previous.request_digest != record.request_digest
                || permit_revision_tag(&previous.permits).map_err(|_| HandoffError::Binding)?
                    != permit_revision_tag(&record.permits).map_err(|_| HandoffError::Binding)?
                || previous.entry != record.entry
            {
                return Err(HandoffError::Binding);
            }
            // Check stored definitions/input too, rather than trusting a digest label.
            let qualified = Arc::new(QualifiedChainPlan::new_trusted(
                &self.namespace,
                &self.tenant,
                &previous.entry,
                &previous.definitions,
                invocation.plan.catalog.clone(),
            )?);
            if qualified.digest != record.plan_digest
                || qualified.bind_input(&previous.origin)?.request_digest != record.request_digest
            {
                return Err(HandoffError::Binding);
            }
        }
        Ok(())
    }
    /// Restriction-only historical recovery. Provider removal cannot disable
    /// cancellation. This verifies signed root and original input provenance,
    /// but never qualifies a plan or authorizes another external effect.
    pub async fn recover_root_for_cancellation(
        &self,
        job_id: Uuid,
        contexts: &TrustedContextStore,
    ) -> Result<VerifiedExecutionContext, HandoffError> {
        let record = self.read(job_id).await?;
        let digest = super::plan_request_digest(&record.plan_digest, &record.origin)?;
        let root = contexts
            .recover_reference_for_observation(&record.root)
            .await?;
        if digest != record.request_digest
            || record.root.request_digest() != digest
            || root.root_execution_id() != root.execution_id()
            || permit_revision_tag(&record.permits).map_err(|_| HandoffError::Binding)?
                != root.accepted_ceiling_revision()
        {
            return Err(HandoffError::Binding);
        }
        Ok(root)
    }

    /// Recovery verifies historical provenance even after revocation/expiry.
    /// Fresh child admission and effect registration still check current rights.
    /// Changed route qualification refuses execution instead of adopting it.
    pub async fn recover(
        &self,
        job_id: Uuid,
        catalog: QualifiedProviderCatalog,
        contexts: &TrustedContextStore,
    ) -> Result<RecoveredPlanJob, HandoffError> {
        let record = self.read(job_id).await?;
        let plan = Arc::new(QualifiedChainPlan::new_trusted(
            &self.namespace,
            &self.tenant,
            &record.entry,
            &record.definitions,
            catalog,
        )?);
        if plan.digest != record.plan_digest {
            return Err(HandoffError::Binding);
        }
        let invocation = plan.bind_input(&record.origin)?;
        if invocation.request_digest != record.request_digest {
            return Err(HandoffError::Binding);
        }
        let root = contexts
            .recover_reference_for_observation(&record.root)
            .await?;
        invocation.verify_root(&root)?;
        if permit_revision_tag(&record.permits).map_err(|_| HandoffError::Binding)?
            != root.accepted_ceiling_revision()
        {
            return Err(HandoffError::Binding);
        }
        Ok(RecoveredPlanJob {
            job_id,
            invocation,
            root,
            permits: record.permits,
            origin: record.origin,
        })
    }
}

/// A host-selected logical provider attempt. Keep this identity through state
/// write/admission repair; choose a new identity only for an actual new attempt.
pub struct ReservePlanCall<'a> {
    pub logical_attempt: Uuid,
    pub parent: &'a VerifiedExecutionContext,
    pub call_site: &'a PlanCallSite,
    pub chain_path: &'a [String],
    pub action: &'a Action,
    pub selected: &'a Arc<dyn DynProvider>,
    pub limits: RootBudgetLimits,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredPlanCall {
    format: u32,
    job_id: Uuid,
    root: ExecutionContextReference,
    parent: ExecutionContextReference,
    plan_input: String,
    call_site: serde_json::Value,
    chain_path: Vec<String>,
    input_digest: String,
    limits: RootBudgetLimits,
    admission_key: String,
    handle: ExecutionContextHandle,
    execution_id: Uuid,
}
pub struct ReservedPlanCall(StoredPlanCall);
impl ReservedPlanCall {
    #[must_use]
    pub fn admission_key(&self) -> &str {
        &self.0.admission_key
    }
    #[must_use]
    pub fn handle(&self) -> &ExecutionContextHandle {
        &self.0.handle
    }
    #[must_use]
    pub const fn execution_id(&self) -> Uuid {
        self.0.execution_id
    }

    /// Historical signed provenance, never permission to execute again.
    pub async fn observe_context(
        &self,
        contexts: &TrustedContextStore,
    ) -> Result<Option<VerifiedExecutionContext>, HandoffError> {
        match contexts
            .recover_for_observation(
                &self.0.handle,
                &acteon_governance::context::ContextBinding {
                    execution_id: self.0.execution_id,
                    principal: self.0.root.principal().clone(),
                    request_digest: self.0.input_digest.clone(),
                },
            )
            .await
        {
            Ok(context) => {
                if context.root_execution_id() != self.0.root.execution_id()
                    || context.parent_reference() != Some(&self.0.parent)
                {
                    return Err(HandoffError::Binding);
                }
                Ok(Some(context))
            }
            Err(ContextError::Missing) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}
impl PlanHandoffStore {
    /// Atomically retain one identity for competing replicas before child
    /// admission. A lost write acknowledgement must recover this exact record.
    pub async fn reserve_provider_call(
        &self,
        job: &RecoveredPlanJob,
        request: ReservePlanCall<'_>,
    ) -> Result<ReservedPlanCall, HandoffError> {
        self.provider_call(job, request, true)
            .await?
            .ok_or(HandoffError::Binding)
    }

    /// Inspect a retained logical call without allocating or publishing work.
    pub async fn recover_provider_call(
        &self,
        job: &RecoveredPlanJob,
        request: ReservePlanCall<'_>,
    ) -> Result<Option<ReservedPlanCall>, HandoffError> {
        self.provider_call(job, request, false).await
    }

    async fn provider_call(
        &self,
        job: &RecoveredPlanJob,
        request: ReservePlanCall<'_>,
        create: bool,
    ) -> Result<Option<ReservedPlanCall>, HandoffError> {
        let pinned = self.read(job.job_id).await?;
        if pinned.root != job.root.reference()?
            || pinned.request_digest != job.invocation.request_digest
            || request.logical_attempt.is_nil()
            || request.parent.root_execution_id() != job.root.execution_id()
            || request.parent.principal() != job.root.principal()
            || request.parent.representation() != job.root.representation()
        {
            return Err(HandoffError::Binding);
        }
        let plan = &job.invocation.plan;
        let declared = plan
            .routes
            .get(request.call_site)
            .ok_or(HandoffError::Binding)?;
        let actual = plan
            .catalog
            .resolve(request.action, request.selected)
            .map_err(|_| PlanError::Unqualified)?;
        if !matches_effect(actual.effect(), &declared.effect) {
            return Err(PlanError::Unqualified.into());
        }
        plan.restrictions(request.call_site, request.chain_path)?;
        if request.limits.max_units == 0
            || request.limits.max_concurrent == 0
            || request.limits.deadline_ms > job.root.deadline_ms()
        {
            return Err(HandoffError::Binding);
        }
        let site = serde_json::to_value(request.call_site).map_err(|_| HandoffError::Binding)?;
        let identity = super::canonical_bytes(&serde_json::json!({
            "format":"acteon-plan-call:v1", "job":job.job_id,
            "site":site, "path":request.chain_path, "attempt":request.logical_attempt,
        }))?;
        let admission_key = format!("{:x}", Sha256::digest(identity));
        let key = StateKey::new(
            self.namespace.clone(),
            self.tenant.clone(),
            KeyKind::Custom("governed_plan_call".into()),
            admission_key.clone(),
        );
        let candidate = StoredPlanCall {
            format: FORMAT,
            job_id: job.job_id,
            root: job.root.reference()?,
            parent: request.parent.reference()?,
            plan_input: job.invocation.request_digest.clone(),
            call_site: site,
            chain_path: request.chain_path.to_vec(),
            input_digest: governed_provider_input_digest(request.action)
                .map_err(|_| PlanError::Invalid)?,
            limits: request.limits,
            admission_key,
            handle: ExecutionContextHandle::new(),
            execution_id: Uuid::new_v4(),
        };
        if create {
            let raw = self.encode(&candidate)?;
            if self.state.check_and_set(&key, &raw, None).await? {
                return Ok(Some(ReservedPlanCall(candidate)));
            }
        }
        let Some(stored) = self.state.get(&key).await? else {
            return if create {
                Err(HandoffError::Binding)
            } else {
                Ok(None)
            };
        };
        let prior: StoredPlanCall = self.decode(&stored)?;
        if prior.format != FORMAT
            || prior.job_id != candidate.job_id
            || prior.root != candidate.root
            || prior.parent != candidate.parent
            || prior.plan_input != candidate.plan_input
            || prior.call_site != candidate.call_site
            || prior.chain_path != candidate.chain_path
            || prior.input_digest != candidate.input_digest
            || prior.limits != candidate.limits
            || prior.admission_key != candidate.admission_key
            || prior.execution_id.is_nil()
        {
            return Err(HandoffError::Binding);
        }
        Ok(Some(ReservedPlanCall(prior)))
    }
}
