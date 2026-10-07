//! Signed cross-principal service intent and shared sponsorship.
use acteon_core::PrincipalIdentity;
use acteon_time::Clock;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

use super::{
    AcceptedEffect, ContextBinding, ContextError, ContextRecord, FORMAT, RootContextAdmission,
    TrustedContextStore, VerifiedExecutionContext,
};
use crate::delegation_policy::{self, DelegationGrant, DelegationGrantReference};
use crate::permit::{PermitReference, PermittedAttempt, matches_effect, valid_effects};
use crate::{
    AuthorityCoordinator, CONTROL_BYTE_RESERVE, CONTROL_RECORD_RESERVE, CoordinationError,
    CoordinatorSnapshot, MAX_BUDGET_DEPTH, MAX_ROOT_DESCENDANTS, RETRIES, RootBudget,
    RootBudgetLimits, RootReservation,
};

/// Root grant selection is part of initial acceptance, not a late upgrade.
pub struct DelegatingRootAdmission<'a> {
    pub admission_key: &'a str,
    pub admission: RootContextAdmission,
    pub permits: &'a [PermitReference],
    pub credential: crate::credential::CredentialReference,
    pub grants: Vec<DelegationGrantReference>,
    pub limits: RootBudgetLimits,
    pub representation: Option<&'a crate::workforce::VerifiedRepresentation>,
    pub clock: &'a dyn Clock,
}

/// Trusted recipient authentication and qualification for this exact new input.
/// No recipient seed root or client-provided identity/parent/payer is reused.
pub struct DelegatedContextAdmission<'a> {
    pub admission_key: &'a str,
    pub parent: &'a VerifiedExecutionContext,
    pub parent_permits: &'a [PermitReference],
    pub grant: DelegationGrantReference,
    pub binding_digest: &'a str,
    pub recipient: RootContextAdmission,
    pub recipient_permits: &'a [PermitReference],
    pub credential: crate::credential::CredentialReference,
    pub representation: Option<&'a crate::workforce::VerifiedRepresentation>,
    /// Complete qualified footprint including any onward service intent.
    /// Recipient direct effects can be a subset; its credentials must authorize
    /// each direct effect, while onward grants remain bounded by this footprint.
    pub intent_effects: Vec<AcceptedEffect>,
    pub onward_grants: Vec<DelegationGrantReference>,
    pub limits: RootBudgetLimits,
    pub clock: &'a dyn Clock,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DelegatedLineage {
    pub(super) source: Box<ContextRecord>,
    pub(super) source_permits: Vec<PermitReference>,
    grant: DelegationGrantReference,
    pub(super) boundary_execution_id: Uuid,
    pub(super) boundary_reference: acteon_core::ExecutionContextReference,
    intent_effects: Vec<AcceptedEffect>,
}

