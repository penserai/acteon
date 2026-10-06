//! Explicit service delegation rights, serialized with effect starts.
//! A grant authorizes an exact agent invocation and qualified effect footprint;
//! it never grants the caller direct execution permission for that footprint.
use std::collections::BTreeMap;

use acteon_core::{PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef};
use acteon_time::Clock;
use serde::{Deserialize, Serialize};

use crate::context::AcceptedEffect;
use crate::permit::{matches_effect, valid_effects};
use crate::{
    AuthorityChange, AuthorityCoordinator, AuthorityStamp, CONTROL_BYTE_RESERVE,
    CONTROL_RECORD_RESERVE, ChangeRecord, CoordinationError, CoordinatorSnapshot, MAX_BUDGET_DEPTH,
    RETRIES, RootBudgetLimits, valid_text,
};

/// Operator-qualified service authority, independent from advertised cards.
/// The binding digest identifies the exact approved runtime/skill plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelegationGrant {
    pub id: String,
    pub revision: u64,
    pub source: PrincipalIdentity,
    pub target: PrincipalIdentity,
    pub agent_resource: ResourceRef,
    pub binding_digest: String,
    pub skill: String,
    pub ingress_effect: AcceptedEffect,
    /// Complete qualified intent; no wildcard or operation/resource cross-product.
    pub effects: Vec<AcceptedEffect>,
    pub valid_from_ms: i64,
    pub limits: RootBudgetLimits,
    pub max_depth: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelegationGrantReference {
    pub id: String,
    pub accepted_revision: u64,
}

/// Trusted host management ceiling; public JSON cannot establish issuer rights.
pub struct DelegationIssuanceCeiling {
    pub issuer: PrincipalIdentity,
    pub sources: Vec<PrincipalIdentity>,
    pub targets: Vec<PrincipalIdentity>,
    pub binding_digests: Vec<String>,
    pub ingress_effects: Vec<AcceptedEffect>,
    pub effects: Vec<AcceptedEffect>,
    pub valid_from_ms: i64,
    pub limits: RootBudgetLimits,
    pub max_depth: usize,
}

pub struct EvaluatedDelegationPublication<'a> {
    pub change_id: &'a str,
    pub grant: DelegationGrant,
    pub expected_revision: u64,
    pub ceiling: &'a DelegationIssuanceCeiling,
    pub evaluated_authority: &'a AuthorityStamp,
    pub reason: &'a str,
    pub clock: &'a dyn Clock,
}

