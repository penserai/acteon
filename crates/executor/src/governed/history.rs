//! Read-only provider history independent of installed routes and retry settings.
//!
//! The host establishes ownership. A public context reference is correlation
//! material, not permission. This reader accepts only ledger-pinned evidence and
//! never invokes a provider, repairs state or accepts a reconciliation candidate.

use std::sync::Arc;

use acteon_core::{ActionOutcome, ExecutionContextReference, PrincipalIdentity};
use acteon_crypto::PayloadEncryptor;
use acteon_governance::context::{AcceptedEffect, TrustedContextStore, VerifiedExecutionContext};
use acteon_governance::permit::{permit_revision_tag, permitted_attempt_digest};
use acteon_governance::{
    AttemptEvidenceReference, AttemptStatus, AuthorityCoordinator, AuthorityStamp,
    CoordinatorSnapshot, MAX_BUDGET_DEPTH, StartRecord,
};
use acteon_state::{KeyKind, StateKey, StateStore};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::reconciliation::{
    ProviderReconciliationRecord, Resolution, resolution_key, validate_resolution,
};
use super::{
    Evidence, GovernedProviderError, GovernedProviderReceipt, GovernedProviderStatus, MAX_ATTEMPTS,
    OPERATION_KIND, Operation, attempt_id, decode_record, governed_provider_input_digest,
    operation_key, result_key, validate_evidence,
};

/// Distinguishes a complete start-time seal from older, narrower evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationIntegrity {
    /// No registered attempt authenticates the complete stored envelope yet.
    Unstarted,
    /// Every registered attempt pins the same complete original envelope.
    Sealed,
    /// At least one legacy attempt lacks a complete operation seal.
    Legacy,
}

/// Binding metadata authenticated by an operation seal or pinned result/finality.
/// This describes the historical effect, without constructing a live provider.
#[derive(Debug, Clone, Serialize)]
pub struct HistoricalProviderBinding {
    pub provider: String,
    pub provider_revision: String,
    pub failure_revision: String,
    pub effect: AcceptedEffect,
}

/// Metadata is exposed only when every retained start seals the whole operation.
#[derive(Debug, Clone, Serialize)]
pub struct HistoricalOperationMetadata {
    pub original_action_id: String,
    pub max_attempts: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoricalProviderAttempt {
    pub attempt_id: String,
    pub ordinal: u32,
    pub ledger_status: AttemptStatus,
    pub original_evidence: Option<AttemptEvidenceReference>,
    pub original_outcome: Option<ActionOutcome>,
    pub reconciliation: Option<ProviderReconciliationRecord>,
}

/// A verified historical projection, without an execution or settlement grant.
#[derive(Debug, Clone, Serialize)]
pub struct HistoricalProviderReceipt {
    /// Participant authenticated by the retained signed context.
    pub subject: PrincipalIdentity,
    pub receipt: GovernedProviderReceipt,
    /// Authority generation observed with this snapshot; it is not an admission grant.
    pub observed_authority: AuthorityStamp,
    pub operation_integrity: OperationIntegrity,
    pub metadata: Option<HistoricalOperationMetadata>,
    pub binding: Option<HistoricalProviderBinding>,
    pub cancellation_fenced: bool,
    pub attempts: Vec<HistoricalProviderAttempt>,
}

/// Host-installed store. Use the execution scope's configured state backend and
/// retained context keys. It deliberately has no provider, executor, clock or
/// reconciliation verifier. The permitted execution subject comes from trusted
/// host authentication and policy; request actor labels cannot authorize a read.
pub struct HistoricalProviderStore {
    state: Arc<dyn StateStore>,
    coordinator: AuthorityCoordinator,
    contexts: Arc<TrustedContextStore>,
    encryptor: Option<Arc<PayloadEncryptor>>,
}
impl HistoricalProviderStore {
    #[must_use]
    pub fn new(
        state: Arc<dyn StateStore>,
        coordinator: AuthorityCoordinator,
        contexts: Arc<TrustedContextStore>,
        encryptor: Option<Arc<PayloadEncryptor>>,
    ) -> Self {
        Self {
            state,
            coordinator,
            contexts,
            encryptor,
        }
    }

