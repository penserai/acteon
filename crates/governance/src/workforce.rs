//! Versioned workforce relationships and representation at the coordinator CAS boundary.
//! Trusted hosts establish management and requester identity; descriptive metadata does not.
use std::collections::{BTreeMap, BTreeSet};

use acteon_core::{
    AgentOwnership, PrincipalIdentity, PrincipalKind, RepresentedParty, TeamRef, TeamRole,
    WorkforceAssignment, WorkforceMembership, WorkforceReference, WorkforceTeam,
};
use acteon_time::Clock;
use serde::{Deserialize, Serialize};

use crate::context::AcceptedEffect;
use crate::permit::{matches_effect, valid_effects};
use crate::{
    AuthorityChange, AuthorityCoordinator, AuthorityStamp, ChangeRecord, CoordinationError,
    CoordinatorSnapshot, RETRIES, RootBudgetLimits, ScopePurpose, valid_text,
};

const MAX_RELATIONSHIPS: usize = 128;
const MAX_DEPENDENCIES: usize = 16;

pub use acteon_core::workforce::WorkforceDependency;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepresentationMandate {
    pub id: String,
    pub revision: u64,
    pub represented: RepresentedParty,
    pub actor: PrincipalIdentity,
    pub job_class: String,
    /// Exact authenticated requesters; a host must establish their provenance.
    pub eligible_initiators: Vec<PrincipalIdentity>,
    /// Agent stewardship is pinned separately from actor and requester identities.
    pub ownership: Option<WorkforceReference>,
    /// Only explicit dependencies apply; a standing team mandate does not borrow its creator's membership.
    pub dependencies: Vec<WorkforceDependency>,
    pub effects: Vec<AcceptedEffect>,
    pub valid_from_ms: i64,
    pub limits: RootBudgetLimits,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkforceRecord<T> {
    pub value: T,
    pub revoked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepresentationPermitBinding {
    pub permit_id: String,
    pub permit_revision: u64,
    pub mandate: WorkforceReference,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkforceState {
    pub teams: BTreeMap<String, WorkforceRecord<WorkforceTeam>>,
    pub memberships: BTreeMap<String, WorkforceRecord<WorkforceMembership>>,
    pub ownership: BTreeMap<String, WorkforceRecord<AgentOwnership>>,
    pub assignments: BTreeMap<String, WorkforceRecord<WorkforceAssignment>>,
    pub mandates: BTreeMap<String, WorkforceRecord<RepresentationMandate>>,
    pub permit_bindings: BTreeMap<String, RepresentationPermitBinding>,
}
impl WorkforceState {
    pub(crate) fn record_count(&self) -> usize {
        self.teams.len()
            + self.memberships.len()
            + self.ownership.len()
            + self.assignments.len()
            + self.mandates.len()
            + self.permit_bindings.len()
    }
}

/// Describes a requested mutation, not proof of authority. Publication resolves current dependencies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkforceMutation {
    PublishRepresentedPermit {
        permit: crate::permit::ExecutionPermit,
        mandate: WorkforceReference,
    },
    PutTeam {
        team: WorkforceTeam,
    },
    DisbandTeam {
        team: TeamRef,
        expected_revision: u64,
    },
    PutMembership {
        membership: WorkforceMembership,
    },
    RemoveMembership {
        id: String,
        expected_revision: u64,
    },
    PutOwnership {
        ownership: AgentOwnership,
    },
    PutAssignment {
        assignment: WorkforceAssignment,
    },
    RemoveAssignment {
        id: String,
        expected_revision: u64,
    },
    PutMandate {
        mandate: RepresentationMandate,
    },
    RevokeMandate {
        id: String,
        expected_revision: u64,
    },
}

/// Independent deployment ceiling, never deserializable from a model or API body.
pub struct WorkforceManagementCeiling {
    pub actor: PrincipalIdentity,
    pub teams: Vec<TeamRef>,
    pub principals: Vec<PrincipalIdentity>,
    pub job_classes: Vec<String>,
    pub effects: Vec<AcceptedEffect>,
    pub valid_from_ms: i64,
    pub limits: RootBudgetLimits,
    pub can_manage_roster: bool,
    pub can_issue_mandates: bool,
    pub can_issue_permits: bool,
}
pub struct WorkforceManagementAuthorization<'a> {
    pub ceiling: &'a WorkforceManagementCeiling,
    pub evaluated_authority: &'a AuthorityStamp,
    pub clock: &'a dyn Clock,
}

pub(crate) fn team_key(team: &TeamRef) -> String {
    // Fixed validated fields; a JSON tuple avoids delimiter and prefix ambiguity.
    serde_json::to_string(&(team.domain(), team.tenant(), team.id()))
        .expect("validated team identity")
}
fn party_valid(party: &RepresentedParty, tenant: &str) -> bool {
    match party {
        RepresentedParty::Human { principal } => principal.kind() == PrincipalKind::Human,
        RepresentedParty::Team { team } => team.tenant() == tenant,
    }
}
fn reference_valid(reference: &WorkforceReference) -> bool {
    valid_text(&reference.id) && reference.accepted_revision > 0
}
fn interval_valid(from: i64, deadline: i64) -> bool {
    from >= 0 && deadline > from
}
fn current_interval(from: i64, deadline: i64, now: i64) -> bool {
    now >= from && now < deadline
}
fn classes_valid(classes: &[String]) -> bool {
    !classes.is_empty()
        && classes.len() <= MAX_RELATIONSHIPS
        && classes.iter().all(|c| valid_text(c))
        && classes.iter().collect::<BTreeSet<_>>().len() == classes.len()
}
fn active_team(state: &WorkforceState, team: &TeamRef) -> bool {
    state
        .teams
        .get(&team_key(team))
        .is_some_and(|r| !r.revoked && r.value.team == *team)
}
fn owner_active(state: &WorkforceState, owner: &RepresentedParty) -> bool {
    match owner {
        RepresentedParty::Team { team } => active_team(state, team),
        RepresentedParty::Human { .. } => true,
    }
}
fn limits_within(limits: &RootBudgetLimits, ceiling: &RootBudgetLimits) -> bool {
    limits.max_units > 0
        && limits.max_concurrent > 0
        && limits.max_units <= ceiling.max_units
        && limits.max_concurrent <= ceiling.max_concurrent
        && limits.deadline_ms <= ceiling.deadline_ms
}
impl WorkforceManagementCeiling {
    pub fn validate(&self) -> Result<(), CoordinationError> {
        if self.teams.len() > MAX_RELATIONSHIPS
            || self.principals.is_empty()
            || self.principals.len() > MAX_RELATIONSHIPS
            || self.teams.iter().collect::<BTreeSet<_>>().len() != self.teams.len()
            || self
                .principals
                .iter()
                .map(PrincipalIdentity::id)
                .collect::<BTreeSet<_>>()
                .len()
                != self.principals.len()
            || (!self.can_manage_roster && !self.can_issue_mandates && !self.can_issue_permits)
            || (!self.job_classes.is_empty() && !classes_valid(&self.job_classes))
            || ((self.can_issue_mandates || self.can_issue_permits) && self.job_classes.is_empty())
            || !interval_valid(self.valid_from_ms, self.limits.deadline_ms)
            || self.limits.max_units == 0
            || self.limits.max_concurrent == 0
            || ((self.can_issue_mandates || self.can_issue_permits)
                && !valid_effects(&self.effects))
        {
            return Err(CoordinationError::Invalid(
                "workforce management ceiling".into(),
            ));
        }
        Ok(())
    }
    fn covers_party(&self, party: &RepresentedParty) -> bool {
        match party {
            RepresentedParty::Human { principal } => self.principals.contains(principal),
            RepresentedParty::Team { team } => self.teams.contains(team),
        }
    }
    fn covers_mandate(&self, mandate: &RepresentationMandate) -> bool {
        self.covers_party(&mandate.represented)
            && self.job_classes.contains(&mandate.job_class)
            && self.principals.contains(&mandate.actor)
            && mandate
                .eligible_initiators
                .iter()
                .all(|p| self.principals.contains(p))
            && mandate.valid_from_ms >= self.valid_from_ms
            && limits_within(&mandate.limits, &self.limits)
            && mandate
                .effects
                .iter()
                .all(|e| self.effects.iter().any(|c| matches_effect(c, e)))
    }
}
impl WorkforceManagementAuthorization<'_> {
    fn validate(
        &self,
        coordinator: &AuthorityCoordinator,
        state: &CoordinatorSnapshot,
        mutation: &WorkforceMutation,
    ) -> Result<(), CoordinationError> {
        self.ceiling.validate()?;
        if state.purpose != ScopePurpose::Execution {
            return Err(CoordinationError::Restricted);
        }
        if state.stamp() != *self.evaluated_authority {
            return Err(CoordinationError::StaleAuthority);
        }
        let now = self.clock.now().timestamp_millis();
        if !current_interval(
            self.ceiling.valid_from_ms,
            self.ceiling.limits.deadline_ms,
            now,
        ) || state.revoked_subjects.contains(self.ceiling.actor.id())
        {
            return Err(CoordinationError::Restricted);
        }
        if self
            .ceiling
            .teams
            .iter()
            .any(|t| t.tenant() != coordinator.key.tenant.as_str())
        {
            return Err(CoordinationError::Invalid("workforce team tenant".into()));
        }
        for effect in &self.ceiling.effects {
            for resource in &effect.resources {
                coordinator.validate_resource_scope(resource)?;
            }
        }
        let allowed = self.allows_mutation(state, mutation);
        if allowed {
            Ok(())
        } else {
            Err(CoordinationError::Restricted)
        }
    }
    fn allows_mutation(&self, state: &CoordinatorSnapshot, mutation: &WorkforceMutation) -> bool {
        let roster = self.ceiling.can_manage_roster;
        let mandates = self.ceiling.can_issue_mandates;
        match mutation {
            WorkforceMutation::PublishRepresentedPermit { permit, mandate } => {
                self.ceiling.can_issue_permits
                    && self.ceiling.principals.contains(&permit.subject)
                    && permit.valid_from_ms >= self.ceiling.valid_from_ms
                    && limits_within(&permit.limits, &self.ceiling.limits)
                    && permit
                        .effects
                        .iter()
                        .all(|e| self.ceiling.effects.iter().any(|c| matches_effect(c, e)))
                    && state.workforce.mandates.get(&mandate.id).is_some_and(|r| {
                        self.ceiling.covers_party(&r.value.represented)
                            && self.ceiling.job_classes.contains(&r.value.job_class)
                    })
            }
            WorkforceMutation::PutTeam { team } => {
                roster && self.ceiling.teams.contains(&team.team)
            }
            WorkforceMutation::DisbandTeam { team, .. } => {
                roster && self.ceiling.teams.contains(team)
            }
            WorkforceMutation::PutMembership { membership } => {
                roster
                    && self.ceiling.teams.contains(&membership.team)
                    && self.ceiling.principals.contains(&membership.human)
            }
            WorkforceMutation::RemoveMembership { id, .. } => {
                roster
                    && state.workforce.memberships.get(id).is_some_and(|r| {
                        self.ceiling.teams.contains(&r.value.team)
                            && self.ceiling.principals.contains(&r.value.human)
                    })
            }
            WorkforceMutation::PutOwnership { ownership } => {
                roster
                    && self.ceiling.principals.contains(&ownership.agent)
                    && self.ceiling.covers_party(&ownership.owner)
                    && state
                        .workforce
                        .ownership
                        .get(ownership.agent.id())
                        .is_none_or(|r| self.ceiling.covers_party(&r.value.owner))
            }
            WorkforceMutation::PutAssignment { assignment } => {
                roster
                    && self.ceiling.teams.contains(&assignment.team)
                    && self.ceiling.principals.contains(&assignment.agent)
                    && assignment
                        .job_classes
                        .iter()
                        .all(|c| self.ceiling.job_classes.contains(c))
            }
            WorkforceMutation::RemoveAssignment { id, .. } => {
                roster
                    && state.workforce.assignments.get(id).is_some_and(|r| {
                        self.ceiling.teams.contains(&r.value.team)
                            && self.ceiling.principals.contains(&r.value.agent)
                    })
            }
            WorkforceMutation::PutMandate { mandate } => {
                mandates && self.ceiling.covers_mandate(mandate)
            }
            WorkforceMutation::RevokeMandate { id, .. } => {
                mandates
                    && state.workforce.mandates.get(id).is_some_and(|r| {
                        self.ceiling.covers_party(&r.value.represented)
                            && self.ceiling.principals.contains(&r.value.actor)
                            && r.value
                                .effects
                                .iter()
                                .all(|e| self.ceiling.effects.iter().any(|c| matches_effect(c, e)))
                    })
            }
        }
    }
}

