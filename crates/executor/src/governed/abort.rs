//! Durable delivery of a provider abort request for already restricted work.
//!
//! The coordinator cancellation is the authority boundary. This module only
//! delivers a capability-specific request and records what is known about that
//! delivery. A socket failure, timeout, or process crash never becomes a
//! cancellation acknowledgement. Definitive provider finality is accepted only
//! through the binding's existing reconciliation verifier.

use super::{
    GovernedProviderError, GovernedProviderExecutor, GovernedProviderReceipt, attempt_id,
    reconciliation::{MAX_PROOF_BYTES, ReconciliationAttempt},
};
use acteon_core::{ExecutionContextReference, PrincipalIdentity};
use acteon_state::{CasResult, KeyKind, StateKey};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};

pub const PROVIDER_ABORT_KIND: &str = "governed_provider_abort";

/// Result of one adapter call. Adapters must classify every failure that could
/// have reached the provider as `Uncertain`; returning an error is conservative
/// and is also persisted as uncertain by the host.
pub enum ProviderAbortDisposition {
    /// The exact binding has no abort capability. No provider call was made.
    Unsupported,
    /// The request may or may not have taken effect. It must be reconciled.
    Uncertain,
    /// Provider-issued proof for the existing qualified finality verifier.
    FinalityProof(Vec<u8>),
}

/// Trusted adapter installed for one exact provider binding. Public input cannot
/// select the adapter, revision, destination, credential, or verifier.
#[async_trait]
pub trait ProviderAbortAdapter: Send + Sync {
    fn revision(&self) -> &str;

