//! At-most-once control effects for separately stored registry metadata.
use crate::{
    AuthorityChange, AuthorityCoordinator, CONTROL_BYTE_RESERVE, CONTROL_RECORD_RESERVE,
    CoordinationError, CoordinatorSnapshot, RETRIES,
};
use acteon_core::ResourceRef;
use acteon_state::{KeyKind, StateKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// The only metadata projections covered by this registry mutation protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistryProjectionKind {
    Agent,
    Card,
}
impl RegistryProjectionKind {
    fn key(self, agent: &ResourceRef) -> StateKey {
        StateKey::new(
            agent.namespace(),
            agent.tenant(),
            match self {
                Self::Agent => KeyKind::BusAgent,
                Self::Card => KeyKind::BusAgentCard,
            },
            agent.id(),
        )
    }
}

/// Bind exact serialized metadata (or deletion), not mutable descriptive labels.
#[must_use]
pub fn registry_projection_digest(
    kind: RegistryProjectionKind,
    expected_version: Option<u64>,
    value: Option<&str>,
) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(
                "acteon.registry-projection.v1",
                kind,
                expected_version,
                value
            ))
            .expect("strings and enum serialize")
        )
    )
}

pub(crate) fn mutation_pending(state: &CoordinatorSnapshot, agent: &ResourceRef) -> bool {
    state.changes.values().any(|record| record.pending && matches!(
        &record.change, AuthorityChange::BeginAgentRegistryMutation { agent: target, .. } if target == agent
    ))
}

impl AuthorityCoordinator {
    /// Deliver one admitted registry projection through the configured backend.
    /// Current bounded operator authority is required even for recovery. An
    /// existing in-flight/uncertain receipt never sends another write. Only a
    /// known write acknowledgement can complete the mutation fence; matching
    /// metadata alone is not finality for an ambiguous delivery.
    pub async fn execute_agent_registry_mutation(
        &self,
        id: &str,
        value: Option<&str>,
        authorization: crate::control::ControlChangeAuthorization<'_>,
    ) -> Result<(), CoordinationError> {
        if value.is_some_and(|raw| raw.len() > 256 * 1024) {
            return Err(CoordinationError::Capacity);
        }
        let (change, attempt) = self
            .register_registry_mutation_delivery(id, value, &authorization)
            .await?;
        let AuthorityChange::BeginAgentRegistryMutation {
            agent,
            projection,
            expected_projection_version,
            input_digest,
            ..
        } = change
        else {
            unreachable!()
        };
        let attempt_id = registry_mutation_attempt_id(id);
        let applied = match attempt {
            crate::StartRegistration::Existing(record) => {
                if record.status != crate::AttemptStatus::Settled {
                    return Err(CoordinationError::RegistryMutationUnresolved);
                }
                match record.evidence {
                    Some(evidence)
                        if evidence.id == attempt_id && evidence.digest == input_digest =>
                    {
                        true
                    }
                    Some(evidence)
                        if evidence.id == attempt_id
                            && evidence.digest == no_effect_digest(&input_digest) =>
                    {
                        false
                    }
                    _ => return Err(CoordinationError::Conflict),
                }
            }
            crate::StartRegistration::New(record) => {
                let key = projection.key(&agent);
                let result = match (value, expected_projection_version) {
                    (Some(raw), Some(version)) => self
                        .store
                        .compare_and_swap(&key, version, raw, None)
                        .await
                        .map(|result| matches!(result, acteon_state::CasResult::Ok)),
                    (Some(raw), None) => self.store.check_and_set(&key, raw, None).await,
                    (None, Some(version)) => self.store.compare_and_delete(&key, version).await,
                    (None, None) => self.store.get(&key).await.map(|actual| actual.is_none()),
                };
                let applied = match result {
                    Ok(applied) => applied,
                    Err(error) => {
                        let _ = self
                            .settle(&attempt_id, &record.token, crate::AttemptStatus::Uncertain)
                            .await;
                        return Err(error.into());
                    }
                };
                self.settle_with_evidence(
                    &attempt_id,
                    &record.token,
                    crate::AttemptStatus::Settled,
                    crate::AttemptEvidenceReference {
                        id: attempt_id.clone(),
                        digest: if applied {
                            input_digest.clone()
                        } else {
                            no_effect_digest(&input_digest)
                        },
                    },
                )
                .await?;
                applied
            }
        };
        self.complete_registry_mutation(id, &attempt_id, applied)
            .await?;
        if applied {
            Ok(())
        } else {
            Err(CoordinationError::Conflict)
        }
    }

