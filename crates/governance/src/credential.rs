//! Exact credential-specific execution ceilings, published with effect starts.
//! Hosts resolve authenticated grants to complete effects; no secrets live here.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::context::{AcceptedEffect, RootContextAdmission, VerifiedExecutionContext};
use crate::permit::{
    ExecutionPermit, PermitDenial, PermitIssuanceCeiling, PermittedAttempt, matches_effect,
    valid_effects,
};
use crate::{
    AuthorityChange, AuthorityCoordinator, AuthorityStamp, CONTROL_BYTE_RESERVE,
    CONTROL_RECORD_RESERVE, ChangeRecord, CoordinationError, CoordinatorSnapshot, RETRIES,
    RootBudgetLimits, valid_text,
};

/// Trusted, resolved authority for one authentication identity. The ceiling ID
/// is the credential identifier, not a token, secret, or actor-wide grant union.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialAuthority {
    pub ceiling: ExecutionPermit,
    pub auth_method: String,
    pub execution_enabled: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialRecord {
    pub authority: CredentialAuthority,
    pub revoked: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialReference {
    pub id: String,
    pub accepted_revision: u64,
}

impl AuthorityCoordinator {
    pub(crate) fn valid_credential(&self, credential: &CredentialAuthority) -> bool {
        self.valid_permit(&credential.ceiling) && valid_text(&credential.auth_method)
    }
    pub(crate) fn valid_credential_history(&self, state: &CoordinatorSnapshot) -> bool {
        let mut reconstructed = BTreeMap::<String, CredentialRecord>::new();
        let mut events: Vec<_> = state.changes.values().collect();
        events.sort_by_key(|e| e.generation);
        for event in events {
            match &event.change {
                AuthorityChange::PublishCredential { credential } => {
                    let policy = &credential.ceiling;
                    let old = reconstructed.get(&policy.id);
                    if !self.valid_credential(credential)
                        || old.is_some_and(|r| r.revoked)
                        || old.map_or(Some(1), |r| r.authority.ceiling.revision.checked_add(1))
                            != Some(policy.revision)
                        || old.is_some_and(|r| {
                            r.authority.ceiling.subject != policy.subject
                                || r.authority.auth_method != credential.auth_method
                        })
                    {
                        return false;
                    }
                    reconstructed.insert(
                        policy.id.clone(),
                        CredentialRecord {
                            authority: credential.clone(),
                            revoked: false,
                        },
                    );
                }
                AuthorityChange::RevokeCredential {
                    credential_id,
                    expected_revision,
                } => {
                    let Some(record) = reconstructed.get_mut(credential_id) else {
                        return false;
                    };
                    if record.authority.ceiling.revision != *expected_revision {
                        return false;
                    }
                    record.revoked = true;
                }
                _ => {}
            }
        }
        reconstructed == state.credentials
    }

    /// Trusted publication after authentication/configuration review. Ceiling
    /// bounds and expected stamp are independently evaluated by the host. This
    /// operation does not authenticate a credential or authorize its publisher.
    #[allow(clippy::too_many_arguments)]
    pub async fn publish_credential(
        &self,
        change_id: &str,
        credential: CredentialAuthority,
        expected_revision: u64,
        issuance: &PermitIssuanceCeiling,
        evaluated_authority: &AuthorityStamp,
        reason: &str,
        now_ms: i64,
    ) -> Result<ChangeRecord, CoordinationError> {
        let policy = &credential.ceiling;
        if ![change_id, reason].into_iter().all(valid_text)
            || !self.valid_credential(&credential)
            || expected_revision.checked_add(1) != Some(policy.revision)
            || !valid_effects(&issuance.effects)
            || issuance.subjects.is_empty()
            || issuance.subjects.len() > 16
            || issuance.valid_from_ms < 0
            || issuance.limits.max_units == 0
            || issuance.limits.max_concurrent == 0
            || issuance.limits.deadline_ms <= issuance.valid_from_ms
            || now_ms < 0
        {
            return Err(CoordinationError::Invalid("credential publication".into()));
        }
        let change = AuthorityChange::PublishCredential {
            credential: credential.clone(),
        };
        for _ in 0..RETRIES {
            let (mut state, version) = self.load().await?;
            if state.incarnation != evaluated_authority.incarnation {
                return Err(CoordinationError::StaleAuthority);
            }
            if let Some(old) = state.changes.get(change_id) {
                if old.change != change || old.actor != issuance.issuer.id() || old.reason != reason
                {
                    return Err(CoordinationError::Conflict);
                }
                return Ok(old.clone());
            }
            if !issuance.subjects.contains(&policy.subject)
                || policy.valid_from_ms < issuance.valid_from_ms
                || now_ms < issuance.valid_from_ms
                || now_ms >= issuance.limits.deadline_ms
                || policy.limits.deadline_ms <= now_ms
                || policy.limits.deadline_ms > issuance.limits.deadline_ms
                || policy.limits.max_units > issuance.limits.max_units
                || policy.limits.max_concurrent > issuance.limits.max_concurrent
                || policy
                    .effects
                    .iter()
                    .any(|e| !issuance.effects.iter().any(|c| matches_effect(c, e)))
            {
                return Err(CoordinationError::PermitDenied(PermitDenial::Effect));
            }
            if state.stamp() != *evaluated_authority {
                return Err(CoordinationError::StaleAuthority);
            }
            if state.revoked_subjects.contains(issuance.issuer.id()) {
                return Err(CoordinationError::Restricted);
            }
            let old = state.credentials.get(&policy.id);
            if old.is_some_and(|r| r.revoked) {
                return Err(CoordinationError::PermitDenied(PermitDenial::Revoked));
            }
            if old.map_or(0, |r| r.authority.ceiling.revision) != expected_revision
                || old.is_some_and(|r| {
                    r.authority.ceiling.subject != policy.subject
                        || r.authority.auth_method != credential.auth_method
                })
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
            let event = ChangeRecord {
                change: change.clone(),
                actor: issuance.issuer.id().into(),
                reason: reason.into(),
                generation: state.generation,
                pending: true,
            };
            state.credentials.insert(
                policy.id.clone(),
                CredentialRecord {
                    authority: credential.clone(),
                    revoked: false,
                },
            );
            state.changes.insert(change_id.into(), event.clone());
            if Self::encode(&state)?.len() > state.limits.max_bytes - CONTROL_BYTE_RESERVE {
                return Err(CoordinationError::Capacity);
            }
            if self.commit(&state, version).await? {
                return Ok(event);
            }
        }
        Err(CoordinationError::Contention)
    }
}

fn original<'a>(
    state: &'a CoordinatorSnapshot,
    reference: &CredentialReference,
) -> Result<&'a CredentialAuthority, CoordinationError> {
    state
        .changes
        .values()
        .find_map(|event| match &event.change {
            AuthorityChange::PublishCredential { credential }
                if credential.ceiling.id == reference.id
                    && credential.ceiling.revision == reference.accepted_revision =>
            {
                Some(credential)
            }
            _ => None,
        })
        .ok_or(CoordinationError::PermitDenied(PermitDenial::Missing))
}
fn current<'a>(
    state: &'a CoordinatorSnapshot,
    reference: &CredentialReference,
) -> Result<&'a CredentialAuthority, CoordinationError> {
    let deny = |r| CoordinationError::PermitDenied(r);
    let record = state
        .credentials
        .get(&reference.id)
        .ok_or_else(|| deny(PermitDenial::Missing))?;
    if record.revoked {
        return Err(deny(PermitDenial::Revoked));
    }
    if record.authority.ceiling.revision < reference.accepted_revision {
        return Err(deny(PermitDenial::Missing));
    }
    Ok(&record.authority)
}
fn check_identity(
    policy: &CredentialAuthority,
    id: &str,
    method: &str,
    subject: &acteon_core::PrincipalIdentity,
) -> Result<(), CoordinationError> {
    if policy.ceiling.id != id
        || policy.auth_method != method
        || policy.ceiling.subject != *subject
        || !policy.execution_enabled
    {
        return Err(CoordinationError::PermitDenied(PermitDenial::Subject));
    }
    Ok(())
}
fn check_effect(
    policy: &CredentialAuthority,
    effect: &AcceptedEffect,
) -> Result<(), CoordinationError> {
    if !policy
        .ceiling
        .effects
        .iter()
        .any(|e| matches_effect(e, effect))
    {
        return Err(CoordinationError::PermitDenied(PermitDenial::Effect));
    }
    Ok(())
}
fn within_limits(limits: &RootBudgetLimits, ceiling: &RootBudgetLimits) -> bool {
    limits.max_units <= ceiling.max_units
        && limits.max_concurrent <= ceiling.max_concurrent
        && limits.deadline_ms <= ceiling.deadline_ms
}

