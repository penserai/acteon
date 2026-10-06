//! Independent finality evidence for an existing attempt, never a send permit.
//! Verifiers run locally over evidence supplied by a trusted provider adapter.
//! A remote probe needs its own qualified effect and current maintenance permit.
use super::{
    GovernedProviderError, GovernedProviderExecutor, GovernedProviderReceipt,
    GovernedProviderStatus, Operation, Runtime, attempt_id, valid_text,
};
use acteon_core::{
    ActionError, ActionOutcome, ExecutionContextReference, PrincipalIdentity, ProviderResponse,
};
use acteon_governance::{
    AttemptEvidenceReference, AttemptReconciliationReference, AttemptStatus, CoordinationError,
    StartRecord, reconciliation::ReconciliationAuthorization,
};
use acteon_state::{KeyKind, StateKey};
use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Arc};
use uuid::Uuid;

mod store;
pub use store::ProviderReconciliationStore;

pub const RECONCILIATION_KIND: &str = "governed_provider_reconciliation";
pub(super) const MAX_PROOF_BYTES: usize = 64 * 1024;

/// Correlation material from the authoritative operation and registered start.
/// It is not authority, a lease, or a credential. External issuers must correlate
/// it with the actual provider attempt before attesting irrevocable finality.
/// In-flight no-effect receipts require the external source to fence every
/// possible future delivery of that attempt. A lookup returning no record cannot
/// establish finality while a worker or external service may still perform it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconciliationAttempt {
    pub context: ExecutionContextReference,
    pub action_id: String,
    pub attempt_id: String,
    pub ordinal: u32,
    pub token: String,
    pub binding_digest: String,
}

/// Only definitive completion or definitive absence of an effect is accepted.
/// A timeout, elapsed lease, model response or operator assumption is neither.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProviderFinality {
    Completed { response: ProviderResponse },
    NoEffect { reason: String },
}
impl ProviderFinality {
    fn outcome(self, ordinal: u32) -> Result<ActionOutcome, GovernedProviderError> {
        Ok(match self {
            Self::Completed { response } => ActionOutcome::Executed(response),
            Self::NoEffect { reason } => {
                if !valid_text(&reason) || reason.len() > 4096 {
                    return Err(GovernedProviderError::Invalid);
                }
                ActionOutcome::Failed(ActionError {
                    code: "RECONCILED_NO_EFFECT".into(),
                    message: reason,
                    retryable: false,
                    attempts: ordinal + 1,
                })
            }
        })
    }
}