fn next_revision(old: Option<u64>, revision: u64) -> bool {
    old.map_or(Some(1), |r| r.checked_add(1)) == Some(revision)
}
fn remove<T>(
    records: &mut BTreeMap<String, WorkforceRecord<T>>,
    id: &str,
    expected: u64,
    revision: impl Fn(&T) -> u64,
) -> Result<(), CoordinationError> {
    let record = records.get_mut(id).ok_or(CoordinationError::Conflict)?;
    if expected == 0 || record.revoked || revision(&record.value) != expected {
        return Err(CoordinationError::Conflict);
    }
    record.revoked = true;
    Ok(())
}

impl AuthorityCoordinator {
    /// Current authentication and independent policy must be evaluated by the host.
    /// Replay observes the original mutation after checking current management authority.
    pub async fn change_workforce(
        &self,
        id: &str,
        mutation: WorkforceMutation,
        reason: &str,
        authorization: WorkforceManagementAuthorization<'_>,
    ) -> Result<ChangeRecord, CoordinationError> {
        if ![id, reason].iter().all(|s| valid_text(s)) {
            return Err(CoordinationError::Invalid("workforce change fields".into()));
        }
        for _ in 0..RETRIES {
            let (mut state, version) = self.load().await?;
            authorization.validate(self, &state, &mutation)?;
            if let Some(record) = state.changes.get(id) {
                if matches!(&record.change, AuthorityChange::Workforce { mutation: saved, .. } if **saved == mutation)
                    && record.actor == authorization.ceiling.actor.id()
                    && record.reason == reason
                {
                    return Ok(record.clone());
                }
                return Err(CoordinationError::Conflict);
            }
            let now = authorization.clock.now().timestamp_millis();
            apply(
                self,
                &mut state.workforce,
                &mut state.permits,
                &mutation,
                now,
            )?;
            let granted_actor = match &mutation {
                WorkforceMutation::PutMandate { mandate } => Some(&mandate.actor),
                WorkforceMutation::PublishRepresentedPermit { permit, .. } => Some(&permit.subject),
                _ => None,
            };
            if granted_actor.is_some_and(|actor| state.revoked_subjects.contains(actor.id())) {
                return Err(CoordinationError::Restricted);
            }
            state.generation = state
                .generation
                .checked_add(1)
                .ok_or(CoordinationError::Capacity)?;
            let record = ChangeRecord {
                change: AuthorityChange::Workforce {
                    mutation: Box::new(mutation.clone()),
                    recorded_at_ms: now,
                },
                actor: authorization.ceiling.actor.id().into(),
                reason: reason.into(),
                generation: state.generation,
                pending: true,
            };
            state.changes.insert(id.into(), record.clone());
            let grants = matches!(
                &mutation,
                WorkforceMutation::PutTeam { .. }
                    | WorkforceMutation::PutMembership { .. }
                    | WorkforceMutation::PutOwnership { .. }
                    | WorkforceMutation::PutAssignment { .. }
                    | WorkforceMutation::PutMandate { .. }
                    | WorkforceMutation::PublishRepresentedPermit { .. }
            );
            if (grants
                && (state.record_count()
                    > state.limits.max_records - crate::CONTROL_RECORD_RESERVE
                    || Self::encode(&state)?.len()
                        > state.limits.max_bytes - crate::CONTROL_BYTE_RESERVE))
                || state.record_count() > state.limits.max_records
            {
                return Err(CoordinationError::Capacity);
            }
            if self.commit(&state, version).await? {
                return Ok(record);
            }
        }
        Err(CoordinationError::Contention)
    }
    pub(crate) fn valid_workforce_history(&self, state: &CoordinatorSnapshot) -> bool {
        if state.purpose != ScopePurpose::Execution
            && (state.workforce.record_count() != 0
                || state
                    .changes
                    .values()
                    .any(|r| matches!(r.change, AuthorityChange::Workforce { .. })))
        {
            return false;
        }
        let mut reconstructed = WorkforceState::default();
        let mut permits = BTreeMap::new();
        let mut events: Vec<_> = state.changes.values().collect();
        events.sort_by_key(|e| e.generation);
        for event in events {
            if let AuthorityChange::PublishPermit { permit } = &event.change {
                if has_representation(&reconstructed, &permit.id) {
                    return false;
                }
                permits.insert(
                    permit.id.clone(),
                    crate::permit::PermitRecord {
                        permit: permit.clone(),
                        revoked: false,
                    },
                );
            } else if let AuthorityChange::RevokePermit { permit_id, .. } = &event.change
                && let Some(record) = permits.get_mut(permit_id)
            {
                record.revoked = true;
            }
            if let AuthorityChange::Workforce {
                mutation,
                recorded_at_ms,
            } = &event.change
                && (*recorded_at_ms < 0
                    || apply(
                        self,
                        &mut reconstructed,
                        &mut permits,
                        mutation,
                        *recorded_at_ms,
                    )
                    .is_err())
            {
                return false;
            }
        }
        reconstructed == state.workforce
    }
}

