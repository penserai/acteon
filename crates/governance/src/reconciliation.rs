//! Independently authorized acceptance of finality evidence. This grants no
//! permission to call a provider, retry an operation, or refund spent budget.
use std::collections::BTreeSet;

use acteon_core::{PrincipalIdentity, ResourceRef};
use acteon_time::Clock;

use crate::{
    AttemptEvidenceReference, AttemptReconciliationReference, AuthorityCoordinator, AuthorityStamp,
    CoordinationError, CoordinatorSnapshot, OriginalEvidenceExpectation, ScopePurpose,
};

/// Durable attribution of the original evaluated acceptance. Recorded by the
/// coordinator, never copied from a request or rewritten by authorized replay.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconciliationAcceptance {
    pub operator: PrincipalIdentity,
    pub authority: AuthorityStamp,
    /// Time of the authorization decision for the successful settlement CAS.
    pub accepted_at_ms: i64,
}

/// Trusted-host bounds, deliberately not deserializable. The host must verify
/// the original signed operation's full principal identity before deriving the
/// subject IDs: legacy start records retain IDs, not principal kinds.
pub struct ReconciliationCeiling {
    pub actor: PrincipalIdentity,
    pub subjects: Vec<String>,
    pub resources: Vec<ResourceRef>,
    pub valid_from_ms: i64,
    pub deadline_ms: i64,
}

/// Host-controlled freshness check for independent authentication or deployment
/// policy. It is evaluated before staging and on every settlement CAS attempt,
/// including replay. It cannot make separate authority records transactional;
/// the execution coordinator's generation remains the atomic effect fence.
#[async_trait::async_trait]
pub trait ReconciliationAuthorityGuard: Send + Sync {
    async fn check_current(&self) -> Result<(), CoordinationError>;
}

/// Derived from independent management policy and current authentication. A
/// stale stamp requires fresh host evaluation; it cannot simply be refreshed.
/// The host must separately validate qualified finality evidence and bind it to
/// the immutable operation, attempt token and original evidence.
pub struct ReconciliationAuthorization<'a> {
    pub ceiling: &'a ReconciliationCeiling,
    pub evaluated_authority: &'a AuthorityStamp,
    pub clock: &'a dyn Clock,
    pub guard: Option<&'a dyn ReconciliationAuthorityGuard>,
}

impl ReconciliationAuthorization<'_> {
    pub(crate) async fn validate_current(
        &self,
        coordinator: &AuthorityCoordinator,
        state: &CoordinatorSnapshot,
        attempt_id: &str,
    ) -> Result<i64, CoordinationError> {
        if let Some(guard) = self.guard {
            guard.check_current().await?;
        }
        self.validate(coordinator, state, attempt_id)
    }

    pub(crate) fn validate(
        &self,
        coordinator: &AuthorityCoordinator,
        state: &CoordinatorSnapshot,
        attempt_id: &str,
    ) -> Result<i64, CoordinationError> {
        let ceiling = self.ceiling;
        if ceiling.subjects.is_empty()
            || ceiling.subjects.len() > 128
            || ceiling.resources.is_empty()
            || ceiling.resources.len() > 128
            || ceiling.subjects.iter().any(|id| !crate::valid_text(id))
            || ceiling.subjects.iter().collect::<BTreeSet<_>>().len() != ceiling.subjects.len()
            || ceiling.resources.iter().collect::<BTreeSet<_>>().len() != ceiling.resources.len()
            || ceiling.valid_from_ms < 0
            || ceiling.deadline_ms <= ceiling.valid_from_ms
        {
            return Err(CoordinationError::Invalid("reconciliation ceiling".into()));
        }
        for resource in &ceiling.resources {
            coordinator.validate_resource_scope(resource)?;
        }
        if state.purpose != ScopePurpose::Execution {
            return Err(CoordinationError::Restricted);
        }
        if state.stamp() != *self.evaluated_authority {
            return Err(CoordinationError::StaleAuthority);
        }
        let now = self.clock.now().timestamp_millis();
        if now < ceiling.valid_from_ms
            || now >= ceiling.deadline_ms
            || state.revoked_subjects.contains(ceiling.actor.id())
        {
            return Err(CoordinationError::Restricted);
        }
        let record = state
            .starts
            .get(attempt_id)
            .ok_or(CoordinationError::Conflict)?;
        if !ceiling.subjects.contains(&record.subject)
            || !record
                .resources
                .iter()
                .all(|resource| ceiling.resources.contains(resource))
        {
            return Err(CoordinationError::Restricted);
        }
        Ok(now)
    }
}

impl AuthorityCoordinator {
    /// Advisory preflight against this coordinator's current state. Acceptance
    /// must still use an evaluated CAS; this check does not issue a capability.
    pub async fn check_reconciliation_authorization(
        &self,
        id: &str,
        authorization: &ReconciliationAuthorization<'_>,
    ) -> Result<(), CoordinationError> {
        let (state, _) = self.load().await?;
        authorization
            .validate_current(self, &state, id)
            .await
            .map(|_| ())
    }

    /// Atomically retain a verified original uncertain result whose earlier
    /// acknowledgment was lost, together with the finality link. The host must
    /// validate original evidence before this call and refuse known results.
    /// Existing pins cannot be replaced; a competing acknowledgment conflicts.
    pub async fn reconcile_attempt_evaluated_with_original(
        &self,
        id: &str,
        token: &str,
        expected_original: Option<AttemptEvidenceReference>,
        reconciliation: AttemptReconciliationReference,
        authorization: ReconciliationAuthorization<'_>,
    ) -> Result<(), CoordinationError> {
        if expected_original
            .as_ref()
            .is_some_and(|reference| !crate::valid_evidence(reference))
        {
            return Err(CoordinationError::Invalid("original evidence".into()));
        }
        self.reconcile_attempt_internal(
            id,
            token,
            reconciliation,
            Some(&authorization),
            OriginalEvidenceExpectation::Observed(expected_original),
        )
        .await
    }

    /// Accept a verifier's immutable finality link within independently granted
    /// operator bounds. Revalidate before idempotent replay and every CAS retry.
    /// Resource closures block new effects, not evidence about past effects;
    /// their generation change still invalidates an earlier evaluation.
    pub async fn reconcile_attempt_evaluated(
        &self,
        id: &str,
        token: &str,
        reconciliation: AttemptReconciliationReference,
        authorization: ReconciliationAuthorization<'_>,
    ) -> Result<(), CoordinationError> {
        self.reconcile_attempt_internal(
            id,
            token,
            reconciliation,
            Some(&authorization),
            OriginalEvidenceExpectation::Proposed,
        )
        .await
    }
}