/// Installed by the trusted host for this exact provider binding. No user field
/// chooses a verifier or key. Implementations must prove finality for the whole
/// attempt footprint; verifying a signature without source/attempt correlation
/// is insufficient. This synchronous interface does not supply probe authority.
pub trait ProviderReconciliationVerifier: Send + Sync {
    fn revision(&self) -> &str;
    fn verify(
        &self,
        attempt: &ReconciliationAttempt,
        proof: &[u8],
    ) -> Result<ProviderFinality, GovernedProviderError>;
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedFinalityReceipt {
    schema: u32,
    verifier_revision: String,
    key_id: String,
    attempt: ReconciliationAttempt,
    finality: ProviderFinality,
    signature: String,
}
fn signature_bytes(receipt: &SignedFinalityReceipt) -> Result<Vec<u8>, GovernedProviderError> {
    let value = serde_json::json!({
        "domain":"acteon.provider.finality.v1", "schema":receipt.schema,
        "verifier_revision":receipt.verifier_revision, "key_id":receipt.key_id,
        "attempt":receipt.attempt, "finality":receipt.finality,
    });
    crate::plan::canonical_bytes(&value).map_err(|_| GovernedProviderError::Invalid)
}

/// Local verifier for independently issued HMAC receipts. Use dedicated finality
/// keys, separate from execution credentials/context signing keys. Accepted keys
/// authenticate the external source, whose adapter must qualify attempt mapping
/// and irrevocable finality (including any internal retries or deferred effects).
pub struct HmacFinalityVerifier {
    revision: String,
    keys: BTreeMap<String, Vec<u8>>,
}
impl HmacFinalityVerifier {
    pub fn new_trusted(
        revision: &str,
        keys: BTreeMap<String, Vec<u8>>,
    ) -> Result<Self, GovernedProviderError> {
        if !valid_text(revision)
            || keys.is_empty()
            || keys.len() > 16
            || keys
                .iter()
                .any(|(id, key)| !valid_text(id) || !(32..=1024).contains(&key.len()))
        {
            return Err(GovernedProviderError::Invalid);
        }
        Ok(Self {
            revision: revision.into(),
            keys,
        })
    }
}
impl ProviderReconciliationVerifier for HmacFinalityVerifier {
    fn revision(&self) -> &str {
        &self.revision
    }
    fn verify(
        &self,
        attempt: &ReconciliationAttempt,
        proof: &[u8],
    ) -> Result<ProviderFinality, GovernedProviderError> {
        if proof.is_empty() || proof.len() > MAX_PROOF_BYTES {
            return Err(GovernedProviderError::Invalid);
        }
        let receipt: SignedFinalityReceipt =
            serde_json::from_slice(proof).map_err(|_| GovernedProviderError::Invalid)?;
        if receipt.schema != 1
            || receipt.verifier_revision != self.revision
            || receipt.attempt != *attempt
        {
            return Err(GovernedProviderError::Conflict);
        }
        let key = self
            .keys
            .get(&receipt.key_id)
            .ok_or(GovernedProviderError::Ownership)?;
        let signature = STANDARD
            .decode(&receipt.signature)
            .map_err(|_| GovernedProviderError::Invalid)?;
        let mut mac =
            Hmac::<Sha256>::new_from_slice(key).map_err(|_| GovernedProviderError::Invalid)?;
        mac.update(&signature_bytes(&receipt)?);
        mac.verify_slice(&signature)
            .map_err(|_| GovernedProviderError::Ownership)?;
        Ok(receipt.finality)
    }
}

/// External trusted issuer helper. Signing requires a configured finality key;
/// this function must only be used after independent attempt/finality checks.
/// It does not cause Acteon to trust that key or authorize any effect.
pub fn sign_finality_receipt(
    attempt: ReconciliationAttempt,
    finality: ProviderFinality,
    verifier_revision: &str,
    key_id: &str,
    key: &[u8],
) -> Result<Vec<u8>, GovernedProviderError> {
    if !valid_text(verifier_revision) || !valid_text(key_id) || !(32..=1024).contains(&key.len()) {
        return Err(GovernedProviderError::Invalid);
    }
    let mut receipt = SignedFinalityReceipt {
        schema: 1,
        verifier_revision: verifier_revision.into(),
        key_id: key_id.into(),
        attempt,
        finality,
        signature: String::new(),
    };
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key).map_err(|_| GovernedProviderError::Invalid)?;
    mac.update(&signature_bytes(&receipt)?);
    receipt.signature = STANDARD.encode(mac.finalize().into_bytes());
    let proof = serde_json::to_vec(&receipt).map_err(|_| GovernedProviderError::Invalid)?;
    if proof.len() > MAX_PROOF_BYTES {
        return Err(GovernedProviderError::Invalid);
    }
    Ok(proof)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Resolution {
    pub(super) schema: u32,
    pub(super) attempt: ReconciliationAttempt,
    pub(super) verifier_revision: String,
    pub(super) proof: Vec<u8>,
    pub(super) outcome: ActionOutcome,
    pub(super) resolved_at_ms: i64,
    /// Public management ingress requires fresh evaluated authority to accept
    /// even a valid staged proof. Ordinary receipt observation cannot adopt it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) requires_management_authorization: bool,
}
pub(super) fn resolution_key(context: &ExecutionContextReference, ordinal: u32) -> StateKey {
    StateKey::new(
        context.namespace(),
        context.tenant(),
        KeyKind::Custom(RECONCILIATION_KIND.into()),
        attempt_id(context.execution_id(), ordinal),
    )
}
fn binding_digest(binding: &super::Binding) -> Result<String, GovernedProviderError> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(binding).map_err(|_| GovernedProviderError::Invalid)?)
    ))
}