fn apply(
    coordinator: &AuthorityCoordinator,
    state: &mut WorkforceState,
    permits: &mut BTreeMap<String, crate::permit::PermitRecord>,
    mutation: &WorkforceMutation,
    now: i64,
) -> Result<(), CoordinationError> {
    let invalid = || CoordinationError::Invalid("workforce relationship or mandate".into());
    match mutation {
        WorkforceMutation::PublishRepresentedPermit { permit, mandate } => {
            publish_represented_permit(coordinator, state, permits, permit, mandate, now)?;
        }
        WorkforceMutation::PutTeam { team } => {
            if team.team.tenant() != coordinator.key.tenant.as_str() || !valid_text(&team.name) {
                return Err(invalid());
            }
            let key = team_key(&team.team);
            let old = state.teams.get(&key);
            if old.is_some_and(|r| r.revoked)
                || !next_revision(old.map(|r| r.value.revision), team.revision)
            {
                return Err(CoordinationError::Conflict);
            }
            state.teams.insert(
                key,
                WorkforceRecord {
                    value: team.clone(),
                    revoked: false,
                },
            );
        }
        WorkforceMutation::DisbandTeam {
            team,
            expected_revision,
        } => remove(&mut state.teams, &team_key(team), *expected_revision, |t| {
            t.revision
        })?,
        WorkforceMutation::PutMembership { membership: member } => put_membership(state, member)?,
        WorkforceMutation::RemoveMembership {
            id,
            expected_revision,
        } => remove(&mut state.memberships, id, *expected_revision, |m| {
            m.revision
        })?,
        WorkforceMutation::PutOwnership { ownership } => {
            put_ownership(coordinator, state, ownership)?;
        }
        WorkforceMutation::PutAssignment { assignment } => put_assignment(state, assignment)?,
        WorkforceMutation::RemoveAssignment {
            id,
            expected_revision,
        } => remove(&mut state.assignments, id, *expected_revision, |a| {
            a.revision
        })?,
        WorkforceMutation::PutMandate { mandate } => {
            validate_mandate(
                coordinator.key.namespace.as_str(),
                coordinator.key.tenant.as_str(),
                state,
                mandate,
                now,
            )?;
            let old = state.mandates.get(&mandate.id);
            if !next_revision(old.map(|r| r.value.revision), mandate.revision)
                || old.is_some_and(|r| {
                    r.revoked
                        || r.value.actor != mandate.actor
                        || r.value.represented != mandate.represented
                        || r.value.job_class != mandate.job_class
                })
            {
                return Err(CoordinationError::Conflict);
            }
            state.mandates.insert(
                mandate.id.clone(),
                WorkforceRecord {
                    value: mandate.clone(),
                    revoked: false,
                },
            );
        }
        WorkforceMutation::RevokeMandate {
            id,
            expected_revision,
        } => remove(&mut state.mandates, id, *expected_revision, |m| m.revision)?,
    }
    Ok(())
}

