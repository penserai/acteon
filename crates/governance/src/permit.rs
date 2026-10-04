//! Exact, current execution permits in the authority coordinator.
//! Publication is a privileged host boundary; this is not credential authentication.
use std::collections::{BTreeMap, BTreeSet};

use acteon_core::{ExecutionContextReference, PrincipalIdentity};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::context::{AcceptedEffect, VerifiedExecutionContext};
use crate::{
    AttemptRequest, AuthorityChange, AuthorityCoordinator, AuthorityStamp, CONTROL_BYTE_RESERVE,
    CONTROL_RECORD_RESERVE, ChangeRecord, CoordinationError, CoordinatorSnapshot, RETRIES,
    RootBudgetLimits, RootReservation, StartRegistration, valid_text,
};

const MAX_EFFECTS: usize = 128;
const MAX_REFERENCES: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionPermit {
    pub id: String,
    pub revision: u64,
    pub subject: PrincipalIdentity,
    pub effects: Vec<AcceptedEffect>,
    pub valid_from_ms: i64,
    /// Per-root ceilings, not an aggregate funding/accounting guarantee.
    pub limits: RootBudgetLimits,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermitRecord {
    pub permit: ExecutionPermit,
    /// Revocation is terminal for this ID. New authority requires a new ID.
    pub revoked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermitReference {
    pub id: String,
    pub accepted_revision: u64,
}

/// Independently evaluated management ceiling supplied by a trusted host.
/// No Deserialize implementation: public payloads cannot establish issuance rights.
pub struct PermitIssuanceCeiling {
    pub issuer: PrincipalIdentity,
    pub subjects: Vec<PrincipalIdentity>,
    pub effects: Vec<AcceptedEffect>,
    pub valid_from_ms: i64,
    pub limits: RootBudgetLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum PermitDenial {
    #[error("permit binding differs from accepted provenance")]
    Binding,
    #[error("permit missing or revision is not available")]
    Missing,
    #[error("permit revoked")]
    Revoked,
    #[error("permit subject differs from the actual actor")]
    Subject,
    #[error("permit not currently valid")]
    Validity,
    #[error("complete effect is outside the permit or accepted ceiling")]
    Effect,
    #[error("current permit limits exhausted")]
    Limits,
}

/// Root-only context profile. Delegated/represented authority is not synthesized.
pub struct PermittedAttempt<'a> {
    pub id: &'a str,
    pub context: &'a VerifiedExecutionContext,
    pub permits: &'a [PermitReference],
    pub effect: &'a AcceptedEffect,
    /// Semantic input digest computed by the trusted adapter from actual work.
    /// This root profile requires the originally admitted input binding.
    pub request_digest: &'a str,
    pub units: u64,
    /// Trusted host clock, sampled again after storage/CAS contention.
    pub clock: &'a dyn acteon_time::Clock,
}

pub(crate) fn matches_effect(accepted: &AcceptedEffect, effect: &AcceptedEffect) -> bool {
    accepted.operation == effect.operation
        && accepted.resources.len() == effect.resources.len()
        && effect
            .resources
            .iter()
            .all(|r| accepted.resources.contains(r))
}

pub(crate) fn valid_effects(effects: &[AcceptedEffect]) -> bool {
    !effects.is_empty()
        && effects.len() <= MAX_EFFECTS
        && effects.iter().enumerate().all(|(i, effect)| {
            valid_text(&effect.operation)
                && effect.operation != "*"
                && !effect.resources.is_empty()
                && effect.resources.len() <= crate::MAX_ATTEMPT_RESOURCES
                && effect.resources.iter().collect::<BTreeSet<_>>().len() == effect.resources.len()
                && !effects[..i]
                    .iter()
                    .any(|prior| matches_effect(prior, effect))
        })
}

/// Canonical selected-revision binding carried in the existing sealed context's
/// accepted-ceiling revision. A label from a prompt is never a verified context.
pub fn permit_revision_tag(references: &[PermitReference]) -> Result<String, CoordinationError> {
    if references.is_empty()
        || references.len() > MAX_REFERENCES
        || references
            .iter()
            .any(|r| !valid_text(&r.id) || r.accepted_revision == 0)
        || references
            .iter()
            .map(|r| &r.id)
            .collect::<BTreeSet<_>>()
            .len()
            != references.len()
    {
        return Err(CoordinationError::Invalid("permit references".into()));
    }
    let mut sorted: Vec<_> = references.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    let raw = serde_json::to_vec(&sorted)
        .map_err(|_| CoordinationError::Invalid("permit references".into()))?;
    Ok(format!("permits-v1:{:x}", Sha256::digest(raw)))
}

/// Canonical retained-start binding for a permitted effect. This fingerprint
/// supports observation; it does not establish authority or authorize a send.
pub fn permitted_attempt_digest(
    reference: &ExecutionContextReference,
    effect: &AcceptedEffect,
    units: u64,
    permits: &[PermitReference],
) -> Result<String, CoordinationError> {
    let mut canonical_effect = effect.clone();
    canonical_effect.resources.sort();
    let raw = serde_json::to_vec(&(
        reference,
        canonical_effect,
        units,
        permit_revision_tag(permits)?,
    ))
    .map_err(|_| CoordinationError::Invalid("effect digest".into()))?;
    Ok(format!("{:x}", Sha256::digest(raw)))
}

impl AuthorityCoordinator {
    pub(crate) fn valid_permit_history(&self, state: &CoordinatorSnapshot) -> bool {
        if state.changes.keys().any(|id| !valid_text(id)) {
            return false;
        }
        let mut events: Vec<_> = state.changes.values().collect();
        events.sort_by_key(|event| event.generation);
        let mut reconstructed: BTreeMap<String, PermitRecord> = BTreeMap::new();
        let mut generations = BTreeSet::new();
        for event in events {
            if event.generation == 0
                || event.generation > state.generation
                || !generations.insert(event.generation)
                || !valid_text(&event.actor)
                || !valid_text(&event.reason)
            {
                return false;
            }
            match &event.change {
                AuthorityChange::PublishPermit { permit } => {
                    let prior = reconstructed.get(&permit.id);
                    if !self.valid_permit(permit)
                        || prior.is_some_and(|p| p.revoked)
                        || prior.map_or(Some(1), |p| p.permit.revision.checked_add(1))
                            != Some(permit.revision)
                        || prior.is_some_and(|p| p.permit.subject != permit.subject)
                    {
                        return false;
                    }
                    reconstructed.insert(
                        permit.id.clone(),
                        PermitRecord {
                            permit: permit.clone(),
                            revoked: false,
                        },
                    );
                }
                AuthorityChange::RevokePermit {
                    permit_id,
                    expected_revision,
                } => {
                    let Some(record) = reconstructed.get_mut(permit_id) else {
                        return false;
                    };
                    if record.permit.revision != *expected_revision {
                        return false;
                    }
                    record.revoked = true;
                }
                _ => {}
            }
        }
        reconstructed == state.permits
    }
    pub(crate) fn valid_permit(&self, permit: &ExecutionPermit) -> bool {
        valid_text(&permit.id)
            && permit.revision > 0
            && valid_effects(&permit.effects)
            && permit.valid_from_ms >= 0
            && permit.limits.deadline_ms > permit.valid_from_ms
            && permit.limits.max_units > 0
            && permit.limits.max_concurrent > 0
            && permit
                .effects
                .iter()
                .flat_map(|e| &e.resources)
                .all(|r| self.validate_resource_scope(r).is_ok())
    }

    /// Publish a current revision and pending control event through the same CAS
    /// as effect registration. Replays observe; optimistic revision/stamp conflicts
    /// require fresh management evaluation. A revoked ID cannot be reactivated.
    #[allow(clippy::too_many_arguments)]
    pub async fn publish_permit(
        &self,
        change_id: &str,
        permit: ExecutionPermit,
        expected_revision: u64,
        ceiling: &PermitIssuanceCeiling,
        evaluated_authority: &AuthorityStamp,
        reason: &str,
        now_ms: i64,
    ) -> Result<ChangeRecord, CoordinationError> {
        if ![change_id, reason].into_iter().all(valid_text)
            || !self.valid_permit(&permit)
            || expected_revision.checked_add(1) != Some(permit.revision)
            || !valid_effects(&ceiling.effects)
            || ceiling.valid_from_ms < 0
            || ceiling.subjects.is_empty()
            || ceiling.subjects.len() > MAX_REFERENCES
            || ceiling.limits.max_units == 0
            || ceiling.limits.max_concurrent == 0
            || ceiling.limits.deadline_ms <= ceiling.valid_from_ms
            || now_ms < 0
        {
            return Err(CoordinationError::Invalid("permit publication".into()));
        }
        let change = AuthorityChange::PublishPermit {
            permit: permit.clone(),
        };
        for _ in 0..RETRIES {
            let (mut state, version) = self.load().await?;
            if evaluated_authority.incarnation != state.incarnation {
                return Err(CoordinationError::StaleAuthority);
            }
            if let Some(existing) = state.changes.get(change_id) {
                if existing.change != change
                    || existing.actor != ceiling.issuer.id()
                    || existing.reason != reason
                {
                    return Err(CoordinationError::Conflict);
                }
                return Ok(existing.clone());
            }
            if !ceiling.subjects.contains(&permit.subject)
                || permit.valid_from_ms < ceiling.valid_from_ms
                || now_ms < ceiling.valid_from_ms
                || now_ms >= ceiling.limits.deadline_ms
                || permit.limits.deadline_ms <= now_ms
                || permit.limits.deadline_ms > ceiling.limits.deadline_ms
                || permit.limits.max_units > ceiling.limits.max_units
                || permit.limits.max_concurrent > ceiling.limits.max_concurrent
                || permit
                    .effects
                    .iter()
                    .any(|e| !ceiling.effects.iter().any(|c| matches_effect(c, e)))
            {
                return Err(CoordinationError::PermitDenied(PermitDenial::Effect));
            }
            if state.stamp() != *evaluated_authority {
                return Err(CoordinationError::StaleAuthority);
            }
            if state.revoked_subjects.contains(ceiling.issuer.id()) {
                return Err(CoordinationError::Restricted);
            }
            let old = state.permits.get(&permit.id);
            if old.is_some_and(|p| p.revoked) {
                return Err(CoordinationError::PermitDenied(PermitDenial::Revoked));
            }
            if old.map_or(0, |p| p.permit.revision) != expected_revision
                || old.is_some_and(|p| p.permit.subject != permit.subject)
            {
                return Err(CoordinationError::Conflict);
            }
            if state.record_count() + usize::from(old.is_none())
                >= state.limits.max_records - CONTROL_RECORD_RESERVE
            {
                return Err(CoordinationError::Capacity);
            }
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
            state.permits.insert(
                permit.id.clone(),
                PermitRecord {
                    permit: permit.clone(),
                    revoked: false,
                },
            );
            state.changes.insert(change_id.into(), record.clone());
            if Self::encode(&state)?.len() > state.limits.max_bytes - CONTROL_BYTE_RESERVE {
                return Err(CoordinationError::Capacity);
            }
            if self.commit(&state, version).await? {
                return Ok(record);
            }
        }
        Err(CoordinationError::Contention)
    }

    /// Register a fresh root-profile effect, reevaluating current permit limits
    /// on every CAS retry so concurrent budget writes cannot exceed a narrowed
    /// permit. Root ID is derived from verified provenance, never a caller payer.
    pub async fn register_permitted_attempt(
        &self,
        request: PermittedAttempt<'_>,
    ) -> Result<StartRegistration, CoordinationError> {
        let reference = request
            .context
            .reference()
            .map_err(|_| CoordinationError::PermitDenied(PermitDenial::Binding))?;
        if reference.namespace() != self.key.namespace.as_str()
            || reference.tenant() != self.key.tenant.as_str()
            || request.request_digest != reference.request_digest()
            || permit_revision_tag(request.permits)? != request.context.accepted_ceiling_revision()
        {
            return Err(CoordinationError::PermitDenied(PermitDenial::Binding));
        }
        let stamp = self.snapshot().await?.stamp();
        if request.context.authority_stamp().incarnation != stamp.incarnation {
            return Err(CoordinationError::StaleAuthority);
        }
        let root_id = request.context.execution_id().to_string();
        let digest =
            permitted_attempt_digest(&reference, request.effect, request.units, request.permits)?;
        self.register_attempt_checked(
            AttemptRequest {
                id: request.id,
                subject: request.context.principal().id(),
                resources: &request.effect.resources,
                request_digest: &digest,
                expected_authority: &stamp,
                reservation: Some(RootReservation {
                    root_id,
                    units: request.units,
                }),
                now_ms: request.clock.now().timestamp_millis(),
            },
            Some(&request),
        )
        .await
    }
}

pub(crate) fn evaluate(
    state: &CoordinatorSnapshot,
    request: &PermittedAttempt<'_>,
    now_ms: i64,
) -> Result<(), CoordinationError> {
    let deny = |reason| CoordinationError::PermitDenied(reason);
    if now_ms >= request.context.deadline_ms() {
        return Err(deny(PermitDenial::Validity));
    }
    if !request.context.within_accepted_ceiling(request.effect) {
        return Err(deny(PermitDenial::Effect));
    }
    crate::credential::evaluate(state, request, now_ms)?;
    let root_id = request.context.execution_id().to_string();
    let root = state
        .roots
        .get(&root_id)
        .ok_or_else(|| deny(PermitDenial::Binding))?;
    if root.owner_subject != request.context.principal().id() {
        return Err(deny(PermitDenial::Subject));
    }
    for reference in request.permits {
        let original = state
            .changes
            .values()
            .find_map(|event| match &event.change {
                AuthorityChange::PublishPermit { permit }
                    if permit.id == reference.id
                        && permit.revision == reference.accepted_revision =>
                {
                    Some(permit)
                }
                _ => None,
            })
            .ok_or_else(|| deny(PermitDenial::Missing))?;
        if original.subject != *request.context.principal()
            || root.limits.max_units > original.limits.max_units
            || root.limits.max_concurrent > original.limits.max_concurrent
            || root.limits.deadline_ms > request.context.deadline_ms()
            || request.context.deadline_ms() > original.limits.deadline_ms
        {
            return Err(deny(PermitDenial::Binding));
        }
        if !original
            .effects
            .iter()
            .any(|e| matches_effect(e, request.effect))
        {
            return Err(deny(PermitDenial::Effect));
        }
        let record = state
            .permits
            .get(&reference.id)
            .ok_or_else(|| deny(PermitDenial::Missing))?;
        let permit = &record.permit;
        if permit.revision < reference.accepted_revision {
            return Err(deny(PermitDenial::Missing));
        }
        if record.revoked {
            return Err(deny(PermitDenial::Revoked));
        }
        if permit.subject != *request.context.principal() {
            return Err(deny(PermitDenial::Subject));
        }
        if now_ms < permit.valid_from_ms || now_ms >= permit.limits.deadline_ms {
            return Err(deny(PermitDenial::Validity));
        }
        if !permit
            .effects
            .iter()
            .any(|e| matches_effect(e, request.effect))
        {
            return Err(deny(PermitDenial::Effect));
        }
        if root
            .spent_units
            .checked_add(request.units)
            .is_none_or(|spent| spent > permit.limits.max_units)
            || root.active_attempts >= permit.limits.max_concurrent
        {
            return Err(deny(PermitDenial::Limits));
        }
    }
    Ok(())
}

pub(crate) fn validate_root_admission(
    state: &CoordinatorSnapshot,
    admission: &crate::context::RootContextAdmission,
    references: &[PermitReference],
    limits: &RootBudgetLimits,
    now_ms: i64,
) -> Result<(), CoordinationError> {
    if state.stamp() != admission.evaluated_authority {
        return Err(CoordinationError::StaleAuthority);
    }
    if state
        .revoked_subjects
        .contains(admission.binding.principal.id())
    {
        return Err(CoordinationError::Restricted);
    }
    if now_ms < 0
        || limits.max_units == 0
        || limits.max_concurrent == 0
        || limits.deadline_ms != admission.deadline_ms
        || limits.deadline_ms <= now_ms
    {
        return Err(CoordinationError::PermitDenied(PermitDenial::Limits));
    }
    for reference in references {
        let record = state
            .permits
            .get(&reference.id)
            .ok_or(CoordinationError::PermitDenied(PermitDenial::Missing))?;
        let permit = &record.permit;
        if permit.revision != reference.accepted_revision {
            return Err(CoordinationError::StaleAuthority);
        }
        if record.revoked {
            return Err(CoordinationError::PermitDenied(PermitDenial::Revoked));
        }
        if permit.subject != admission.binding.principal {
            return Err(CoordinationError::PermitDenied(PermitDenial::Subject));
        }
        if now_ms < permit.valid_from_ms || now_ms >= permit.limits.deadline_ms {
            return Err(CoordinationError::PermitDenied(PermitDenial::Validity));
        }
        if limits.max_units > permit.limits.max_units
            || limits.max_concurrent > permit.limits.max_concurrent
            || limits.deadline_ms > permit.limits.deadline_ms
        {
            return Err(CoordinationError::PermitDenied(PermitDenial::Limits));
        }
        if admission.accepted_effects.iter().any(|e| {
            !permit
                .effects
                .iter()
                .any(|allowed| matches_effect(allowed, e))
        }) {
            return Err(CoordinationError::PermitDenied(PermitDenial::Effect));
        }
    }
    Ok(())
}