impl super::BoundProvider {
    /// Retain this immutable identity in trusted host configuration before
    /// retiring a binding. Public request fields cannot qualify an archive.
    pub fn reconciliation_binding_digest(&self) -> Result<String, GovernedProviderError> {
        binding_digest(&self.binding)
    }
}

pub(super) fn descriptor(
    op: &Operation,
    ordinal: u32,
    token: &str,
) -> Result<ReconciliationAttempt, GovernedProviderError> {
    Ok(ReconciliationAttempt {
        context: op.context.clone(),
        action_id: op.action.id.to_string(),
        attempt_id: attempt_id(op.context.execution_id(), ordinal),
        ordinal,
        token: token.into(),
        binding_digest: binding_digest(&op.binding)?,
    })
}
pub(super) fn validate_resolution(
    resolution: &Resolution,
    op: &Operation,
    ordinal: u32,
    start: &StartRecord,
) -> Result<(), GovernedProviderError> {
    if resolution.schema != 1
        || resolution.attempt != descriptor(op, ordinal, &start.token)?
        || !valid_text(&resolution.verifier_revision)
        || resolution.resolved_at_ms < 0
        || (resolution.requires_management_authorization
            && start.reconciliation.is_some()
            && start.reconciliation_acceptance.is_none())
        || resolution.proof.is_empty()
        || resolution.proof.len() > MAX_PROOF_BYTES
        || !matches!(
            resolution.outcome,
            ActionOutcome::Executed(_) | ActionOutcome::Failed(_)
        )
    {
        return Err(GovernedProviderError::Conflict);
    }
    Ok(())
}
fn same_outcome(a: &ActionOutcome, b: &ActionOutcome) -> Result<bool, GovernedProviderError> {
    Ok(
        serde_json::to_value(a).map_err(|_| GovernedProviderError::Invalid)?
            == serde_json::to_value(b).map_err(|_| GovernedProviderError::Invalid)?,
    )
}
impl Runtime {
    async fn load_resolution(
        &self,
        op: &Operation,
        ordinal: u32,
        start: &StartRecord,
    ) -> Result<Option<(Resolution, AttemptEvidenceReference)>, GovernedProviderError> {
        let key = resolution_key(&op.context, ordinal);
        let Some(raw) = self
            .state
            .get(&key)
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?
        else {
            if start.reconciliation.is_some() {
                return Err(GovernedProviderError::Unavailable);
            }
            return Ok(None);
        };
        let (resolution, digest): (Resolution, _) = self.decode(&raw)?;
        validate_resolution(&resolution, op, ordinal, start)?;
        Ok(Some((
            resolution,
            AttemptEvidenceReference {
                id: key.id.clone(),
                digest,
            },
        )))
    }