fn membership<'a>(
    state: &'a WorkforceState,
    reference: &WorkforceReference,
    now: i64,
) -> Option<&'a WorkforceMembership> {
    state
        .memberships
        .get(&reference.id)
        .filter(|r| {
            !r.revoked
                && r.value.revision == reference.accepted_revision
                && current_interval(r.value.valid_from_ms, r.value.deadline_ms, now)
        })
        .map(|r| &r.value)
}
fn assignment<'a>(
    state: &'a WorkforceState,
    reference: &WorkforceReference,
    now: i64,
) -> Option<&'a WorkforceAssignment> {
    state
        .assignments
        .get(&reference.id)
        .filter(|r| {
            !r.revoked
                && r.value.revision == reference.accepted_revision
                && current_interval(r.value.valid_from_ms, r.value.deadline_ms, now)
        })
        .map(|r| &r.value)
}
fn validate_mandate(
    namespace: &str,
    tenant: &str,
    state: &WorkforceState,
    mandate: &RepresentationMandate,
    now: i64,
) -> Result<(), CoordinationError> {
    let invalid =
        || CoordinationError::Invalid("representation mandate bounds or dependencies".into());
    if !valid_text(&mandate.id)
        || mandate.revision == 0
        || !valid_text(&mandate.job_class)
        || !party_valid(&mandate.represented, tenant)
        || !owner_active(state, &mandate.represented)
        || !interval_valid(mandate.valid_from_ms, mandate.limits.deadline_ms)
        || now >= mandate.limits.deadline_ms
        || mandate.limits.max_units == 0
        || mandate.limits.max_concurrent == 0
        || !valid_effects(&mandate.effects)
        || mandate.eligible_initiators.is_empty()
        || mandate.eligible_initiators.len() > MAX_RELATIONSHIPS
        || mandate
            .eligible_initiators
            .iter()
            .map(PrincipalIdentity::id)
            .collect::<BTreeSet<_>>()
            .len()
            != mandate.eligible_initiators.len()
        || mandate.dependencies.len() > MAX_DEPENDENCIES
    {
        return Err(invalid());
    }
    for effect in &mandate.effects {
        for resource in &effect.resources {
            if resource.namespace() != namespace || resource.tenant() != tenant {
                return Err(invalid());
            }
        }
    }
    let mut dependencies = BTreeSet::new();
    for dependency in &mandate.dependencies {
        let (kind, reference) = match dependency {
            WorkforceDependency::Membership { reference } => ("membership", reference),
            WorkforceDependency::Assignment { reference } => ("assignment", reference),
        };
        if !reference_valid(reference) || !dependencies.insert((kind, &reference.id)) {
            return Err(invalid());
        }
        match dependency {
            WorkforceDependency::Membership { reference } => {
                let member = membership(state, reference, now).ok_or_else(invalid)?;
                if !matches!(&mandate.represented, RepresentedParty::Team { team } if *team == member.team)
                {
                    return Err(invalid());
                }
            }
            WorkforceDependency::Assignment { reference } => {
                let assigned = assignment(state, reference, now).ok_or_else(invalid)?;
                if assigned.agent != mandate.actor
                    || !assigned.job_classes.contains(&mandate.job_class)
                    || !matches!(&mandate.represented, RepresentedParty::Team { team } if *team == assigned.team)
                {
                    return Err(invalid());
                }
            }
        }
    }
    validate_actor_representation(state, mandate, now)?;
    Ok(())
}