    async fn register_registry_mutation_delivery(
        &self,
        id: &str,
        value: Option<&str>,
        authorization: &crate::control::ControlChangeAuthorization<'_>,
    ) -> Result<(AuthorityChange, crate::StartRegistration), CoordinationError> {
        for _ in 0..RETRIES {
            let (mut state, version) = self.load().await?;
            let mutation = state.changes.get(id).ok_or(CoordinationError::Conflict)?;
            let AuthorityChange::BeginAgentRegistryMutation {
                agent,
                projection,
                input_digest,
                expected_projection_version,
                ..
            } = &mutation.change
            else {
                return Err(CoordinationError::Restricted);
            };
            authorization.validate(self, &state, &mutation.change)?;
            if mutation.actor != authorization.ceiling.actor.id()
                || registry_projection_digest(*projection, *expected_projection_version, value)
                    != *input_digest
            {
                return Err(CoordinationError::Conflict);
            }
            let attempt_id = registry_mutation_attempt_id(id);
            let resources = BTreeSet::from([agent.clone()]);
            if let Some(record) = state.starts.get(&attempt_id) {
                if record.subject != mutation.actor
                    || record.resources != resources
                    || record.request_digest != *input_digest
                    || record.reservation.is_some()
                    || record.operation_evidence.is_some()
                {
                    return Err(CoordinationError::Conflict);
                }
                return Ok((
                    mutation.change.clone(),
                    crate::StartRegistration::Existing(record.clone()),
                ));
            }
            if !mutation.pending
                || state
                    .starts
                    .values()
                    .filter(|r| r.status != crate::AttemptStatus::Settled)
                    .count()
                    >= state.limits.max_active
                || state.record_count() >= state.limits.max_records - CONTROL_RECORD_RESERVE
            {
                return Err(CoordinationError::Capacity);
            }
            let change = mutation.change.clone();
            let record = crate::StartRecord {
                subject: mutation.actor.clone(),
                resources,
                reservation: None,
                request_digest: input_digest.clone(),
                authority: state.stamp(),
                token: uuid::Uuid::new_v4().to_string(),
                status: crate::AttemptStatus::InFlight,
                evidence: None,
                operation_evidence: None,
                reconciliation: None,
                reconciliation_acceptance: None,
            };
            state.starts.insert(attempt_id, record.clone());
            if Self::encode(&state)?.len() > state.limits.max_bytes - CONTROL_BYTE_RESERVE {
                return Err(CoordinationError::Capacity);
            }
            // This is an independently authorized control effect, so an agent
            // execution closure does not prevent its operator from removing it.
            if self.commit(&state, version).await? {
                return Ok((change, crate::StartRegistration::New(record)));
            }
        }
        Err(CoordinationError::Contention)
    }

    async fn complete_registry_mutation(
        &self,
        id: &str,
        attempt_id: &str,
        applied: bool,
    ) -> Result<(), CoordinationError> {
        for _ in 0..RETRIES {
            let (mut state, version) = self.load().await?;
            let record = state.changes.get(id).ok_or(CoordinationError::Conflict)?;
            let AuthorityChange::BeginAgentRegistryMutation {
                agent,
                projection,
                input_digest,
                expected_projection_version,
                ..
            } = &record.change
            else {
                return Err(CoordinationError::Restricted);
            };
            if !record.pending {
                return Ok(());
            }
            let delivery = state
                .starts
                .get(attempt_id)
                .ok_or(CoordinationError::Conflict)?;
            if delivery.status != crate::AttemptStatus::Settled
                || delivery.evidence
                    != Some(crate::AttemptEvidenceReference {
                        id: attempt_id.into(),
                        digest: if applied {
                            input_digest.clone()
                        } else {
                            no_effect_digest(input_digest)
                        },
                    })
            {
                return Err(CoordinationError::Restricted);
            }
            if applied {
                let actual = self.store.get(&projection.key(agent)).await?;
                if registry_projection_digest(
                    *projection,
                    *expected_projection_version,
                    actual.as_deref(),
                ) != *input_digest
                {
                    return Err(CoordinationError::Conflict);
                }
            }
            state.changes.get_mut(id).expect("loaded mutation").pending = false;
            state.generation = state
                .generation
                .checked_add(1)
                .ok_or(CoordinationError::Capacity)?;
            if self.commit(&state, version).await? {
                return Ok(());
            }
        }
        Err(CoordinationError::Contention)
    }
}

pub(super) fn registry_mutation_attempt_id(change_id: &str) -> String {
    format!(
        "registry-mutation/{:x}",
        Sha256::digest(change_id.as_bytes())
    )
}

pub(super) fn no_effect_digest(input_digest: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("acteon.registry-no-effect.v1/{input_digest}").as_bytes())
    )
}