    pub(super) async fn observe_resolution(
        &self,
        op: &Operation,
        ordinal: u32,
        start: &StartRecord,
        pending_evidence: Option<&AttemptEvidenceReference>,
    ) -> Result<Option<ActionOutcome>, GovernedProviderError> {
        let Some((resolution, reference)) = self.load_resolution(op, ordinal, start).await? else {
            return Ok(None);
        };
        if let Some(pinned) = &start.reconciliation {
            if pinned.resolution != reference {
                return Err(GovernedProviderError::Conflict);
            }
            return Ok(Some(resolution.outcome));
        }
        // A staged management proof is inert until an evaluated settlement CAS.
        // Do not repair original evidence as a side effect of this candidate.
        if resolution.requires_management_authorization || start.status == AttemptStatus::Settled {
            return Ok(None);
        }
        let mut current = start.clone();
        if let Some(original) = pending_evidence {
            if current.evidence.as_ref().is_some_and(|e| e != original) {
                return Err(GovernedProviderError::Conflict);
            }
            if current.evidence.is_none() {
                // Preserve an uncertain receipt saved before its ledger ack.
                // The attestation does not replace that interrupted evidence.
                self.coordinator
                    .settle_with_evidence(
                        &resolution.attempt.attempt_id,
                        &current.token,
                        AttemptStatus::Uncertain,
                        original.clone(),
                    )
                    .await
                    .map_err(|_| GovernedProviderError::Unavailable)?;
                current.evidence = Some(original.clone());
                current.status = AttemptStatus::Uncertain;
            }
        }
        let Some(verifier) = &self.reconciler else {
            return Ok(None);
        };
        if verifier.revision() != resolution.verifier_revision {
            return Ok(None);
        }
        let final_outcome = verifier
            .verify(&resolution.attempt, &resolution.proof)?
            .outcome(ordinal)?;
        if !same_outcome(&final_outcome, &resolution.outcome)? {
            return Err(GovernedProviderError::Conflict);
        }
        self.coordinator
            .reconcile_attempt(
                &resolution.attempt.attempt_id,
                &start.token,
                AttemptReconciliationReference {
                    prior_status: current.status,
                    original_evidence: current.evidence.clone(),
                    resolution: reference,
                },
            )
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?;
        Ok(Some(final_outcome))
    }
}
/// Accepted audit projection. The original receipt link remains available;
/// this exposes only verified, digest-pinned finality, never an uncommitted claim.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderReconciliationRecord {
    pub prior_status: AttemptStatus,
    pub execution_id: Uuid,
    pub attempt_id: String,
    pub original_evidence: Option<AttemptEvidenceReference>,
    pub resolution: AttemptEvidenceReference,
    pub verifier_revision: String,
    pub proof_digest: String,
    pub resolved_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<acteon_governance::reconciliation::ReconciliationAcceptance>,
    pub outcome: ActionOutcome,
}

impl GovernedProviderExecutor {
    /// Install before execution; actual provider/issuer correlation is reviewed
    /// by the host. Public request fields cannot install or select a verifier.
    pub fn with_trusted_reconciliation_verifier(
        mut self,
        verifier: Arc<dyn ProviderReconciliationVerifier>,
    ) -> Result<Self, GovernedProviderError> {
        if !valid_text(verifier.revision()) {
            return Err(GovernedProviderError::Invalid);
        }
        Arc::get_mut(&mut self.runtime)
            .ok_or(GovernedProviderError::Conflict)?
            .reconciler = Some(verifier);
        Ok(self)
    }
    /// Observe the accepted reconciliation history for owned work. Retained
    /// context and provider qualification are checked just as for receipt reads.
    pub async fn reconciliation_record(
        &self,
        context: &ExecutionContextReference,
        actor: &PrincipalIdentity,
    ) -> Result<Option<ProviderReconciliationRecord>, GovernedProviderError> {
        let op = self
            .runtime
            .load_operation(context, actor)
            .await?
            .ok_or(GovernedProviderError::Ownership)?;
        let receipt = self.runtime.observe(&op).await?;
        let Some(ordinal) = receipt.attempts.checked_sub(1) else {
            return Ok(None);
        };
        let snapshot = self
            .runtime
            .coordinator
            .snapshot()
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?;
        let id = attempt_id(context.execution_id(), ordinal);
        let start = snapshot
            .starts
            .get(&id)
            .ok_or(GovernedProviderError::Conflict)?;
        let Some(pinned) = &start.reconciliation else {
            return Ok(None);
        };
        let (stored, reference) = self
            .runtime
            .load_resolution(&op, ordinal, start)
            .await?
            .ok_or(GovernedProviderError::Unavailable)?;
        if pinned.resolution != reference {
            return Err(GovernedProviderError::Conflict);
        }
        Ok(Some(ProviderReconciliationRecord {
            prior_status: pinned.prior_status,
            execution_id: context.execution_id(),
            attempt_id: id,
            original_evidence: pinned.original_evidence.clone(),
            resolution: reference,
            verifier_revision: stored.verifier_revision,
            proof_digest: format!("{:x}", Sha256::digest(&stored.proof)),
            resolved_at_ms: stored.resolved_at_ms,
            acceptance: start.reconciliation_acceptance.clone(),
            outcome: stored.outcome,
        }))
    }