pub(crate) fn permit_binding_key(id: &str, revision: u64) -> String {
    serde_json::to_string(&(id, revision)).expect("permit reference tuple")
}
pub(crate) fn has_representation(state: &WorkforceState, id: &str) -> bool {
    state.permit_bindings.values().any(|b| b.permit_id == id)
}
pub(crate) fn published_permit(
    change: &AuthorityChange,
) -> Option<&crate::permit::ExecutionPermit> {
    match change {
        AuthorityChange::PublishPermit { permit } => Some(permit),
        AuthorityChange::Workforce { mutation, .. } => match mutation.as_ref() {
            WorkforceMutation::PublishRepresentedPermit { permit, .. } => Some(permit),
            _ => None,
        },
        _ => None,
    }
}

/// Retained signed provenance. A deserialized value alone establishes no authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepresentationBinding {
    pub mandate: WorkforceReference,
    pub initiator: PrincipalIdentity,
    pub represented: RepresentedParty,
    pub job_class: String,
    pub ownership: Option<WorkforceReference>,
    pub dependencies: Vec<WorkforceDependency>,
}

/// Produced only by current mandate evaluation, never deserialized from public inputs.
#[derive(Debug, Clone)]
pub struct VerifiedRepresentation {
    pub(crate) binding: RepresentationBinding,
    execution: crate::context::ContextBinding,
    effects: Vec<AcceptedEffect>,
    limits: RootBudgetLimits,
    authority: AuthorityStamp,
}
impl VerifiedRepresentation {
    #[must_use]
    pub fn binding(&self) -> &RepresentationBinding {
        &self.binding
    }
    pub(crate) fn binds(
        &self,
        admission: &crate::context::RootContextAdmission,
        limits: &RootBudgetLimits,
    ) -> bool {
        self.execution.execution_id == admission.binding.execution_id
            && self.execution.principal == admission.binding.principal
            && self.execution.request_digest == admission.binding.request_digest
            && self.effects == admission.accepted_effects
            && self.limits == *limits
            && self.authority == admission.evaluated_authority
    }
}

/// Every fact comes from a trusted authenticated job adapter, not model metadata.
#[derive(Clone, Copy)]
pub struct WorkforceJobAdmission<'a> {
    pub mandate: &'a WorkforceReference,
    pub initiator: &'a PrincipalIdentity,
    pub job_class: &'a str,
    pub admission: &'a crate::context::RootContextAdmission,
    pub limits: &'a RootBudgetLimits,
    pub clock: &'a dyn Clock,
}

/// Resolve representation from permit history, never from caller-supplied
/// represented-party metadata. The host supplies the authenticated initiator
/// and the class of the actual qualified job.
#[derive(Clone, Copy)]
pub struct WorkforcePermitAdmission<'a> {
    pub permits: &'a [crate::permit::PermitReference],
    pub initiator: &'a PrincipalIdentity,
    pub job_class: &'a str,
    pub admission: &'a crate::context::RootContextAdmission,
    pub limits: &'a RootBudgetLimits,
    pub clock: &'a dyn Clock,
}

impl AuthorityCoordinator {
    pub async fn evaluate_permit_representation(
        &self,
        job: WorkforcePermitAdmission<'_>,
    ) -> Result<Option<VerifiedRepresentation>, CoordinationError> {
        let state = self.snapshot().await?;
        if state.purpose != ScopePurpose::Execution {
            return Err(CoordinationError::Restricted);
        }
        if state.stamp() != job.admission.evaluated_authority {
            return Err(CoordinationError::StaleAuthority);
        }
        let Some(mandate) = required_mandate(&state, job.permits)? else {
            return Ok(None);
        };
        evaluate_job_representation(
            &state,
            WorkforceJobAdmission {
                mandate: &mandate,
                initiator: job.initiator,
                job_class: job.job_class,
                admission: job.admission,
                limits: job.limits,
                clock: job.clock,
            },
        )
        .map(Some)
    }

