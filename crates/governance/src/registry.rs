//! Qualified registry revisions share the same CAS as delegated effect starts.
//! Descriptive cards do not publish qualification or revive retired bindings.
mod mutation;
pub(crate) use mutation::mutation_pending;
pub use mutation::{RegistryProjectionKind, registry_projection_digest};
use mutation::{no_effect_digest, registry_mutation_attempt_id};
use std::collections::{BTreeMap, BTreeSet};

use acteon_core::{PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef};
use acteon_time::Clock;
use serde::{Deserialize, Serialize};

use crate::{
    AuthorityChange, AuthorityCoordinator, AuthorityStamp, CONTROL_BYTE_RESERVE,
    CONTROL_RECORD_RESERVE, ChangeRecord, CoordinationError, CoordinatorSnapshot, RETRIES,
    ScopePurpose, valid_text,
};

/// One independently qualified agent revision and its exact approved skill digests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRegistryQualification {
    pub agent: ResourceRef,
    pub target: PrincipalIdentity,
    pub revision: u64,
    pub bindings: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRegistryRecord {
    pub qualification: AgentRegistryQualification,
    pub retired: bool,
}

/// Independent trusted-host approval, deliberately not deserializable. Public
/// registry metadata and ordinary execution permits cannot mint this ceiling.
pub struct AgentRegistryIssuanceCeiling {
    pub issuer: PrincipalIdentity,
    pub approved: Vec<AgentRegistryQualification>,
    pub valid_from_ms: i64,
    pub deadline_ms: i64,
}