fn digest_valid(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn bounded_limits(limits: &RootBudgetLimits, ceiling: &RootBudgetLimits) -> bool {
    limits.max_units > 0
        && limits.max_concurrent > 0
        && limits.max_units <= ceiling.max_units
        && limits.max_concurrent <= ceiling.max_concurrent
        && limits.deadline_ms <= ceiling.deadline_ms
}
impl DelegationIssuanceCeiling {
    pub fn validate(&self) -> Result<(), CoordinationError> {
        if self.sources.is_empty()
            || self.sources.len() > 16
            || self.targets.is_empty()
            || self.targets.len() > 16
            || self
                .targets
                .iter()
                .any(|p| p.kind() != PrincipalKind::Agent)
            || self.binding_digests.is_empty()
            || self.binding_digests.len() > 128
            || self.binding_digests.iter().any(|d| !digest_valid(d))
            || !valid_effects(&self.ingress_effects)
            || !valid_effects(&self.effects)
            || self.valid_from_ms < 0
            || self.limits.deadline_ms <= self.valid_from_ms
            || self.limits.max_units == 0
            || self.limits.max_concurrent == 0
            || self.max_depth == 0
            || self.max_depth >= MAX_BUDGET_DEPTH
        {
            return Err(CoordinationError::Invalid(
                "delegation issuance ceiling".into(),
            ));
        }
        Ok(())
    }
    fn covers(&self, grant: &DelegationGrant) -> bool {
        self.sources.contains(&grant.source)
            && self.targets.contains(&grant.target)
            && self.binding_digests.contains(&grant.binding_digest)
            && self
                .ingress_effects
                .iter()
                .any(|e| matches_effect(e, &grant.ingress_effect))
            && grant.effects.iter().all(|e| {
                self.effects
                    .iter()
                    .any(|allowed| matches_effect(allowed, e))
            })
            && grant.valid_from_ms >= self.valid_from_ms
            && bounded_limits(&grant.limits, &self.limits)
            && grant.max_depth <= self.max_depth
    }
}
impl AuthorityCoordinator {
    pub(crate) fn valid_delegation_grant(&self, grant: &DelegationGrant) -> bool {
        valid_text(&grant.id)
            && grant.revision > 0
            && grant.source != grant.target
            && grant.target.kind() == PrincipalKind::Agent
            && grant.agent_resource.kind() == ResourceKind::Agent
            && self.validate_resource_scope(&grant.agent_resource).is_ok()
            && digest_valid(&grant.binding_digest)
            && valid_text(&grant.skill)
            && grant.skill.len() <= 120
            && grant.ingress_effect.operation == "agent.invoke"
            && valid_effects(std::slice::from_ref(&grant.ingress_effect))
            && grant
                .ingress_effect
                .resources
                .contains(&grant.agent_resource)
            && grant
                .ingress_effect
                .resources
                .iter()
                .all(|r| self.validate_resource_scope(r).is_ok())
            && valid_effects(&grant.effects)
            && grant.effects.iter().all(|e| {
                e.resources
                    .iter()
                    .all(|r| grant.ingress_effect.resources.contains(r))
            })
            && grant.valid_from_ms >= 0
            && grant.limits.deadline_ms > grant.valid_from_ms
            && grant.limits.max_units > 0
            && grant.limits.max_concurrent > 0
            && grant.max_depth > 0
            && grant.max_depth < MAX_BUDGET_DEPTH
    }
    /// The bounded change log is the authoritative grant ledger. Keeping policy
    /// in the same CAS record makes revocation and starts share one boundary.
    pub(crate) fn valid_delegation_history(&self, state: &CoordinatorSnapshot) -> bool {
        let mut policies = BTreeMap::<String, (DelegationGrant, bool)>::new();
        let mut events: Vec<_> = state.changes.values().collect();
        events.sort_by_key(|e| e.generation);
        for event in events {
            match &event.change {
                AuthorityChange::PublishDelegationGrant { grant } => {
                    let old = policies.get(&grant.id);
                    if !self.valid_delegation_grant(grant)
                        || old.is_some_and(|(_, revoked)| *revoked)
                        || old.map_or(Some(1), |(g, _)| g.revision.checked_add(1))
                            != Some(grant.revision)
                        || old.is_some_and(|(g, _)| !same_binding(g, grant))
                    {
                        return false;
                    }
                    policies.insert(grant.id.clone(), (grant.clone(), false));
                }
                AuthorityChange::RevokeDelegationGrant {
                    grant_id,
                    expected_revision,
                } => {
                    let Some((grant, revoked)) = policies.get_mut(grant_id) else {
                        return false;
                    };
                    if grant.revision != *expected_revision || *revoked {
                        return false;
                    }
                    *revoked = true;
                }
                _ => {}
            }
        }
        true
    }
    /// Host-authenticated publication. Caller and recipient do not supply the
    /// issuer ceiling or approved binding. Check current authority before replay.
    pub async fn publish_delegation_grant(
        &self,
        request: EvaluatedDelegationPublication<'_>,
    ) -> Result<ChangeRecord, CoordinationError> {
        if !self.valid_delegation_grant(&request.grant)
            || !request.ceiling.covers(&request.grant)
            || request.expected_revision.checked_add(1) != Some(request.grant.revision)
        {
            return Err(CoordinationError::Restricted);
        }
        self.change_delegation(
            request.change_id,
            AuthorityChange::PublishDelegationGrant {
                grant: request.grant,
            },
            request.expected_revision,
            request.ceiling,
            request.evaluated_authority,
            request.reason,
            request.clock,
        )
        .await
    }
    /// Terminal retirement. Republish requires a new grant ID, and old sealed
    /// executions retain the original grant reference for denial and observation.
    #[allow(clippy::too_many_arguments)]
    pub async fn revoke_delegation_grant(
        &self,
        change_id: &str,
        grant_id: &str,
        expected_revision: u64,
        ceiling: &DelegationIssuanceCeiling,
        evaluated_authority: &AuthorityStamp,
        reason: &str,
        clock: &dyn Clock,
    ) -> Result<ChangeRecord, CoordinationError> {
        self.change_delegation(
            change_id,
            AuthorityChange::RevokeDelegationGrant {
                grant_id: grant_id.into(),
                expected_revision,
            },
            expected_revision,
            ceiling,
            evaluated_authority,
            reason,
            clock,
        )
        .await
    }
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn change_delegation(
        &self,
        change_id: &str,
        change: AuthorityChange,
        expected_revision: u64,
        ceiling: &DelegationIssuanceCeiling,
        evaluated_authority: &AuthorityStamp,
        reason: &str,
        clock: &dyn Clock,
    ) -> Result<ChangeRecord, CoordinationError> {
        ceiling.validate()?;
        if !valid_text(change_id) || !valid_text(reason) {
            return Err(CoordinationError::Invalid("delegation publication".into()));
        }
        for _ in 0..RETRIES {
            let (mut state, version) = self.load().await?;
            let now = clock.now().timestamp_millis();
            if state.purpose != crate::ScopePurpose::Execution
                || state.stamp() != *evaluated_authority
            {
                return Err(CoordinationError::StaleAuthority);
            }
            if now < ceiling.valid_from_ms
                || now >= ceiling.limits.deadline_ms
                || state.revoked_subjects.contains(ceiling.issuer.id())
            {
                return Err(CoordinationError::Restricted);
            }
            let (id, proposal) = match &change {
                AuthorityChange::PublishDelegationGrant { grant } => {
                    (grant.id.as_str(), Some(grant))
                }
                AuthorityChange::RevokeDelegationGrant { grant_id, .. } => {
                    (grant_id.as_str(), None)
                }
                _ => unreachable!("private delegation publication"),
            };
            let current = current(&state, id);
            if proposal.is_none() && !current.as_ref().is_some_and(|(g, _)| ceiling.covers(g)) {
                return Err(CoordinationError::Restricted);
            }
            if let Some(old) = state.changes.get(change_id) {
                if old.change != change || old.actor != ceiling.issuer.id() || old.reason != reason
                {
                    return Err(CoordinationError::Conflict);
                }
                return Ok(old.clone());
            }
            if current.as_ref().map_or(0, |(g, _)| g.revision) != expected_revision {
                return Err(CoordinationError::Conflict);
            }
            if current.as_ref().is_some_and(|(_, revoked)| *revoked) {
                return Err(CoordinationError::Restricted);
            }
            if let Some(grant) = proposal {
                if grant.limits.deadline_ms <= now
                    || !ceiling.covers(grant)
                    || current
                        .as_ref()
                        .is_some_and(|(g, _)| !same_binding(g, grant))
                {
                    return Err(CoordinationError::Restricted);
                }
            } else if !current.as_ref().is_some_and(|(g, _)| ceiling.covers(g)) {
                return Err(CoordinationError::Restricted);
            }
            if state.record_count()
                >= state.limits.max_records
                    - if proposal.is_some() {
                        CONTROL_RECORD_RESERVE
                    } else {
                        0
                    }
            {
                return Err(CoordinationError::Capacity);
            }
            state.generation = state
                .generation
                .checked_add(1)
                .ok_or(CoordinationError::Capacity)?;
            let event = ChangeRecord {
                change: change.clone(),
                actor: ceiling.issuer.id().into(),
                reason: reason.into(),
                generation: state.generation,
                pending: true,
            };
            state.changes.insert(change_id.into(), event.clone());
            if Self::encode(&state)?.len()
                > state.limits.max_bytes
                    - if proposal.is_some() {
                        CONTROL_BYTE_RESERVE
                    } else {
                        0
                    }
            {
                return Err(CoordinationError::Capacity);
            }
            if self.commit(&state, version).await? {
                return Ok(event);
            }
        }
        Err(CoordinationError::Contention)
    }
}
fn same_binding(a: &DelegationGrant, b: &DelegationGrant) -> bool {
    a.source == b.source
        && a.target == b.target
        && a.agent_resource == b.agent_resource
        && a.binding_digest == b.binding_digest
        && a.skill == b.skill
        && a.ingress_effect == b.ingress_effect
}
pub(crate) fn current(state: &CoordinatorSnapshot, id: &str) -> Option<(DelegationGrant, bool)> {
    let mut events: Vec<_> = state.changes.values().collect();
    events.sort_by_key(|e| e.generation);
    let mut result = None;
    for e in events {
        match &e.change {
            AuthorityChange::PublishDelegationGrant { grant } if grant.id == id => {
                result = Some((grant.clone(), false));
            }
            AuthorityChange::RevokeDelegationGrant { grant_id, .. } if grant_id == id => {
                if let Some((_, revoked)) = &mut result {
                    *revoked = true;
                }
            }
            _ => {}
        }
    }
    result
}
pub(crate) fn original<'a>(
    state: &'a CoordinatorSnapshot,
    reference: &DelegationGrantReference,
) -> Option<&'a DelegationGrant> {
    state.changes.values().find_map(|e| match &e.change {
        AuthorityChange::PublishDelegationGrant { grant }
            if grant.id == reference.id && grant.revision == reference.accepted_revision =>
        {
            Some(grant)
        }
        _ => None,
    })
}