    pub async fn evaluate_representation(
        &self,
        job: WorkforceJobAdmission<'_>,
    ) -> Result<VerifiedRepresentation, CoordinationError> {
        let state = self.snapshot().await?;
        if state.purpose != ScopePurpose::Execution {
            return Err(CoordinationError::Restricted);
        }
        if state.stamp() != job.admission.evaluated_authority {
            return Err(CoordinationError::StaleAuthority);
        }
        evaluate_job_representation(&state, job)
    }
}

fn evaluate_job_representation(
    state: &CoordinatorSnapshot,
    job: WorkforceJobAdmission<'_>,
) -> Result<VerifiedRepresentation, CoordinationError> {
    let record = state
        .workforce
        .mandates
        .get(&job.mandate.id)
        .filter(|r| !r.revoked && r.value.revision == job.mandate.accepted_revision)
        .ok_or(CoordinationError::Restricted)?;
    let binding = RepresentationBinding {
        mandate: job.mandate.clone(),
        initiator: job.initiator.clone(),
        represented: record.value.represented.clone(),
        job_class: job.job_class.into(),
        ownership: record.value.ownership.clone(),
        dependencies: record.value.dependencies.clone(),
    };
    validate_representation(
        state,
        &binding,
        &job.admission.binding.principal,
        &job.admission.accepted_effects,
        job.limits,
        job.clock.now().timestamp_millis(),
    )?;
    Ok(VerifiedRepresentation {
        binding,
        execution: job.admission.binding.clone(),
        effects: job.admission.accepted_effects.clone(),
        limits: job.limits.clone(),
        authority: state.stamp(),
    })
}

fn validate_representation(
    state: &CoordinatorSnapshot,
    binding: &RepresentationBinding,
    actor: &PrincipalIdentity,
    effects: &[AcceptedEffect],
    limits: &RootBudgetLimits,
    now: i64,
) -> Result<(), CoordinationError> {
    let mandate = &state
        .workforce
        .mandates
        .get(&binding.mandate.id)
        .filter(|r| !r.revoked && r.value.revision == binding.mandate.accepted_revision)
        .ok_or(CoordinationError::Restricted)?
        .value;
    if state.purpose != ScopePurpose::Execution
        || mandate.actor != *actor
        || mandate.represented != binding.represented
        || mandate.job_class != binding.job_class
        || mandate.ownership != binding.ownership
        || mandate.dependencies != binding.dependencies
        || !mandate.eligible_initiators.contains(&binding.initiator)
        || state.revoked_subjects.contains(actor.id())
        || state.revoked_subjects.contains(binding.initiator.id())
        || dependency_subject_revoked(state, binding)
        || matches!(&binding.represented, RepresentedParty::Human { principal } if state.revoked_subjects.contains(principal.id()))
        || !current_interval(mandate.valid_from_ms, mandate.limits.deadline_ms, now)
        || !limits_within(limits, &mandate.limits)
        || limits.deadline_ms <= now
        || !valid_effects(effects)
        || effects.iter().any(|e| {
            !mandate
                .effects
                .iter()
                .any(|allowed| matches_effect(allowed, e))
        })
    {
        return Err(CoordinationError::Restricted);
    }
    validate_mandate(
        &state.namespace,
        &state.tenant,
        &state.workforce,
        mandate,
        now,
    )
    .map_err(|_| CoordinationError::Restricted)
}

fn dependency_subject_revoked(
    state: &CoordinatorSnapshot,
    binding: &RepresentationBinding,
) -> bool {
    let owner_revoked = binding.ownership.as_ref().is_some_and(|reference| {
        state
            .workforce
            .ownership
            .get(&reference.id)
            .is_some_and(|record| {
                matches!(&record.value.owner, RepresentedParty::Human { principal }
                if state.revoked_subjects.contains(principal.id()))
            })
    });
    owner_revoked
        || binding
            .dependencies
            .iter()
            .any(|dependency| match dependency {
                WorkforceDependency::Membership { reference } => state
                    .workforce
                    .memberships
                    .get(&reference.id)
                    .is_some_and(|record| state.revoked_subjects.contains(record.value.human.id())),
                WorkforceDependency::Assignment { reference } => state
                    .workforce
                    .assignments
                    .get(&reference.id)
                    .is_some_and(|record| state.revoked_subjects.contains(record.value.agent.id())),
            })
}

fn required_mandate(
    state: &CoordinatorSnapshot,
    permits: &[crate::permit::PermitReference],
) -> Result<Option<WorkforceReference>, CoordinationError> {
    let mut required = None;
    for reference in permits {
        if let Some(binding) = state.workforce.permit_bindings.get(&permit_binding_key(
            &reference.id,
            reference.accepted_revision,
        )) {
            if required.as_ref().is_some_and(|r| r != &binding.mandate) {
                return Err(CoordinationError::Restricted);
            }
            let current = state
                .permits
                .get(&reference.id)
                .ok_or(CoordinationError::Restricted)?;
            if state
                .workforce
                .permit_bindings
                .get(&permit_binding_key(&reference.id, current.permit.revision))
                .is_none_or(|b| b.mandate != binding.mandate)
            {
                return Err(CoordinationError::Restricted);
            }
            required = Some(binding.mandate.clone());
        } else if state
            .permits
            .get(&reference.id)
            .is_some_and(|p| has_representation(&state.workforce, &p.permit.id))
        {
            // An ordinary old revision cannot be reused after representation became mandatory.
            return Err(CoordinationError::Restricted);
        }
    }
    Ok(required)
}