    /// Obtain correlation material only for owned, unresolved registered work.
    /// Deliver it through a qualified provider adapter; labels do not prove that
    /// an external service executed this exact attempt.
    pub async fn reconciliation_attempt(
        &self,
        context: &ExecutionContextReference,
        actor: &PrincipalIdentity,
    ) -> Result<Option<ReconciliationAttempt>, GovernedProviderError> {
        let op = self
            .runtime
            .load_operation(context, actor)
            .await?
            .ok_or(GovernedProviderError::Ownership)?;
        let receipt = self.runtime.observe(&op).await?;
        if !matches!(
            receipt.status,
            GovernedProviderStatus::InFlight { .. }
                | GovernedProviderStatus::ReconciliationRequired { .. }
        ) {
            return Ok(None);
        }
        let ordinal = receipt
            .attempts
            .checked_sub(1)
            .ok_or(GovernedProviderError::Conflict)?;
        let snapshot = self
            .runtime
            .coordinator
            .snapshot()
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?;
        let start = snapshot
            .starts
            .get(&attempt_id(context.execution_id(), ordinal))
            .ok_or(GovernedProviderError::Conflict)?;
        Ok(Some(descriptor(&op, ordinal, &start.token)?))
    }
    /// Read correlation for the latest unresolved attempt under independent
    /// management authorization. This never accepts staged proof or repairs an
    /// interrupted original result acknowledgment.
    pub async fn reconciliation_attempt_evaluated(
        &self,
        context: &ExecutionContextReference,
        owner: &PrincipalIdentity,
        ordinal: u32,
        authorization: ReconciliationAuthorization<'_>,
    ) -> Result<ReconciliationAttempt, GovernedProviderError> {
        self.management_store()?
            .reconciliation_attempt_evaluated(context, owner, ordinal, authorization)
            .await
    }

    /// Accept qualified finality under freshly evaluated management authority.
    /// Uses the same guarded store as retired-binding reconciliation.
    pub async fn reconcile_evaluated(
        &self,
        context: &ExecutionContextReference,
        owner: &PrincipalIdentity,
        ordinal: u32,
        proof: &[u8],
        authorization: ReconciliationAuthorization<'_>,
    ) -> Result<GovernedProviderReceipt, GovernedProviderError> {
        self.management_store()?
            .reconcile_evaluated(context, owner, ordinal, proof, authorization)
            .await
    }

    fn management_store(&self) -> Result<ProviderReconciliationStore, GovernedProviderError> {
        Ok(ProviderReconciliationStore {
            state: self.runtime.state.clone(),
            coordinator: self.runtime.coordinator.clone(),
            contexts: self.runtime.contexts.clone(),
            clock: self.runtime.clock.clone(),
            encryptor: self.runtime.encryptor.clone(),
            bindings: BTreeMap::from([(
                binding_digest(&self.runtime.bound.binding)?,
                self.runtime.reconciler.clone(),
            )]),
            current: Some((
                self.runtime.bound.binding.clone(),
                self.runtime.settings.clone(),
            )),
        })
    }