pub(crate) fn validate_root(
    state: &CoordinatorSnapshot,
    admission: &RootContextAdmission,
    reference: &CredentialReference,
    limits: &RootBudgetLimits,
    now_ms: i64,
) -> Result<(), CoordinationError> {
    let policy = current(state, reference)?;
    if policy.ceiling.revision != reference.accepted_revision {
        return Err(CoordinationError::StaleAuthority);
    }
    check_identity(
        policy,
        &admission.credential_id,
        &admission.auth_method,
        &admission.binding.principal,
    )?;
    if now_ms < policy.ceiling.valid_from_ms
        || now_ms >= policy.ceiling.limits.deadline_ms
        || !within_limits(limits, &policy.ceiling.limits)
        || admission.deadline_ms > policy.ceiling.limits.deadline_ms
    {
        return Err(CoordinationError::PermitDenied(PermitDenial::Limits));
    }
    for effect in &admission.accepted_effects {
        check_effect(policy, effect)?;
    }
    Ok(())
}

/// Compatibility actor-only contexts deliberately have no credential ceiling.
/// Server enforce profiles must require the credentialed capture entrypoint.
pub(crate) fn evaluate(
    state: &CoordinatorSnapshot,
    request: &PermittedAttempt<'_>,
    now_ms: i64,
) -> Result<(), CoordinationError> {
    let context: &VerifiedExecutionContext = request.context;
    let Some(reference) = context.credential_authority() else {
        return Ok(());
    };
    let accepted = original(state, reference)?;
    let policy = current(state, reference)?;
    check_identity(
        accepted,
        context.credential_id(),
        context.auth_method(),
        context.principal(),
    )?;
    check_identity(
        policy,
        context.credential_id(),
        context.auth_method(),
        context.principal(),
    )?;
    check_effect(accepted, request.effect)?;
    check_effect(policy, request.effect)?;
    let root = state
        .roots
        .get(&context.execution_id().to_string())
        .ok_or(CoordinationError::PermitDenied(PermitDenial::Binding))?;
    if !within_limits(&root.limits, &accepted.ceiling.limits)
        || context.deadline_ms() > accepted.ceiling.limits.deadline_ms
    {
        return Err(CoordinationError::PermitDenied(PermitDenial::Binding));
    }
    if now_ms < policy.ceiling.valid_from_ms || now_ms >= policy.ceiling.limits.deadline_ms {
        return Err(CoordinationError::PermitDenied(PermitDenial::Validity));
    }
    if root
        .spent_units
        .checked_add(request.units)
        .is_none_or(|u| u > policy.ceiling.limits.max_units)
        || root.active_attempts >= policy.ceiling.limits.max_concurrent
    {
        return Err(CoordinationError::PermitDenied(PermitDenial::Limits));
    }
    Ok(())
}