pub(crate) fn required_mandate_limits<'a>(
    state: &'a CoordinatorSnapshot,
    permits: &[crate::permit::PermitReference],
) -> Result<Option<&'a RootBudgetLimits>, CoordinationError> {
    let Some(reference) = required_mandate(state, permits)? else {
        return Ok(None);
    };
    state
        .workforce
        .mandates
        .get(&reference.id)
        .filter(|r| !r.revoked && r.value.revision == reference.accepted_revision)
        .map(|r| Some(&r.value.limits))
        .ok_or(CoordinationError::Restricted)
}

pub(crate) fn evaluate_root(
    state: &CoordinatorSnapshot,
    admission: &crate::context::RootContextAdmission,
    permits: &[crate::permit::PermitReference],
    limits: &RootBudgetLimits,
    now: i64,
    representation: Option<&RepresentationBinding>,
) -> Result<(), CoordinationError> {
    match (required_mandate(state, permits)?, representation) {
        (None, None) => Ok(()),
        (Some(required), Some(binding)) if required == binding.mandate => validate_representation(
            state,
            binding,
            &admission.binding.principal,
            &admission.accepted_effects,
            limits,
            now,
        ),
        _ => Err(CoordinationError::Restricted),
    }
}
pub(crate) fn evaluate_effect(
    state: &CoordinatorSnapshot,
    request: &crate::permit::PermittedAttempt<'_>,
    root: &crate::RootBudget,
    now: i64,
) -> Result<(), CoordinationError> {
    match (
        required_mandate(state, request.permits)?,
        request.context.representation(),
    ) {
        (None, None) => Ok(()),
        (Some(required), Some(binding)) if required == binding.mandate => {
            validate_representation(
                state,
                binding,
                request.context.principal(),
                std::slice::from_ref(request.effect),
                &root.limits,
                now,
            )?;
            let mandate = &state.workforce.mandates[&required.id].value;
            if root
                .spent_units
                .checked_add(request.units)
                .is_none_or(|n| n > mandate.limits.max_units)
                || root.active_attempts >= mandate.limits.max_concurrent
            {
                return Err(CoordinationError::Restricted);
            }
            Ok(())
        }
        _ => Err(CoordinationError::Restricted),
    }
}

pub(crate) fn binding_shape_valid(binding: &RepresentationBinding, tenant: &str) -> bool {
    reference_valid(&binding.mandate)
        && valid_text(&binding.job_class)
        && party_valid(&binding.represented, tenant)
        && binding.ownership.as_ref().is_none_or(reference_valid)
        && binding.dependencies.len() <= MAX_DEPENDENCIES
        && binding.dependencies.iter().all(|d| match d {
            WorkforceDependency::Membership { reference }
            | WorkforceDependency::Assignment { reference } => reference_valid(reference),
        })
}

fn publish_represented_permit(
    coordinator: &AuthorityCoordinator,
    state: &mut WorkforceState,
    permits: &mut BTreeMap<String, crate::permit::PermitRecord>,
    permit: &crate::permit::ExecutionPermit,
    mandate: &WorkforceReference,
    now: i64,
) -> Result<(), CoordinationError> {
    let invalid = || CoordinationError::Invalid("workforce relationship or mandate".into());

    if !reference_valid(mandate) || !coordinator.valid_permit(permit) {
        return Err(invalid());
    }
    let record = state
        .mandates
        .get(&mandate.id)
        .filter(|r| !r.revoked && r.value.revision == mandate.accepted_revision)
        .ok_or(CoordinationError::Restricted)?;
    validate_mandate(
        coordinator.key.namespace.as_str(),
        coordinator.key.tenant.as_str(),
        state,
        &record.value,
        now,
    )?;
    if permit.subject != record.value.actor
        || permit.valid_from_ms < record.value.valid_from_ms
        || !limits_within(&permit.limits, &record.value.limits)
        || permit.limits.deadline_ms <= now
        || permit
            .effects
            .iter()
            .any(|e| !record.value.effects.iter().any(|c| matches_effect(c, e)))
    {
        return Err(CoordinationError::Restricted);
    }
    let old = permits.get(&permit.id);
    if !next_revision(old.map(|r| r.permit.revision), permit.revision)
        || old.is_some_and(|r| r.revoked || r.permit.subject != permit.subject)
    {
        return Err(CoordinationError::Conflict);
    }
    permits.insert(
        permit.id.clone(),
        crate::permit::PermitRecord {
            permit: permit.clone(),
            revoked: false,
        },
    );
    state.permit_bindings.insert(
        permit_binding_key(&permit.id, permit.revision),
        RepresentationPermitBinding {
            permit_id: permit.id.clone(),
            permit_revision: permit.revision,
            mandate: mandate.clone(),
        },
    );

    Ok(())
}

