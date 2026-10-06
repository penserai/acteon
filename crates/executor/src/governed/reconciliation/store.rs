//! Management-only reconciliation of retained work, including retired bindings.
use std::{collections::BTreeMap, sync::Arc};

use acteon_core::{ActionOutcome, ExecutionContextReference, PrincipalIdentity};
use acteon_crypto::PayloadEncryptor;
use acteon_governance::{
    AttemptEvidenceReference, AttemptReconciliationReference, AttemptStatus, AuthorityCoordinator,
    StartRecord, context::TrustedContextStore, reconciliation::ReconciliationAuthorization,
};
use acteon_state::StateStore;
use acteon_time::Clock;
use serde::Serialize;

use super::super::{
    Binding, Evidence, GovernedProviderError, GovernedProviderReceipt, GovernedProviderStatus,
    MAX_ATTEMPTS, MAX_BYTES, Operation, Settings, attempt_id, decode_record,
    history::{HistoricalProviderStore, OperationIntegrity},
    operation_key, result_key, validate_evidence,
};
use super::{
    MAX_PROOF_BYTES, ProviderReconciliationVerifier, ReconciliationAttempt, Resolution,
    binding_digest, descriptor, reconciliation_authority_error, resolution_key, same_outcome,
    validate_resolution,
};

/// Trusted host installation for exact immutable binding digests. The host must
/// qualify each verifier against the original external source and full attempt
/// footprint. Neither public requests nor registry cards can populate this map.
///
/// This store has no provider or dispatch route. Archived installations need no
/// current executor settings. Retired work
/// must carry a complete start-time operation seal. Removing a qualification
/// prevents new acceptance; accepted history remains independently readable.
/// Replacing a verifier must retain its revision while resuming a staged proof,
/// or staging fails closed. Accepted exact-proof replay does not reverify with a
/// new key: it preserves the original acceptance under current operator authority.
pub struct ProviderReconciliationStore {
    pub(super) state: Arc<dyn StateStore>,
    pub(super) coordinator: AuthorityCoordinator,
    pub(super) contexts: Arc<TrustedContextStore>,
    pub(super) clock: Arc<dyn Clock>,
    pub(super) encryptor: Option<Arc<PayloadEncryptor>>,
    pub(super) bindings: BTreeMap<String, Option<Arc<dyn ProviderReconciliationVerifier>>>,
    pub(super) current: Option<(Binding, Settings)>,
}
impl ProviderReconciliationStore {
    pub fn new_trusted(
        state: Arc<dyn StateStore>,
        coordinator: AuthorityCoordinator,
        contexts: Arc<TrustedContextStore>,
        clock: Arc<dyn Clock>,
        encryptor: Option<Arc<PayloadEncryptor>>,
        bindings: BTreeMap<String, Arc<dyn ProviderReconciliationVerifier>>,
    ) -> Result<Self, GovernedProviderError> {
        if bindings.is_empty()
            || bindings.len() > 128
            || bindings.iter().any(|(digest, verifier)| {
                digest.len() != 64
                    || !digest
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    || !super::valid_text(verifier.revision())
            })
        {
            return Err(GovernedProviderError::Invalid);
        }
        Ok(Self {
            state,
            coordinator,
            contexts,
            clock,
            encryptor,
            bindings: bindings.into_iter().map(|(k, v)| (k, Some(v))).collect(),
            current: None,
        })
    }

    /// Read an owned retained reference for management lookup. The subsequent
    /// evaluated correlation/acceptance still enforces seals and qualification.
    pub async fn owned_execution_reference(
        &self,
        execution_id: uuid::Uuid,
        owner: &PrincipalIdentity,
    ) -> Result<Option<ExecutionContextReference>, GovernedProviderError> {
        let snapshot = self
            .coordinator
            .snapshot()
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?;
        let key = acteon_state::StateKey::new(
            snapshot.namespace.as_str(),
            snapshot.tenant.as_str(),
            acteon_state::KeyKind::Custom(super::super::OPERATION_KIND.into()),
            execution_id.to_string(),
        );
        let Some(raw) = self
            .state
            .get(&key)
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?
        else {
            return if (0..MAX_ATTEMPTS).any(|ordinal| {
                snapshot
                    .starts
                    .contains_key(&attempt_id(execution_id, ordinal))
            }) {
                Err(GovernedProviderError::Unavailable)
            } else {
                Ok(None)
            };
        };
        let (op, _): (Operation, _) = decode_record(&raw, self.encryptor.as_deref())?;
        if op.context.execution_id() != execution_id {
            return Err(GovernedProviderError::Conflict);
        }
        if op.context.principal() != owner {
            return Err(GovernedProviderError::Ownership);
        }
        HistoricalProviderStore::new(
            self.state.clone(),
            self.coordinator.clone(),
            self.contexts.clone(),
            self.encryptor.clone(),
        )
        .inspect(&op.context, owner)
        .await?
        .ok_or(GovernedProviderError::Ownership)?;
        Ok(Some(op.context))
    }