    async fn abort(
        &self,
        attempt: &ReconciliationAttempt,
    ) -> Result<ProviderAbortDisposition, GovernedProviderError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum AbortState {
    Registered,
    Unsupported,
    Uncertain,
    Proof { proof: Vec<u8> },
    Reconciled { proof_digest: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AbortRecord {
    schema: u32,
    attempt: ReconciliationAttempt,
    adapter_revision: String,
    requested_at_ms: i64,
    state: AbortState,
}

/// Honest public state for the provider-side intervention. `RestrictedOnly`
/// means Acteon blocked future starts but has no provider abort capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProviderAbortStatus {
    RestrictedOnly,
    Uncertain { attempt_id: String },
    Reconciled { proof_digest: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderAbortReceipt {
    pub execution: GovernedProviderReceipt,
    pub abort: ProviderAbortStatus,
}

fn valid_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= 1024 && !value.chars().any(char::is_control)
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_record(record: &AbortRecord) -> bool {
    record.schema == 1
        && valid_text(&record.adapter_revision)
        && record.requested_at_ms >= 0
        && match &record.state {
            AbortState::Proof { proof } => !proof.is_empty() && proof.len() <= MAX_PROOF_BYTES,
            AbortState::Reconciled { proof_digest } => valid_digest(proof_digest),
            AbortState::Registered | AbortState::Unsupported | AbortState::Uncertain => true,
        }
}

fn abort_key(attempt: &ReconciliationAttempt) -> StateKey {
    abort_key_for_id(&attempt.context, &attempt.attempt_id)
}

fn abort_key_for_id(context: &ExecutionContextReference, id: &str) -> StateKey {
    StateKey::new(
        context.namespace(),
        context.tenant(),
        KeyKind::Custom(PROVIDER_ABORT_KIND.into()),
        id,
    )
}

impl GovernedProviderExecutor {
    /// Install one host-owned abort adapter for this exact qualified binding.
    /// Finality proofs still require a separately installed reconciliation
    /// verifier; adapter installation alone cannot settle work.
    pub fn with_trusted_abort_adapter(
        mut self,
        adapter: Arc<dyn ProviderAbortAdapter>,
    ) -> Result<Self, GovernedProviderError> {
        if !valid_text(adapter.revision()) {
            return Err(GovernedProviderError::Invalid);
        }
        Arc::get_mut(&mut self.runtime)
            .ok_or(GovernedProviderError::Conflict)?
            .aborter = Some(adapter);
        Ok(self)
    }

    /// Deliver at most one automatic abort call for the latest registered
    /// attempt after the execution has been durably restricted. A crash or lost
    /// response leaves an uncertain record and never causes an automatic resend.
    /// A retained proof is replayed only into the idempotent reconciliation CAS.
    pub async fn abort_restricted(
        &self,
        context: &ExecutionContextReference,
        actor: &PrincipalIdentity,
    ) -> Result<Option<ProviderAbortReceipt>, GovernedProviderError> {
        let snapshot = self
            .runtime
            .coordinator
            .snapshot()
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?;
        if !snapshot
            .roots
            .get(&context.execution_id().to_string())
            .is_some_and(|root| root.cancelled)
        {
            return Err(GovernedProviderError::Admission(
                "provider abort requires a durable execution restriction",
            ));
        }
        let Some(observed) = self.inspect(context, actor).await? else {
            return Ok(None);
        };
        let Some(attempt) = self.reconciliation_attempt(context, actor).await? else {
            let Some(ordinal) = observed.attempts.checked_sub(1) else {
                return Ok(None);
            };
            let id = attempt_id(context.execution_id(), ordinal);
            let key = abort_key_for_id(context, &id);
            let Some(record) = self.load_abort_record(&key).await? else {
                return Ok(None);
            };
            let expected = self.abort_attempt(context, actor, ordinal).await?;
            if record.attempt != expected {
                return Err(GovernedProviderError::Conflict);
            }
            return self.finish_abort_record(&key, record, context, actor).await;
        };
        let key = abort_key(&attempt);
        if let Some(record) = self.load_abort_record(&key).await? {
            if record.attempt != attempt {
                return Err(GovernedProviderError::Conflict);
            }
            return self.finish_abort_record(&key, record, context, actor).await;
        }
        let Some(adapter) = self.runtime.aborter.as_ref() else {
            return Ok(Some(ProviderAbortReceipt {
                execution: observed,
                abort: ProviderAbortStatus::RestrictedOnly,
            }));
        };
        let requested_at_ms = self.runtime.clock.now().timestamp_millis();
        if requested_at_ms < 0 {
            return Err(GovernedProviderError::Invalid);
        }
        let candidate = AbortRecord {
            schema: 1,
            attempt: attempt.clone(),
            adapter_revision: adapter.revision().into(),
            requested_at_ms,
            state: AbortState::Registered,
        };
        let created = self
            .runtime
            .state
            .check_and_set(&key, &self.runtime.encode(&candidate)?, None)
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?;
        if !created {
            return self
                .resume_abort_record(&key, &candidate, context, actor)
                .await;
        }

        let delivery = tokio::time::timeout(
            Duration::from_nanos(self.runtime.settings.timeout_ns),
            adapter.abort(&attempt),
        )
        .await;
        let state = match delivery {
            Ok(Ok(ProviderAbortDisposition::Unsupported)) => AbortState::Unsupported,
            Ok(Ok(ProviderAbortDisposition::Uncertain) | Err(_)) | Err(_) => AbortState::Uncertain,
            Ok(Ok(ProviderAbortDisposition::FinalityProof(proof))) => {
                if proof.is_empty() || proof.len() > MAX_PROOF_BYTES {
                    AbortState::Uncertain
                } else {
                    AbortState::Proof { proof }
                }
            }
        };
        let saved = self.transition_abort(&key, &candidate, state).await?;
        self.finish_abort_record(&key, saved, context, actor).await
    }

    async fn load_abort_record(
        &self,
        key: &StateKey,
    ) -> Result<Option<AbortRecord>, GovernedProviderError> {
        self.runtime
            .state
            .get(key)
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?
            .map(|raw| {
                let (record, _) = self.runtime.decode::<AbortRecord>(&raw)?;
                if !valid_record(&record) {
                    return Err(GovernedProviderError::Conflict);
                }
                Ok(record)
            })
            .transpose()
    }

    async fn abort_attempt(
        &self,
        context: &ExecutionContextReference,
        actor: &PrincipalIdentity,
        ordinal: u32,
    ) -> Result<ReconciliationAttempt, GovernedProviderError> {
        let op = self
            .runtime
            .load_operation(context, actor)
            .await?
            .ok_or(GovernedProviderError::Ownership)?;
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
        super::reconciliation::descriptor(&op, ordinal, &start.token)
    }

    async fn resume_abort_record(
        &self,
        key: &StateKey,
        expected: &AbortRecord,
        context: &ExecutionContextReference,
        actor: &PrincipalIdentity,
    ) -> Result<Option<ProviderAbortReceipt>, GovernedProviderError> {
        let record = self
            .load_abort_record(key)
            .await?
            .ok_or(GovernedProviderError::Unavailable)?;
        if record.schema != 1
            || record.attempt != expected.attempt
            || record.adapter_revision != expected.adapter_revision
        {
            return Err(GovernedProviderError::Conflict);
        }
        self.finish_abort_record(key, record, context, actor).await
    }

    async fn finish_abort_record(
        &self,
        key: &StateKey,
        record: AbortRecord,
        context: &ExecutionContextReference,
        actor: &PrincipalIdentity,
    ) -> Result<Option<ProviderAbortReceipt>, GovernedProviderError> {
        let state = match &record.state {
            AbortState::Proof { proof } => {
                let execution = self.reconcile(context, actor, proof).await?;
                let proof_digest = format!("{:x}", Sha256::digest(proof));
                let next = AbortState::Reconciled {
                    proof_digest: proof_digest.clone(),
                };
                let saved = self.transition_abort(key, &record, next).await?;
                let AbortState::Reconciled { proof_digest } = saved.state else {
                    return Err(GovernedProviderError::Conflict);
                };
                return Ok(Some(ProviderAbortReceipt {
                    execution,
                    abort: ProviderAbortStatus::Reconciled { proof_digest },
                }));
            }
            AbortState::Registered | AbortState::Uncertain => ProviderAbortStatus::Uncertain {
                attempt_id: record.attempt.attempt_id.clone(),
            },
            AbortState::Unsupported => ProviderAbortStatus::RestrictedOnly,
            AbortState::Reconciled { proof_digest } => ProviderAbortStatus::Reconciled {
                proof_digest: proof_digest.clone(),
            },
        };
        let execution = self
            .inspect(context, actor)
            .await?
            .ok_or(GovernedProviderError::Ownership)?;
        Ok(Some(ProviderAbortReceipt {
            execution,
            abort: state,
        }))
    }

    async fn transition_abort(
        &self,
        key: &StateKey,
        expected: &AbortRecord,
        state: AbortState,
    ) -> Result<AbortRecord, GovernedProviderError> {
        for _ in 0..16 {
            let (raw, version) = self
                .runtime
                .state
                .get_versioned(key)
                .await
                .map_err(|_| GovernedProviderError::Unavailable)?
                .ok_or(GovernedProviderError::Unavailable)?;
            let (current, _): (AbortRecord, _) = self.runtime.decode(&raw)?;
            if current == *expected {
                let mut next = current;
                next.state = state.clone();
                if matches!(
                    self.runtime
                        .state
                        .compare_and_swap(key, version, &self.runtime.encode(&next)?, None)
                        .await
                        .map_err(|_| GovernedProviderError::Unavailable)?,
                    CasResult::Ok
                ) {
                    return Ok(next);
                }
                continue;
            }
            if current.schema != 1
                || current.attempt != expected.attempt
                || current.adapter_revision != expected.adapter_revision
            {
                return Err(GovernedProviderError::Conflict);
            }
            return Ok(current);
        }
        Err(GovernedProviderError::Unavailable)
    }
}