impl VerifiedExecutionContext {
    #[must_use]
    pub fn original_requester(&self) -> &PrincipalIdentity {
        original_actor(&self.0)
    }
    /// Original immediate service requester, recovered from signed lineage.
    /// This provenance is for observation; executing it still requires current
    /// context, credential, permit, workforce and budget checks.
    #[must_use]
    pub fn immediate_service_source(&self) -> Option<Self> {
        self.0
            .delegated_from
            .as_ref()
            .map(|lineage| Self((*lineage.source).clone()))
    }
    pub(crate) fn matches_service_runtime(
        &self,
        state: &CoordinatorSnapshot,
        binding_digest: &str,
        direct_effects: &[AcceptedEffect],
    ) -> bool {
        let Some(lineage) = &self.0.delegated_from else {
            return false;
        };
        let Some(grant) = delegation_policy::original(state, &lineage.grant) else {
            return false;
        };
        grant.binding_digest == binding_digest
            && grant.target == self.0.principal
            && grant.source == lineage.source.principal
            && self.credential_authority().is_some()
            && direct_effects.len() == self.0.accepted_effects.len()
            && valid_effects(direct_effects)
            && direct_effects
                .iter()
                .all(|effect| self.within_accepted_ceiling(effect))
    }
    /// Read-only qualification of an onward service against sealed ancestor intent.
    pub(crate) fn check_service_intent(
        &self,
        state: &CoordinatorSnapshot,
        grant: &DelegationGrantReference,
        effects: &[AcceptedEffect],
        now: i64,
    ) -> Result<(), CoordinationError> {
        if !self.0.delegation_grants.contains(grant) || !valid_effects(effects) {
            return Err(CoordinationError::Restricted);
        }
        let (original, current) = grant_pair(state, grant, now)?;
        for policy in [original, &current] {
            if policy.source != self.0.principal
                || !crate::budget::current_limits_allow(
                    state,
                    &self.execution_id().to_string(),
                    &policy.limits,
                    1,
                )?
                || effects.iter().any(|e| !contains(&policy.effects, e))
            {
                return Err(CoordinationError::Restricted);
            }
        }
        for effect in effects {
            intent_check(state, &self.0, effect, self.delegation_depth() + 1, now)?;
        }
        Ok(())
    }
    #[must_use]
    pub fn immediate_delegator(&self) -> Option<&PrincipalIdentity> {
        self.0.delegated_from.as_ref().map(|d| &d.source.principal)
    }
    #[must_use]
    pub fn accepted_delegation_grants(&self) -> &[DelegationGrantReference] {
        &self.0.delegation_grants
    }
    #[must_use]
    pub fn delegation_depth(&self) -> usize {
        cross_depth(&self.0)
    }
}
fn original_actor(record: &ContextRecord) -> &PrincipalIdentity {
    record.delegated_from.as_ref().map_or_else(
        || {
            record
                .representation
                .as_ref()
                .map_or(&record.principal, |r| &r.initiator)
        },
        |d| original_actor(&d.source),
    )
}
fn cross_depth(record: &ContextRecord) -> usize {
    record
        .delegated_from
        .as_ref()
        .map_or(0, |d| cross_depth(&d.source) + 1)
}
pub(super) fn valid_record_shape(record: &ContextRecord) -> bool {
    if record.delegation_grants.len() > 16
        || record
            .delegation_grants
            .iter()
            .any(|r| !crate::valid_text(&r.id) || r.accepted_revision == 0)
        || record
            .delegation_grants
            .iter()
            .map(|r| &r.id)
            .collect::<BTreeSet<_>>()
            .len()
            != record.delegation_grants.len()
    {
        return false;
    }
    let mut current = record;
    let mut depth = 0;
    let mut executions = BTreeSet::new();
    while let Some(d) = &current.delegated_from {
        depth += 1;
        if depth >= MAX_BUDGET_DEPTH
            || !executions.insert(current.execution_id)
            || d.boundary_execution_id.is_nil()
            || d.boundary_reference.execution_id() != d.boundary_execution_id
            || d.boundary_reference.principal() != &current.principal
            || d.boundary_reference.namespace() != record.namespace
            || d.boundary_reference.tenant() != record.tenant
            || current.lineage.is_none()
            || current.credential_authority.is_none()
            || d.source.credential_authority.is_none()
            || d.source.principal == current.principal
            || d.source.namespace != record.namespace
            || d.source.tenant != record.tenant
            || d.source.domain != record.domain
            || d.source.authority.incarnation != record.authority.incarnation
            || d.source.execution_id == current.execution_id
            || !d.source.delegation_grants.contains(&d.grant)
            || crate::permit::permit_revision_tag(&d.source_permits)
                .ok()
                .as_deref()
                != Some(d.source.accepted_ceiling_revision.as_str())
            || !valid_effects(&d.intent_effects)
            || current.accepted_effects.iter().any(|e| {
                !d.intent_effects
                    .iter()
                    .any(|allowed| matches_effect(allowed, e))
            })
        {
            return false;
        }
        current = &d.source;
    }
    true
}