    async fn management_attempt(
        &self,
        context: &ExecutionContextReference,
        owner: &PrincipalIdentity,
        ordinal: u32,
        authorization: &ReconciliationAuthorization<'_>,
    ) -> Result<(Operation, StartRecord), GovernedProviderError> {
        let id = attempt_id(context.execution_id(), ordinal);
        self.coordinator
            .check_reconciliation_authorization(&id, authorization)
            .await
            .map_err(|error| reconciliation_authority_error(&error))?;
        let history = HistoricalProviderStore::new(
            self.state.clone(),
            self.coordinator.clone(),
            self.contexts.clone(),
            self.encryptor.clone(),
        )
        .inspect(context, owner)
        .await?
        .ok_or(GovernedProviderError::Ownership)?;
        if history.receipt.attempts.checked_sub(1) != Some(ordinal)
            || (self.current.is_none() && history.operation_integrity != OperationIntegrity::Sealed)
        {
            return Err(GovernedProviderError::Conflict);
        }
        let raw = self
            .state
            .get(&operation_key(context))
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?
            .ok_or(GovernedProviderError::Unavailable)?;
        let (op, digest): (Operation, _) = decode_record(&raw, self.encryptor.as_deref())?;
        if op.context != *context
            || !self.bindings.contains_key(&binding_digest(&op.binding)?)
            || self.current.as_ref().is_some_and(|(binding, settings)| {
                op.binding != *binding || op.settings != *settings
            })
        {
            return Err(GovernedProviderError::Conflict);
        }
        let snapshot = self
            .coordinator
            .snapshot()
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?;
        // Authenticate the second operation read and current attempt set rather
        // than treating an earlier history projection as a write authorization.
        let recovered = self
            .contexts
            .recover_reference_for_observation(context)
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?;
        super::super::history::validate_operation(&op, context, &recovered)?;
        let mut latest = None;
        let mut gap = false;
        for retained_ordinal in 0..MAX_ATTEMPTS {
            let retained_id = attempt_id(context.execution_id(), retained_ordinal);
            let Some(retained) = snapshot.starts.get(&retained_id) else {
                gap = true;
                continue;
            };
            if gap || (self.current.is_none() && retained.operation_evidence.is_none()) {
                return Err(GovernedProviderError::Conflict);
            }
            super::super::history::validate_start(&op, &digest, &recovered, retained)?;
            latest = Some(retained_ordinal);
        }
        if latest != Some(ordinal) {
            return Err(GovernedProviderError::Conflict);
        }
        let start = snapshot
            .starts
            .get(&id)
            .ok_or(GovernedProviderError::Conflict)?
            .clone();
        self.coordinator
            .check_reconciliation_authorization(&id, authorization)
            .await
            .map_err(|error| reconciliation_authority_error(&error))?;
        Ok((op, start))
    }