    /// Locate owned work using the public execution ID. The retained context is
    /// still verified; an ID alone cannot authorize this lookup.
    pub async fn inspect_execution(
        &self,
        execution_id: Uuid,
        owner: &PrincipalIdentity,
    ) -> Result<Option<HistoricalProviderReceipt>, GovernedProviderError> {
        let snapshot = self
            .coordinator
            .snapshot()
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?;
        let key = StateKey::new(
            snapshot.namespace.as_str(),
            snapshot.tenant.as_str(),
            KeyKind::Custom(OPERATION_KIND.into()),
            execution_id.to_string(),
        );
        let Some(raw) = self
            .state
            .get(&key)
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?
        else {
            if has_registered(&snapshot, execution_id) {
                return Err(GovernedProviderError::Unavailable);
            }
            return Ok(None);
        };
        let (op, _): (Operation, _) = decode_record(&raw, self.encryptor.as_deref())?;
        if op.context.execution_id() != execution_id {
            return Err(GovernedProviderError::Conflict);
        }
        self.inspect(&op.context, owner).await
    }

    pub async fn inspect(
        &self,
        reference: &ExecutionContextReference,
        owner: &PrincipalIdentity,
    ) -> Result<Option<HistoricalProviderReceipt>, GovernedProviderError> {
        if reference.principal() != owner {
            return Err(GovernedProviderError::Ownership);
        }
        let snapshot = self
            .coordinator
            .snapshot()
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?;
        if snapshot.namespace != reference.namespace() || snapshot.tenant != reference.tenant() {
            return Err(GovernedProviderError::Ownership);
        }
        let context = self
            .contexts
            .recover_reference_for_observation(reference)
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?;
        if context.authority_stamp().incarnation != snapshot.incarnation {
            return Err(GovernedProviderError::Conflict);
        }
        let Some(raw) = self
            .state
            .get(&operation_key(reference))
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?
        else {
            if has_registered(&snapshot, reference.execution_id()) {
                return Err(GovernedProviderError::Unavailable);
            }
            return Ok(None);
        };
        let (op, digest): (Operation, _) = decode_record(&raw, self.encryptor.as_deref())?;
        validate_operation(&op, reference, &context)?;
        self.project(&op, &digest, &context, &snapshot)
            .await
            .map(Some)
    }

    async fn pinned<T: serde::de::DeserializeOwned>(
        &self,
        key: &StateKey,
        pin: &AttemptEvidenceReference,
    ) -> Result<T, GovernedProviderError> {
        if pin.id != key.id {
            return Err(GovernedProviderError::Conflict);
        }
        let raw = self
            .state
            .get(key)
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?
            .ok_or(GovernedProviderError::Unavailable)?;
        let (record, digest) = decode_record(&raw, self.encryptor.as_deref())?;
        if digest != pin.digest {
            return Err(GovernedProviderError::Conflict);
        }
        Ok(record)
    }

    async fn original(
        &self,
        op: &Operation,
        ordinal: u32,
        start: &StartRecord,
    ) -> Result<Option<Evidence>, GovernedProviderError> {
        let Some(pin) = &start.evidence else {
            return Ok(None);
        };
        let e: Evidence = self.pinned(&result_key(&op.context, ordinal), pin).await?;
        validate_evidence(&e, op, ordinal, &start.token)?;
        Ok(Some(e))
    }