pub(crate) fn valid_digest(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid(q: &AgentRegistryQualification) -> bool {
    q.agent.kind() == ResourceKind::Agent
        && q.target.kind() == PrincipalKind::Agent
        && q.revision > 0
        && !q.bindings.is_empty()
        && q.bindings.len() <= 16
        && q.bindings
            .iter()
            .all(|(skill, digest)| valid_text(skill) && skill.len() <= 256 && valid_digest(digest))
        && q.bindings.values().collect::<BTreeSet<_>>().len() == q.bindings.len()
}

fn apply(
    state: &mut BTreeMap<String, AgentRegistryRecord>,
    change: &AuthorityChange,
    seen: &mut BTreeSet<(String, String)>,
) -> Result<(), CoordinationError> {
    match change {
        AuthorityChange::PublishAgentRegistry { qualification: q } => {
            let key = q.agent.id().to_owned();
            let expected = state
                .get(&key)
                .map_or(Some(1), |r| r.qualification.revision.checked_add(1));
            if !valid(q)
                || expected != Some(q.revision)
                || q.bindings
                    .values()
                    .any(|digest| seen.contains(&(key.clone(), digest.clone())))
            {
                return Err(CoordinationError::Conflict);
            }
            for digest in q.bindings.values() {
                seen.insert((key.clone(), digest.clone()));
            }
            state.insert(
                key,
                AgentRegistryRecord {
                    qualification: q.clone(),
                    retired: false,
                },
            );
        }
        AuthorityChange::BeginAgentRegistryMutation {
            agent,
            expected_revision,
            input_digest,
            ..
        } => {
            if !valid_digest(input_digest) {
                return Err(CoordinationError::Conflict);
            }
            match state.get_mut(agent.id()) {
                Some(current)
                    if current.qualification.agent == *agent
                        && current.qualification.revision == *expected_revision =>
                {
                    current.retired = true;
                }
                None if *expected_revision == 0 => {}
                _ => return Err(CoordinationError::Conflict),
            }
        }
        AuthorityChange::RetireAgentRegistry {
            agent,
            expected_revision,
        } => {
            let current = state
                .get_mut(agent.id())
                .ok_or(CoordinationError::Conflict)?;
            if current.qualification.agent != *agent
                || current.qualification.revision != *expected_revision
            {
                return Err(CoordinationError::Conflict);
            }
            current.retired = true;
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn validate_grant(
    state: &CoordinatorSnapshot,
    grant: &crate::delegation_policy::DelegationGrant,
) -> Result<(), CoordinationError> {
    if mutation_pending(state, &grant.agent_resource) {
        return Err(CoordinationError::Restricted);
    }
    // Trusted abstract delegation can exist without a registry adapter. A mesh
    // host must register every executable service before exposing it; integration
    // adapters will require this record, rather than treat absence as approval.
    if let Some(record) = state.agent_registry.get(grant.agent_resource.id()) {
        let q = &record.qualification;
        if record.retired
            || q.agent != grant.agent_resource
            || q.target != grant.target
            || q.bindings.get(&grant.skill) != Some(&grant.binding_digest)
        {
            return Err(CoordinationError::Restricted);
        }
    }
    Ok(())
}

impl AuthorityCoordinator {
    pub(crate) fn valid_registry_history(&self, state: &CoordinatorSnapshot) -> bool {
        let mut reconstructed = BTreeMap::new();
        let mut seen = BTreeSet::new();
        let mut changes: Vec<_> = state.changes.values().collect();
        changes.sort_by_key(|r| r.generation);
        for record in changes {
            match &record.change {
                AuthorityChange::PublishAgentRegistry { qualification } => {
                    if state.purpose != ScopePurpose::Execution
                        || self.validate_resource_scope(&qualification.agent).is_err()
                    {
                        return false;
                    }
                }
                AuthorityChange::BeginAgentRegistryMutation { agent, .. }
                | AuthorityChange::RetireAgentRegistry { agent, .. } => {
                    if state.purpose != ScopePurpose::Execution
                        || self.validate_resource_scope(agent).is_err()
                    {
                        return false;
                    }
                }
                _ => continue,
            }
            if apply(&mut reconstructed, &record.change, &mut seen).is_err() {
                return false;
            }
        }
        let mut pending_agents = BTreeSet::new();
        for (id, record) in &state.changes {
            if let AuthorityChange::BeginAgentRegistryMutation {
                agent,
                input_digest,
                ..
            } = &record.change
            {
                if record.pending {
                    if !pending_agents.insert(agent.clone()) || state.changes.values().any(|later| {
                        later.generation > record.generation && matches!(&later.change,
                            AuthorityChange::PublishAgentRegistry { qualification } if qualification.agent == *agent)
                    }) {
                        return false;
                    }
                } else {
                    let attempt_id = registry_mutation_attempt_id(id);
                    let Some(delivery) = state.starts.get(&attempt_id) else {
                        return false;
                    };
                    if delivery.status != crate::AttemptStatus::Settled
                        || delivery.subject != record.actor
                        || delivery.request_digest != *input_digest
                        || delivery.resources != BTreeSet::from([agent.clone()])
                        || delivery.reservation.is_some()
                        || delivery.operation_evidence.is_some()
                        || delivery.authority.incarnation != state.incarnation
                        || delivery.authority.generation < record.generation
                        || !delivery.evidence.as_ref().is_some_and(|evidence| {
                            evidence.id == attempt_id
                                && (evidence.digest == *input_digest
                                    || evidence.digest == no_effect_digest(input_digest))
                        })
                    {
                        return false;
                    }
                }
            }
        }
        reconstructed == state.agent_registry
    }

    /// Publish independently reviewed qualification at the effect-start boundary.
    /// A digest is never reused for another revision: requalification must bind a
    /// new epoch into its digest, so old accepted work cannot regain authority.
    #[allow(clippy::too_many_arguments)]
    pub async fn publish_agent_registry(
        &self,
        id: &str,
        qualification: AgentRegistryQualification,
        expected_revision: u64,
        ceiling: &AgentRegistryIssuanceCeiling,
        evaluated: &AuthorityStamp,
        reason: &str,
        clock: &dyn Clock,
    ) -> Result<ChangeRecord, CoordinationError> {
        self.validate_resource_scope(&qualification.agent)?;
        if !valid(&qualification)
            || !valid_text(id)
            || !valid_text(reason)
            || ceiling.approved.is_empty()
            || ceiling.approved.len() > 128
            || ceiling
                .approved
                .iter()
                .any(|q| !valid(q) || self.validate_resource_scope(&q.agent).is_err())
            || !ceiling.approved.contains(&qualification)
            || ceiling.valid_from_ms < 0
            || ceiling.deadline_ms <= ceiling.valid_from_ms
        {
            return Err(CoordinationError::Restricted);
        }
        let change = AuthorityChange::PublishAgentRegistry { qualification };
        for _ in 0..RETRIES {
            let (mut state, version) = self.load().await?;
            let now = clock.now().timestamp_millis();
            if state.purpose != ScopePurpose::Execution || state.stamp() != *evaluated {
                return Err(CoordinationError::StaleAuthority);
            }
            if now < ceiling.valid_from_ms
                || now >= ceiling.deadline_ms
                || state.revoked_subjects.contains(ceiling.issuer.id())
            {
                return Err(CoordinationError::Restricted);
            }
            if mutation_pending(
                &state,
                match &change {
                    AuthorityChange::PublishAgentRegistry { qualification } => &qualification.agent,
                    _ => unreachable!(),
                },
            ) {
                return Err(CoordinationError::Restricted);
            }
            if let Some(old) = state.changes.get(id) {
                if old.change != change || old.actor != ceiling.issuer.id() || old.reason != reason
                {
                    return Err(CoordinationError::Conflict);
                }
                return Ok(old.clone());
            }
            let AuthorityChange::PublishAgentRegistry { qualification: q } = &change else {
                unreachable!()
            };
            if state
                .agent_registry
                .get(q.agent.id())
                .map_or(0, |r| r.qualification.revision)
                != expected_revision
            {
                return Err(CoordinationError::Conflict);
            }
            if state.record_count() >= state.limits.max_records - CONTROL_RECORD_RESERVE {
                return Err(CoordinationError::Capacity);
            }
            let mut seen = BTreeSet::new();
            for record in state.changes.values() {
                if let AuthorityChange::PublishAgentRegistry {
                    qualification: prior,
                } = &record.change
                {
                    for digest in prior.bindings.values() {
                        seen.insert((prior.agent.id().to_owned(), digest.clone()));
                    }
                }
            }
            apply(&mut state.agent_registry, &change, &mut seen)?;
            state.generation = state
                .generation
                .checked_add(1)
                .ok_or(CoordinationError::Capacity)?;
            let record = ChangeRecord {
                change: change.clone(),
                actor: ceiling.issuer.id().into(),
                reason: reason.into(),
                generation: state.generation,
                pending: true,
            };
            state.changes.insert(id.into(), record.clone());
            if state.record_count() > state.limits.max_records - CONTROL_RECORD_RESERVE
                || Self::encode(&state)?.len() > state.limits.max_bytes - CONTROL_BYTE_RESERVE
            {
                return Err(CoordinationError::Capacity);
            }
            if self.commit(&state, version).await? {
                return Ok(record);
            }
        }
        Err(CoordinationError::Contention)
    }

    pub(crate) fn begin_registry_mutation(
        state: &mut CoordinatorSnapshot,
        agent: &ResourceRef,
        expected_revision: u64,
    ) -> Result<(), CoordinationError> {
        apply(
            &mut state.agent_registry,
            &AuthorityChange::BeginAgentRegistryMutation {
                agent: agent.clone(),
                expected_revision,
                projection: RegistryProjectionKind::Agent,
                expected_projection_version: None,
                input_digest: "0".repeat(64),
            },
            &mut BTreeSet::new(),
        )
    }

    pub(crate) fn retire_registry(
        state: &mut CoordinatorSnapshot,
        agent: &ResourceRef,
        expected_revision: u64,
    ) -> Result<(), CoordinationError> {
        let mut seen = BTreeSet::new();
        apply(
            &mut state.agent_registry,
            &AuthorityChange::RetireAgentRegistry {
                agent: agent.clone(),
                expected_revision,
            },
            &mut seen,
        )
    }
}