    fn encode<T: Serialize>(&self, value: &T) -> Result<String, GovernedProviderError> {
        let raw = serde_json::to_string(value).map_err(|_| GovernedProviderError::Invalid)?;
        if raw.len() > MAX_BYTES {
            return Err(GovernedProviderError::Invalid);
        }
        match &self.encryptor {
            Some(e) => e
                .encrypt_str(&raw)
                .map_err(|_| GovernedProviderError::Unavailable),
            None => Ok(raw),
        }
    }

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
            return if start.reconciliation.is_some() {
                Err(GovernedProviderError::Unavailable)
            } else {
                Ok(None)
            };
        };
        let (resolution, digest): (Resolution, _) = decode_record(&raw, self.encryptor.as_deref())?;
        validate_resolution(&resolution, op, ordinal, start)?;
        Ok(Some((
            resolution,
            AttemptEvidenceReference { id: key.id, digest },
        )))
    }

    async fn load_evidence(
        &self,
        op: &Operation,
        ordinal: u32,
        token: &str,
    ) -> Result<Option<(Evidence, AttemptEvidenceReference)>, GovernedProviderError> {
        let key = result_key(&op.context, ordinal);
        let Some(raw) = self
            .state
            .get(&key)
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?
        else {
            return Ok(None);
        };
        let (evidence, digest): (Evidence, _) = decode_record(&raw, self.encryptor.as_deref())?;
        validate_evidence(&evidence, op, ordinal, token)?;
        Ok(Some((
            evidence,
            AttemptEvidenceReference { id: key.id, digest },
        )))
    }
    async fn stage_management_resolution(
        &self,
        op: &Operation,
        ordinal: u32,
        start: &StartRecord,
        proof: &[u8],
        authorization: &ReconciliationAuthorization<'_>,
    ) -> Result<(ActionOutcome, AttemptEvidenceReference), GovernedProviderError> {
        let existing = self.load_resolution(op, ordinal, start).await?;
        let verifier = self
            .bindings
            .get(&binding_digest(&op.binding)?)
            .and_then(Option::as_ref)
            .ok_or(GovernedProviderError::Admission(
                "reconciliation verifier required",
            ))?;
        let attempt = descriptor(op, ordinal, &start.token)?;
        let outcome = verifier.verify(&attempt, proof)?.outcome(ordinal)?;
        if let Some((stored, _)) = existing {
            if stored.proof != proof
                || stored.verifier_revision != verifier.revision()
                || !stored.requires_management_authorization
                || !same_outcome(&stored.outcome, &outcome)?
            {
                return Err(GovernedProviderError::Conflict);
            }
        } else {
            // Recheck before staging; the authoritative CAS still checks again.
            self.coordinator
                .check_reconciliation_authorization(
                    &attempt_id(op.context.execution_id(), ordinal),
                    authorization,
                )
                .await
                .map_err(|error| reconciliation_authority_error(&error))?;
            let resolution = Resolution {
                schema: 1,
                attempt,
                verifier_revision: verifier.revision().into(),
                proof: proof.into(),
                outcome: outcome.clone(),
                resolved_at_ms: self.clock.now().timestamp_millis(),
                requires_management_authorization: true,
            };
            self.state
                .check_and_set(
                    &resolution_key(&op.context, ordinal),
                    &self.encode(&resolution)?,
                    None,
                )
                .await
                .map_err(|_| GovernedProviderError::Unavailable)?;
        }
        let (stored, reference) = self
            .load_resolution(op, ordinal, start)
            .await?
            .ok_or(GovernedProviderError::Unavailable)?;
        if stored.proof != proof
            || stored.verifier_revision != verifier.revision()
            || !stored.requires_management_authorization
            || !same_outcome(&stored.outcome, &outcome)?
        {
            return Err(GovernedProviderError::Conflict);
        }
        Ok((outcome, reference))
    }
    /// Return correlation for the latest unresolved retained attempt after
    /// checking signed ownership and current bounded management authority.
    /// Does not accept a candidate or repair an interrupted acknowledgment.
    pub async fn reconciliation_attempt_evaluated(
        &self,
        context: &ExecutionContextReference,
        owner: &PrincipalIdentity,
        ordinal: u32,
        authorization: ReconciliationAuthorization<'_>,
    ) -> Result<ReconciliationAttempt, GovernedProviderError> {
        if ordinal >= MAX_ATTEMPTS {
            return Err(GovernedProviderError::Invalid);
        }
        let (op, start) = self
            .management_attempt(context, owner, ordinal, &authorization)
            .await?;
        if start.status == AttemptStatus::Settled || start.reconciliation.is_some() {
            return Err(GovernedProviderError::Conflict);
        }
        descriptor(&op, ordinal, &start.token)
    }

    /// Verify local qualified evidence and atomically accept finality under
    /// current management authority. Never invokes or retries a provider.
    /// An accepted exact-proof replay preserves its original attribution.
    pub async fn reconcile_evaluated(
        &self,
        context: &ExecutionContextReference,
        owner: &PrincipalIdentity,
        ordinal: u32,
        proof: &[u8],
        authorization: ReconciliationAuthorization<'_>,
    ) -> Result<GovernedProviderReceipt, GovernedProviderError> {
        if proof.is_empty() || proof.len() > MAX_PROOF_BYTES || ordinal >= MAX_ATTEMPTS {
            return Err(GovernedProviderError::Invalid);
        }
        let id = attempt_id(context.execution_id(), ordinal);
        let (op, start) = self
            .management_attempt(context, owner, ordinal, &authorization)
            .await?;
        let existing = self.load_resolution(&op, ordinal, &start).await?;
        if let Some(link) = &start.reconciliation {
            let (stored, reference) = existing.ok_or(GovernedProviderError::Unavailable)?;
            if stored.proof != proof || link.resolution != reference {
                return Err(GovernedProviderError::Conflict);
            }
            self.coordinator
                .reconcile_attempt_evaluated(&id, &start.token, link.clone(), authorization)
                .await
                .map_err(|error| reconciliation_authority_error(&error))?;
            return Ok(GovernedProviderReceipt {
                execution_id: context.execution_id(),
                attempts: ordinal + 1,
                status: GovernedProviderStatus::Completed {
                    outcome: stored.outcome,
                },
            });
        }
        if start.status == AttemptStatus::Settled {
            return Err(GovernedProviderError::Conflict);
        }
        let original = self.load_evidence(&op, ordinal, &start.token).await?;
        if original.as_ref().is_some_and(|(e, _)| e.known)
            || (start.evidence.is_some()
                && original
                    .as_ref()
                    .is_none_or(|(_, r)| Some(r) != start.evidence.as_ref()))
        {
            return Err(GovernedProviderError::Conflict);
        }
        let (outcome, reference) = self
            .stage_management_resolution(&op, ordinal, &start, proof, &authorization)
            .await?;
        self.coordinator
            .reconcile_attempt_evaluated_with_original(
                &id,
                &start.token,
                start.evidence.clone(),
                AttemptReconciliationReference {
                    prior_status: start.status,
                    original_evidence: original.map(|(_, reference)| reference),
                    resolution: reference,
                },
                authorization,
            )
            .await
            .map_err(|error| reconciliation_authority_error(&error))?;
        Ok(GovernedProviderReceipt {
            execution_id: context.execution_id(),
            attempts: ordinal + 1,
            status: GovernedProviderStatus::Completed { outcome },
        })
    }
}