    /// Verify and persist independent finality for existing work. Never invokes
    /// a provider, borrows expired permissions, retries, or changes original
    /// evidence. Lost acknowledgements repair from the exact retained proof.
    pub async fn reconcile(
        &self,
        context: &ExecutionContextReference,
        actor: &PrincipalIdentity,
        proof: &[u8],
    ) -> Result<GovernedProviderReceipt, GovernedProviderError> {
        if proof.is_empty() || proof.len() > MAX_PROOF_BYTES {
            return Err(GovernedProviderError::Invalid);
        }
        let op = self
            .runtime
            .load_operation(context, actor)
            .await?
            .ok_or(GovernedProviderError::Ownership)?;
        let receipt = self.runtime.observe(&op).await?;
        let ordinal = receipt
            .attempts
            .checked_sub(1)
            .ok_or(GovernedProviderError::Conflict)?;
        let snapshot = self
            .runtime
            .coordinator
            .snapshot()
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?;
        let start = snapshot
            .starts
            .get(&attempt_id(context.execution_id(), ordinal))
            .ok_or(GovernedProviderError::Conflict)?;
        if let Some((stored, _)) = self.runtime.load_resolution(&op, ordinal, start).await? {
            if stored.proof != proof {
                return Err(GovernedProviderError::Conflict);
            }
            let observed = self.runtime.observe(&op).await?;
            let snapshot = self
                .runtime
                .coordinator
                .snapshot()
                .await
                .map_err(|_| GovernedProviderError::Unavailable)?;
            if snapshot
                .starts
                .get(&stored.attempt.attempt_id)
                .is_none_or(|start| start.reconciliation.is_none())
            {
                return Err(GovernedProviderError::Conflict);
            }
            return Ok(observed);
        }
        if !matches!(
            receipt.status,
            GovernedProviderStatus::InFlight { .. }
                | GovernedProviderStatus::ReconciliationRequired { .. }
        ) {
            return Err(GovernedProviderError::Conflict);
        }
        let verifier = self
            .runtime
            .reconciler
            .as_ref()
            .ok_or(GovernedProviderError::Admission(
                "reconciliation verifier required",
            ))?;
        let attempt = descriptor(&op, ordinal, &start.token)?;
        let outcome = verifier.verify(&attempt, proof)?.outcome(ordinal)?;
        let resolution = Resolution {
            schema: 1,
            attempt,
            verifier_revision: verifier.revision().into(),
            proof: proof.into(),
            outcome,
            resolved_at_ms: self.runtime.clock.now().timestamp_millis(),
            requires_management_authorization: false,
        };
        self.runtime
            .state
            .check_and_set(
                &resolution_key(context, ordinal),
                &self.runtime.encode(&resolution)?,
                None,
            )
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?;
        let receipt = self.runtime.observe(&op).await?;
        // A competing request must not turn this proof into an acknowledgement
        // for a different finality statement.
        let snapshot = self
            .runtime
            .coordinator
            .snapshot()
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?;
        let start = snapshot
            .starts
            .get(&attempt_id(context.execution_id(), ordinal))
            .ok_or(GovernedProviderError::Conflict)?;
        let (stored, _) = self
            .runtime
            .load_resolution(&op, ordinal, start)
            .await?
            .ok_or(GovernedProviderError::Unavailable)?;
        if stored.proof != proof || start.reconciliation.is_none() {
            return Err(GovernedProviderError::Conflict);
        }
        Ok(receipt)
    }
}

fn reconciliation_authority_error(error: &CoordinationError) -> GovernedProviderError {
    match error {
        CoordinationError::State(_) | CoordinationError::Contention => {
            GovernedProviderError::Unavailable
        }
        CoordinationError::StaleAuthority | CoordinationError::Conflict => {
            GovernedProviderError::Conflict
        }
        CoordinationError::Invalid(_) => GovernedProviderError::Invalid,
        _ => GovernedProviderError::Admission("reconciliation authorization refused"),
    }
}