    async fn resolution(
        &self,
        op: &Operation,
        ordinal: u32,
        start: &StartRecord,
    ) -> Result<Option<ProviderReconciliationRecord>, GovernedProviderError> {
        let Some(link) = &start.reconciliation else {
            return Ok(None);
        };
        let r: Resolution = self
            .pinned(&resolution_key(&op.context, ordinal), &link.resolution)
            .await?;
        validate_resolution(&r, op, ordinal, start)?;
        Ok(Some(ProviderReconciliationRecord {
            prior_status: link.prior_status,
            execution_id: op.context.execution_id(),
            attempt_id: attempt_id(op.context.execution_id(), ordinal),
            original_evidence: link.original_evidence.clone(),
            resolution: link.resolution.clone(),
            verifier_revision: r.verifier_revision,
            proof_digest: format!("{:x}", Sha256::digest(&r.proof)),
            resolved_at_ms: r.resolved_at_ms,
            acceptance: start.reconciliation_acceptance.clone(),
            outcome: r.outcome,
        }))
    }

    async fn project(
        &self,
        op: &Operation,
        digest: &str,
        context: &VerifiedExecutionContext,
        snapshot: &CoordinatorSnapshot,
    ) -> Result<HistoricalProviderReceipt, GovernedProviderError> {
        let mut result = HistoricalProviderReceipt {
            subject: context.principal().clone(),
            observed_authority: snapshot.stamp(),
            receipt: GovernedProviderReceipt {
                execution_id: op.context.execution_id(),
                attempts: 0,
                status: GovernedProviderStatus::Prepared,
            },
            operation_integrity: OperationIntegrity::Unstarted,
            metadata: None,
            binding: None,
            cancellation_fenced: cancellation_fenced(
                snapshot,
                &op.context.execution_id().to_string(),
            )?,
            attempts: Vec::new(),
        };
        let mut gap = false;
        let mut all_sealed = true;
        // Do not let an unsealed legacy retry setting hide later registered work.
        for ordinal in 0..MAX_ATTEMPTS {
            let id = attempt_id(op.context.execution_id(), ordinal);
            let Some(start) = snapshot.starts.get(&id) else {
                gap = true;
                continue;
            };
            if gap
                || (ordinal > 0
                    && !matches!(
                        result.receipt.status,
                        GovernedProviderStatus::AwaitingRetry { .. }
                    ))
            {
                return Err(GovernedProviderError::Conflict);
            }
            validate_start(op, digest, context, start)?;
            all_sealed &= start.operation_evidence.is_some();
            if start.operation_evidence.is_some() && ordinal >= op.settings.max_attempts {
                return Err(GovernedProviderError::Conflict);
            }
            let original = self.original(op, ordinal, start).await?;
            let reconciliation = self.resolution(op, ordinal, start).await?;
            if start.operation_evidence.is_some() || original.is_some() || reconciliation.is_some()
            {
                result.binding = Some(HistoricalProviderBinding {
                    provider: op.binding.provider.clone(),
                    provider_revision: op.binding.revision.clone(),
                    failure_revision: op.binding.failure_revision.clone(),
                    effect: op.binding.effect.clone(),
                });
            }
            result.receipt.status =
                historical_status(start, original.as_ref(), reconciliation.as_ref(), &id)?;
            result.receipt.attempts = ordinal + 1;
            result.attempts.push(HistoricalProviderAttempt {
                attempt_id: id,
                ordinal,
                ledger_status: start.status,
                original_evidence: start.evidence.clone(),
                original_outcome: original.map(|e| e.outcome),
                reconciliation,
            });
        }
        if !result.attempts.is_empty() {
            result.operation_integrity = if all_sealed {
                OperationIntegrity::Sealed
            } else {
                OperationIntegrity::Legacy
            };
            if all_sealed {
                result.metadata = Some(HistoricalOperationMetadata {
                    original_action_id: op.action.id.to_string(),
                    max_attempts: op.settings.max_attempts,
                });
            }
        }
        Ok(result)
    }
}

pub(super) fn validate_operation(
    op: &Operation,
    reference: &ExecutionContextReference,
    context: &VerifiedExecutionContext,
) -> Result<(), GovernedProviderError> {
    if op.schema != 1
        || op.context != *reference
        || op.action.namespace.as_str() != reference.namespace()
        || op.action.tenant.as_str() != reference.tenant()
        || governed_provider_input_digest(&op.action)? != reference.request_digest()
        || permit_revision_tag(&op.permits).map_err(|_| GovernedProviderError::Invalid)?
            != context.accepted_ceiling_revision()
        || context
            .effect_registration_resources(&op.binding.effect)
            .is_err()
        || op.settings.max_attempts == 0
        || op.settings.max_attempts > MAX_ATTEMPTS
        || op.settings.delays_ns.len() != (op.settings.max_attempts - 1) as usize
        || op.settings.timeout_ns == 0
    {
        return Err(GovernedProviderError::Conflict);
    }
    Ok(())
}
pub(super) fn validate_start(
    op: &Operation,
    digest: &str,
    context: &VerifiedExecutionContext,
    start: &StartRecord,
) -> Result<(), GovernedProviderError> {
    let resources = context
        .effect_registration_resources(&op.binding.effect)
        .map_err(|_| GovernedProviderError::Conflict)?;
    if start.request_digest
        != permitted_attempt_digest(&op.context, &op.binding.effect, 1, &op.permits)
            .map_err(|_| GovernedProviderError::Conflict)?
        || start.subject != op.context.principal().id()
        || start.resources.iter().ne(resources.iter())
        || start.authority.incarnation != context.authority_stamp().incarnation
        || start
            .reservation
            .as_ref()
            .is_none_or(|r| r.root_id != op.context.execution_id().to_string() || r.units != 1)
        || start.operation_evidence.as_ref().is_some_and(|seal| {
            seal.id != op.context.execution_id().to_string() || seal.digest != digest
        })
    {
        return Err(GovernedProviderError::Conflict);
    }
    Ok(())
}
fn historical_status(
    start: &StartRecord,
    original: Option<&Evidence>,
    reconciliation: Option<&ProviderReconciliationRecord>,
    id: &str,
) -> Result<GovernedProviderStatus, GovernedProviderError> {
    if let Some(r) = reconciliation {
        return Ok(GovernedProviderStatus::Completed {
            outcome: r.outcome.clone(),
        });
    }
    match start.status {
        AttemptStatus::InFlight if original.is_none() => Ok(GovernedProviderStatus::InFlight {
            attempt_id: id.into(),
        }),
        AttemptStatus::Uncertain if original.is_none_or(|e| !e.known) => {
            Ok(GovernedProviderStatus::ReconciliationRequired {
                attempt_id: id.into(),
            })
        }
        AttemptStatus::Settled => {
            let e = original
                .filter(|e| e.known)
                .ok_or(GovernedProviderError::Unavailable)?;
            Ok(e.retry_at_ms.map_or_else(
                || GovernedProviderStatus::Completed {
                    outcome: e.outcome.clone(),
                },
                |not_before_ms| GovernedProviderStatus::AwaitingRetry { not_before_ms },
            ))
        }
        _ => Err(GovernedProviderError::Conflict),
    }
}
fn cancellation_fenced(
    snapshot: &CoordinatorSnapshot,
    execution_id: &str,
) -> Result<bool, GovernedProviderError> {
    let mut current = execution_id;
    let mut fenced = false;
    for _ in 0..MAX_BUDGET_DEPTH {
        fenced |= snapshot
            .roots
            .get(current)
            .ok_or(GovernedProviderError::Unavailable)?
            .cancelled;
        let Some(parent) = snapshot.budget_parents.get(current) else {
            return Ok(fenced);
        };
        current = parent;
    }
    Err(GovernedProviderError::Conflict)
}

fn has_registered(snapshot: &CoordinatorSnapshot, execution_id: Uuid) -> bool {
    (0..MAX_ATTEMPTS).any(|ordinal| {
        snapshot
            .starts
            .contains_key(&attempt_id(execution_id, ordinal))
    })
}