pub(super) fn validate_grants(
    state: &CoordinatorSnapshot,
    source: &PrincipalIdentity,
    effects: &[AcceptedEffect],
    grants: &[DelegationGrantReference],
    intent: Option<&[AcceptedEffect]>,
    now: i64,
) -> Result<(), CoordinationError> {
    if grants.len() > 16
        || grants.iter().map(|r| &r.id).collect::<BTreeSet<_>>().len() != grants.len()
    {
        return Err(CoordinationError::Restricted);
    }
    for reference in grants {
        let (original, current) = grant_pair(state, reference, now)?;
        if original.revision != current.revision
            || &original.source != source
            || !effects
                .iter()
                .any(|e| matches_effect(e, &original.ingress_effect))
            || intent.is_some_and(|scope| {
                original
                    .effects
                    .iter()
                    .any(|e| !scope.iter().any(|allowed| matches_effect(allowed, e)))
            })
        {
            return Err(CoordinationError::Restricted);
        }
    }
    Ok(())
}
fn grant_pair<'a>(
    state: &'a CoordinatorSnapshot,
    reference: &DelegationGrantReference,
    now: i64,
) -> Result<(&'a DelegationGrant, DelegationGrant), CoordinationError> {
    let original =
        delegation_policy::original(state, reference).ok_or(CoordinationError::Restricted)?;
    let (current, revoked) =
        delegation_policy::current(state, &reference.id).ok_or(CoordinationError::Restricted)?;
    crate::registry::validate_grant(state, original)?;
    crate::registry::validate_grant(state, &current)?;
    if revoked
        || current.revision < original.revision
        || now < original.valid_from_ms
        || now >= original.limits.deadline_ms
        || now < current.valid_from_ms
        || now >= current.limits.deadline_ms
    {
        return Err(CoordinationError::Restricted);
    }
    Ok((original, current))
}
fn contains(effects: &[AcceptedEffect], effect: &AcceptedEffect) -> bool {
    effects
        .iter()
        .any(|allowed| matches_effect(allowed, effect))
}
fn intent_check(
    state: &CoordinatorSnapshot,
    record: &ContextRecord,
    effect: &AcceptedEffect,
    final_depth: usize,
    now: i64,
) -> Result<(), CoordinationError> {
    let Some(d) = &record.delegated_from else {
        return Ok(());
    };
    let (original, current) = grant_pair(state, &d.grant, now)?;
    if original.target != record.principal
        || original.source != d.source.principal
        || !contains(&d.intent_effects, effect)
        || !contains(&original.effects, effect)
        || !contains(&current.effects, effect)
        || final_depth - cross_depth(&d.source) > original.max_depth.min(current.max_depth)
    {
        return Err(CoordinationError::Restricted);
    }
    intent_check(state, &d.source, effect, final_depth, now)
}
pub(super) fn validate_lineage_provenance(
    context: &VerifiedExecutionContext,
    state: &CoordinatorSnapshot,
    now: i64,
) -> Result<(), CoordinationError> {
    let Some(d) = &context.0.delegated_from else {
        return Ok(());
    };
    for effect in &context.0.accepted_effects {
        intent_check(state, &context.0, effect, context.delegation_depth(), now)?;
    }
    VerifiedExecutionContext((*d.source).clone()).validate_inheritance(
        state,
        &d.source_permits,
        now,
    )
}
/// Runs within the same loaded coordinator snapshot/CAS as the actual start.
/// Current recipient permits still govern direct execution; source credentials
/// govern ingress requests, with every ancestor's accepted intent preserved.
pub(crate) fn delegated_effect_check(
    state: &CoordinatorSnapshot,
    request: &PermittedAttempt<'_>,
    now: i64,
    availability: crate::budget::Availability,
) -> Result<(), CoordinationError> {
    let context = request.context;
    let Some(d) = &context.0.delegated_from else {
        return Ok(());
    };
    intent_check(
        state,
        &context.0,
        request.effect,
        context.delegation_depth(),
        now,
    )?;
    let (original, current) = grant_pair(state, &d.grant, now)?;
    for grant in [original, &current] {
        if !crate::budget::current_limits_allow_with_availability(
            state,
            &context.execution_id().to_string(),
            &grant.limits,
            request.units,
            availability,
        )? {
            return Err(CoordinationError::Restricted);
        }
    }
    let source = VerifiedExecutionContext((*d.source).clone());
    source.validate_inheritance(state, &d.source_permits, now)?;
    crate::permit::evaluate_with_availability(
        state,
        &PermittedAttempt {
            id: request.id,
            context: &source,
            permits: &d.source_permits,
            effect: &original.ingress_effect,
            request_digest: &d.source.request_digest,
            units: request.units,
            clock: request.clock,
        },
        now,
        availability,
    )
}

