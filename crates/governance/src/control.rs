//! Bounded control changes evaluated against the same snapshot as their write.
//! These inputs come from a trusted host's current management authorization.
//! They do not authenticate callers or grant authority to public request fields.
use std::collections::BTreeSet;

use acteon_core::{PrincipalIdentity, ResourceRef};
use acteon_time::Clock;

use crate::{
    AuthorityChange, AuthorityCoordinator, AuthorityStamp, ChangeRecord, CoordinationError,
    CoordinatorSnapshot,
};

/// Independent intervention bounds. Deliberately not deserializable: execution
/// permits, dispatch grants and caller labels cannot mint management authority.
pub struct ControlChangeCeiling {
    pub actor: PrincipalIdentity,
    pub subjects: Vec<PrincipalIdentity>,
    pub resources: Vec<ResourceRef>,
    pub valid_from_ms: i64,
    pub deadline_ms: i64,
}

/// A host must bind the stamp to the actual authenticated credential's current
/// scope configuration and derive the ceiling from independent operator policy.
/// A stale result requires fresh authentication/management evaluation, rather
/// than substituting a new stamp into the old authorization.
pub struct ControlChangeAuthorization<'a> {
    pub ceiling: &'a ControlChangeCeiling,
    pub evaluated_authority: &'a AuthorityStamp,
    pub clock: &'a dyn Clock,
}

impl ControlChangeCeiling {
    pub fn validate(&self) -> Result<(), CoordinationError> {
        let subjects: BTreeSet<_> = self.subjects.iter().map(PrincipalIdentity::id).collect();
        let resources: BTreeSet<_> = self.resources.iter().collect();
        if self.subjects.len() > 128
            || self.resources.len() > 128
            || (self.subjects.is_empty() && self.resources.is_empty())
            || subjects.len() != self.subjects.len()
            || resources.len() != self.resources.len()
            || self.valid_from_ms < 0
            || self.deadline_ms <= self.valid_from_ms
        {
            return Err(CoordinationError::Invalid("control ceiling".into()));
        }
        Ok(())
    }

    fn covers_policy(&self, policy: &crate::permit::ExecutionPermit) -> bool {
        self.subjects.contains(&policy.subject)
            && policy
                .effects
                .iter()
                .flat_map(|effect| &effect.resources)
                .all(|resource| self.resources.contains(resource))
    }
}

impl ControlChangeAuthorization<'_> {
    pub(crate) fn validate(
        &self,
        coordinator: &AuthorityCoordinator,
        state: &CoordinatorSnapshot,
        change: &AuthorityChange,
    ) -> Result<(), CoordinationError> {
        self.ceiling.validate()?;
        for resource in &self.ceiling.resources {
            coordinator.validate_resource_scope(resource)?;
        }
        if state.purpose != crate::ScopePurpose::Execution {
            return Err(CoordinationError::Restricted);
        }
        if state.stamp() != *self.evaluated_authority {
            return Err(CoordinationError::StaleAuthority);
        }
        let now_ms = self.clock.now().timestamp_millis();
        if now_ms < self.ceiling.valid_from_ms
            || now_ms >= self.ceiling.deadline_ms
            || state.revoked_subjects.contains(self.ceiling.actor.id())
        {
            return Err(CoordinationError::Restricted);
        }
        let allowed = match change {
            AuthorityChange::BeginAgentRegistryMutation {
                agent: resource, ..
            }
            | AuthorityChange::RetireAgentRegistry {
                agent: resource, ..
            }
            | AuthorityChange::CloseResource { resource }
            | AuthorityChange::ReopenResource { resource } => {
                self.ceiling.resources.contains(resource)
            }
            AuthorityChange::RevokeSubject { subject } => self
                .ceiling
                .subjects
                .iter()
                .any(|actor| actor.id() == subject),
            AuthorityChange::RevokePermit { permit_id, .. } => state
                .permits
                .get(permit_id)
                .is_some_and(|record| self.ceiling.covers_policy(&record.permit)),
            AuthorityChange::RevokeCredential { credential_id, .. } => state
                .credentials
                .get(credential_id)
                .is_some_and(|record| self.ceiling.covers_policy(&record.authority.ceiling)),
            AuthorityChange::PublishAgentRegistry { .. }
            | AuthorityChange::CancelExecution { .. }
            | AuthorityChange::UpgradeProtocol { .. }
            | AuthorityChange::Workforce { .. }
            | AuthorityChange::ReserveScope { .. }
            | AuthorityChange::PublishPermit { .. }
            | AuthorityChange::PublishCredential { .. }
            | AuthorityChange::PublishCredentialConfiguration { .. }
            | AuthorityChange::PublishDelegationGrant { .. }
            | AuthorityChange::RevokeDelegationGrant { .. } => false,
        };
        if !allowed {
            return Err(CoordinationError::Restricted);
        }
        Ok(())
    }
}

impl AuthorityCoordinator {
    /// Persist a bounded intervention and its control event atomically. Check
    /// current authority, actor status, bounds and host time before replay and
    /// after every CAS conflict. Lost acknowledgments require a fresh evaluated
    /// stamp; an identical authorized replay observes the original event.
    pub async fn change_evaluated(
        &self,
        id: &str,
        change: AuthorityChange,
        reason: &str,
        authorization: ControlChangeAuthorization<'_>,
    ) -> Result<ChangeRecord, CoordinationError> {
        self.change_internal(
            id,
            change,
            authorization.ceiling.actor.id(),
            reason,
            Some(&authorization),
        )
        .await
    }
}