fn put_membership(
    state: &mut WorkforceState,
    member: &WorkforceMembership,
) -> Result<(), CoordinationError> {
    let invalid = || CoordinationError::Invalid("workforce relationship or mandate".into());

    if !valid_text(&member.id)
        || member.human.kind() != PrincipalKind::Human
        || !active_team(state, &member.team)
        || member.roles.is_empty()
        || member.roles.len() > 4
        || member.roles.iter().collect::<BTreeSet<_>>().len() != member.roles.len()
        || !interval_valid(member.valid_from_ms, member.deadline_ms)
    {
        return Err(invalid());
    }
    let old = state.memberships.get(&member.id);
    if !next_revision(old.map(|r| r.value.revision), member.revision)
        || old.is_some_and(|r| {
            r.revoked || r.value.team != member.team || r.value.human != member.human
        })
        || state.memberships.iter().any(|(id, r)| {
            id != &member.id
                && !r.revoked
                && r.value.team == member.team
                && r.value.human == member.human
        })
    {
        return Err(CoordinationError::Conflict);
    }
    state.memberships.insert(
        member.id.clone(),
        WorkforceRecord {
            value: member.clone(),
            revoked: false,
        },
    );

    Ok(())
}

fn put_ownership(
    coordinator: &AuthorityCoordinator,
    state: &mut WorkforceState,
    ownership: &AgentOwnership,
) -> Result<(), CoordinationError> {
    let invalid = || CoordinationError::Invalid("workforce relationship or mandate".into());

    if ownership.agent.kind() != PrincipalKind::Agent
        || !party_valid(&ownership.owner, coordinator.key.tenant.as_str())
        || !owner_active(state, &ownership.owner)
    {
        return Err(invalid());
    }
    let old = state.ownership.get(ownership.agent.id());
    if !next_revision(old.map(|r| r.value.revision), ownership.revision)
        || old.is_some_and(|r| r.value.agent != ownership.agent)
    {
        return Err(CoordinationError::Conflict);
    }
    state.ownership.insert(
        ownership.agent.id().into(),
        WorkforceRecord {
            value: ownership.clone(),
            revoked: false,
        },
    );

    Ok(())
}

fn put_assignment(
    state: &mut WorkforceState,
    assignment: &WorkforceAssignment,
) -> Result<(), CoordinationError> {
    let invalid = || CoordinationError::Invalid("workforce relationship or mandate".into());

    if !valid_text(&assignment.id)
        || assignment.agent.kind() != PrincipalKind::Agent
        || !active_team(state, &assignment.team)
        || !classes_valid(&assignment.job_classes)
        || !interval_valid(assignment.valid_from_ms, assignment.deadline_ms)
        || !state.ownership.contains_key(assignment.agent.id())
    {
        return Err(invalid());
    }
    let old = state.assignments.get(&assignment.id);
    if !next_revision(old.map(|r| r.value.revision), assignment.revision)
        || old.is_some_and(|r| {
            r.revoked || r.value.agent != assignment.agent || r.value.team != assignment.team
        })
    {
        return Err(CoordinationError::Conflict);
    }
    state.assignments.insert(
        assignment.id.clone(),
        WorkforceRecord {
            value: assignment.clone(),
            revoked: false,
        },
    );

    Ok(())
}

fn validate_actor_representation(
    state: &WorkforceState,
    mandate: &RepresentationMandate,
    now: i64,
) -> Result<(), CoordinationError> {
    let invalid = || CoordinationError::Invalid("mandate actor ownership or membership".into());
    match mandate.actor.kind() {
        PrincipalKind::Agent => {
            let reference = mandate
                .ownership
                .as_ref()
                .filter(|r| reference_valid(r) && r.id == mandate.actor.id())
                .ok_or_else(invalid)?;
            let owner = &state
                .ownership
                .get(&reference.id)
                .filter(|r| !r.revoked && r.value.revision == reference.accepted_revision)
                .ok_or_else(invalid)?
                .value;
            match (&owner.owner, &mandate.represented) {
                (
                    RepresentedParty::Human { principal: owner },
                    RepresentedParty::Human {
                        principal: represented,
                    },
                ) if owner == represented => {}
                (
                    RepresentedParty::Team { team: owner },
                    RepresentedParty::Team { team: represented },
                ) if owner == represented => {}
                (RepresentedParty::Human { principal: owner }, RepresentedParty::Team { team }) => {
                    let sponsor = mandate.dependencies.iter().any(|d| match d {
                        WorkforceDependency::Membership { reference } => {
                            membership(state, reference, now).is_some_and(|m| {
                                m.human == *owner
                                    && m.team == *team
                                    && m.roles.contains(&TeamRole::Requester)
                            })
                        }
                        WorkforceDependency::Assignment { .. } => false,
                    });
                    let roster = mandate
                        .dependencies
                        .iter()
                        .any(|d| matches!(d, WorkforceDependency::Assignment { .. }));
                    if !sponsor || !roster {
                        return Err(invalid());
                    }
                }
                _ => return Err(invalid()),
            }
        }
        PrincipalKind::Human => {
            if mandate.ownership.is_some() {
                return Err(invalid());
            }
            match &mandate.represented {
                RepresentedParty::Human { principal } if *principal == mandate.actor => {}
                RepresentedParty::Team { team } => {
                    if !mandate.dependencies.iter().any(|d| match d {
                        WorkforceDependency::Membership { reference } => {
                            membership(state, reference, now).is_some_and(|m| {
                                m.human == mandate.actor
                                    && m.team == *team
                                    && m.roles.contains(&TeamRole::Requester)
                            })
                        }
                        WorkforceDependency::Assignment { .. } => false,
                    }) {
                        return Err(invalid());
                    }
                }
                RepresentedParty::Human { .. } => return Err(invalid()),
            }
        }
        PrincipalKind::Service | PrincipalKind::System => {
            if mandate.ownership.is_some()
                || !matches!(mandate.represented, RepresentedParty::Team { .. })
            {
                return Err(invalid());
            }
        }
    }
    Ok(())
}