pub(super) fn budget_owners_match(
    context: &VerifiedExecutionContext,
    state: &CoordinatorSnapshot,
    path: &[String],
) -> Result<bool, CoordinationError> {
    let Some(d) = &context.0.delegated_from else {
        return Ok(path
            .iter()
            .all(|id| state.roots[id].owner_subject == context.principal().id()));
    };
    let Some(boundary) = path
        .iter()
        .position(|id| id == &d.boundary_execution_id.to_string())
    else {
        return Ok(false);
    };
    if path[..=boundary]
        .iter()
        .any(|id| state.roots[id].owner_subject != context.principal().id())
    {
        return Ok(false);
    }
    if state.roots[&d.boundary_execution_id.to_string()]
        .accepted_context
        .as_ref()
        != Some(&d.boundary_reference)
    {
        return Ok(false);
    }
    let source = VerifiedExecutionContext((*d.source).clone());
    source.validate_budget_binding(state)?;
    Ok(path[boundary + 1..]
        == crate::budget::budget_path(state, &source.execution_id().to_string())?)
}

impl TrustedContextStore {
    pub async fn capture_delegating_root(
        &self,
        request: DelegatingRootAdmission<'_>,
    ) -> Result<VerifiedExecutionContext, ContextError> {
        self.capture_idempotent_inner(
            super::IdempotentRootAdmission {
                admission_key: request.admission_key,
                admission: request.admission,
                permits: request.permits,
                credential: request.credential,
                limits: request.limits,
                clock: request.clock,
            },
            request.representation,
            request.grants,
        )
        .await
    }
    /// Two recoverable context writes followed by one atomic sponsor allocation.
    /// Interrupted writes remain inert; every start rechecks live source/recipient
    /// authority and charges the immutable caller ancestry plus recipient leaf.
    #[allow(clippy::too_many_lines)]
    pub async fn capture_delegated_child(
        &self,
        mut request: DelegatedContextAdmission<'_>,
    ) -> Result<VerifiedExecutionContext, ContextError> {
        self.validate(&request.parent.0)?;
        if request.parent.credential_authority().is_none()
            || !request.parent.0.delegation_grants.contains(&request.grant)
            || !valid_effects(&request.intent_effects)
        {
            return Err(ContextError::Invalid);
        }
        let state = self.coordinator.snapshot().await?;
        let now = request.clock.now().timestamp_millis();
        let (grant, _) = grant_pair(&state, &request.grant, now)?;
        if grant.binding_digest != request.binding_digest
            || grant.source != *request.parent.principal()
            || grant.target != request.recipient.binding.principal
            || request.representation.is_some_and(|r| {
                !r.binds(&request.recipient, &request.limits)
                    || r.binding().initiator != *request.parent.principal()
            })
        {
            return Err(ContextError::Verification);
        }
        request.recipient.accepted_ceiling_revision =
            crate::permit::permit_revision_tag(request.recipient_permits)?;
        if request.recipient.evaluated_authority != state.stamp() {
            return Err(CoordinationError::StaleAuthority.into());
        }
        let restrictions: BTreeSet<_> = request
            .parent
            .restriction_resources()
            .iter()
            .chain(&grant.ingress_effect.resources)
            .cloned()
            .collect();
        let (key, admission_digest) = self.child_admission_key(request.admission_key)?;
        let boundary_reference = acteon_core::ExecutionContextReference::new(
            request.recipient.handle.0,
            request.recipient.binding.execution_id,
            state.namespace.clone(),
            state.tenant.clone(),
            request.recipient.binding.principal.clone(),
            request.recipient.binding.request_digest.clone(),
        )
        .map_err(|_| ContextError::Verification)?;
        let proposed = ContextRecord {
            schema_version: FORMAT,
            domain: self.domain.clone(),
            namespace: state.namespace.clone(),
            tenant: state.tenant.clone(),
            handle: request.recipient.handle,
            execution_id: request.recipient.binding.execution_id,
            principal: request.recipient.binding.principal,
            credential_id: request.recipient.credential_id,
            auth_method: request.recipient.auth_method,
            credential_authority: Some(request.credential),
            representation: request.representation.map(|r| r.binding.clone()),
            request_digest: request.recipient.binding.request_digest,
            accepted_ceiling_revision: request.recipient.accepted_ceiling_revision,
            accepted_effects: request.recipient.accepted_effects,
            deadline_ms: request.recipient.deadline_ms,
            admitted_at_ms: now,
            authority: state.stamp(),
            delegation_grants: request.onward_grants,
            delegated_from: Some(Box::new(DelegatedLineage {
                source: Box::new(request.parent.0.clone()),
                source_permits: request.parent_permits.to_vec(),
                grant: request.grant,
                boundary_execution_id: request.recipient.binding.execution_id,
                boundary_reference,
                intent_effects: request.intent_effects,
            })),
            lineage: Some(super::children::ChildLineage {
                parent: request.parent.reference()?,
                root_execution_id: request.parent.root_execution_id(),
                depth: request.parent.0.lineage.as_ref().map_or(1, |l| l.depth + 1),
                limits: request.limits,
                admission_digest,
                restrictions: restrictions.into_iter().collect(),
            }),
        };
        self.validate(&proposed)?;
        check_admission(
            &state,
            &proposed,
            request.recipient_permits,
            request.clock,
            now,
        )?;
        let encoded = self.seal_record(&proposed)?;
        let stored = if let Some(old) = self.store.get(&key).await? {
            old
        } else if self.store.check_and_set(&key, &encoded, None).await? {
            encoded
        } else {
            self.store.get(&key).await?.ok_or(ContextError::Missing)?
        };
        let original = self.open_record(&stored)?;
        Self::match_child(&original, &proposed)?;
        let state = self.coordinator.snapshot().await?;
        check_admission(
            &state,
            &original,
            request.recipient_permits,
            request.clock,
            request.clock.now().timestamp_millis(),
        )?;
        let context = self
            .recover_child_record(&original, &state, request.clock)
            .await?;
        self.coordinator
            .create_delegated_budget(&context, request.recipient_permits, request.clock)
            .await?;
        Ok(context)
    }
}
fn root_admission(record: &ContextRecord, state: &CoordinatorSnapshot) -> RootContextAdmission {
    RootContextAdmission {
        handle: record.handle.clone(),
        binding: ContextBinding {
            execution_id: record.execution_id,
            principal: record.principal.clone(),
            request_digest: record.request_digest.clone(),
        },
        credential_id: record.credential_id.clone(),
        auth_method: record.auth_method.clone(),
        accepted_ceiling_revision: record.accepted_ceiling_revision.clone(),
        accepted_effects: record.accepted_effects.clone(),
        deadline_ms: record.deadline_ms,
        evaluated_authority: state.stamp(),
    }
}
#[allow(
    clippy::too_many_lines,
    reason = "evaluate the complete source and recipient admission at one snapshot"
)]
fn check_admission(
    state: &CoordinatorSnapshot,
    record: &ContextRecord,
    permits: &[PermitReference],
    clock: &dyn Clock,
    now: i64,
) -> Result<(), CoordinationError> {
    let d = record
        .delegated_from
        .as_ref()
        .ok_or(CoordinationError::Restricted)?;
    let lineage = record
        .lineage
        .as_ref()
        .ok_or(CoordinationError::Restricted)?;
    let source = VerifiedExecutionContext((*d.source).clone());
    source.validate_inheritance(state, &d.source_permits, now)?;
    let (original, current) = grant_pair(state, &d.grant, now)?;
    if original.source != source.0.principal
        || original.target != record.principal
        || lineage.limits.deadline_ms != record.deadline_ms
        || now < record.admitted_at_ms
        || !crate::permit::valid_effects(&record.accepted_effects)
        || crate::permit::permit_revision_tag(permits)? != record.accepted_ceiling_revision
    {
        return Err(CoordinationError::Restricted);
    }
    let source_limits = &state.roots[&source.execution_id().to_string()].limits;
    for bounds in [&original.limits, &current.limits, source_limits] {
        if lineage.limits.max_units > bounds.max_units
            || lineage.limits.max_concurrent > bounds.max_concurrent
            || lineage.limits.deadline_ms > bounds.deadline_ms
        {
            return Err(CoordinationError::Restricted);
        }
    }
    for effect in &d.intent_effects {
        intent_check(state, record, effect, cross_depth(record), now)?;
    }
    if record
        .accepted_effects
        .iter()
        .any(|e| !contains(&d.intent_effects, e))
    {
        return Err(CoordinationError::Restricted);
    }
    validate_grants(
        state,
        &record.principal,
        &record.accepted_effects,
        &record.delegation_grants,
        Some(&d.intent_effects),
        now,
    )?;
    let admission = root_admission(record, state);
    crate::permit::validate_root_admission_represented(
        state,
        &admission,
        permits,
        &lineage.limits,
        now,
        record.representation.as_ref(),
    )?;
    crate::credential::validate_root(
        state,
        &admission,
        record
            .credential_authority
            .as_ref()
            .ok_or(CoordinationError::Restricted)?,
        &lineage.limits,
        now,
    )?;
    crate::permit::evaluate_with_availability(
        state,
        &PermittedAttempt {
            id: "delegated-admission",
            context: &source,
            permits: &d.source_permits,
            effect: &original.ingress_effect,
            request_digest: &d.source.request_digest,
            units: 1,
            clock,
        },
        now,
        crate::budget::Availability::Admission,
    )?;
    if lineage
        .restrictions
        .iter()
        .any(|r| state.closed_resources.contains(r))
    {
        return Err(CoordinationError::Restricted);
    }
    let path = crate::budget::budget_path(state, &source.execution_id().to_string())?;
    if path
        .iter()
        .any(|id| state.roots[id].owner_subject == record.principal.id())
    {
        return Err(CoordinationError::Restricted);
    }
    crate::budget::check_root_availability(
        state,
        &RootReservation {
            root_id: source.execution_id().to_string(),
            units: 1,
        },
        now,
        crate::budget::Availability::Admission,
    )?;
    Ok(())
}
impl AuthorityCoordinator {
    async fn create_delegated_budget(
        &self,
        context: &VerifiedExecutionContext,
        permits: &[PermitReference],
        clock: &dyn Clock,
    ) -> Result<RootBudget, CoordinationError> {
        let record = &context.0;
        let lineage = record
            .lineage
            .as_ref()
            .ok_or(CoordinationError::Restricted)?;
        let child_id = record.execution_id.to_string();
        let parent_id = lineage.parent.execution_id().to_string();
        for _ in 0..RETRIES {
            let (mut state, version) = self.load().await?;
            check_admission(
                &state,
                record,
                permits,
                clock,
                clock.now().timestamp_millis(),
            )?;
            if let Some(existing) = state.roots.get(&child_id) {
                if existing.owner_subject != record.principal.id()
                    || existing.limits != lineage.limits
                    || existing.accepted_context.as_ref()
                        != Some(
                            &context
                                .reference()
                                .map_err(|_| CoordinationError::Restricted)?,
                        )
                    || state.budget_parents.get(&child_id) != Some(&parent_id)
                {
                    return Err(CoordinationError::Conflict);
                }
                return Ok(existing.clone());
            }
            let path = crate::budget::budget_path(&state, &parent_id)?;
            if path.len() >= MAX_BUDGET_DEPTH {
                return Err(CoordinationError::Capacity);
            }
            let root = path.last().ok_or(CoordinationError::Conflict)?;
            let descendants = state
                .budget_parents
                .keys()
                .filter(|id| {
                    crate::budget::budget_path(&state, id).is_ok_and(|p| p.last() == Some(root))
                })
                .count();
            if descendants >= MAX_ROOT_DESCENDANTS
                || state.record_count() + 2 > state.limits.max_records - CONTROL_RECORD_RESERVE
            {
                return Err(CoordinationError::Capacity);
            }
            let child = RootBudget {
                owner_subject: record.principal.id().into(),
                accepted_context: Some(
                    context
                        .reference()
                        .map_err(|_| CoordinationError::Restricted)?,
                ),
                limits: lineage.limits.clone(),
                spent_units: 0,
                active_attempts: 0,
                cancelled: false,
            };
            state
                .budget_parents
                .insert(child_id.clone(), parent_id.clone());
            state.roots.insert(child_id.clone(), child.clone());
            if Self::encode(&state)?.len() > state.limits.max_bytes - CONTROL_BYTE_RESERVE {
                return Err(CoordinationError::Capacity);
            }
            if self.commit(&state, version).await? {
                return Ok(child);
            }
        }
        Err(CoordinationError::Contention)
    }
}
