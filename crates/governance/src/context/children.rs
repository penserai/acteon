//! Host-qualified same-actor continuation; cross-principal delegation is separate.
use super::AcceptedEffect;
use super::ExecutionContextHandle;
use super::{
    ContextBinding, ContextError, ContextRecord, FORMAT, TrustedContextStore,
    VerifiedExecutionContext, valid_digest,
};
use crate::permit::PermitReference;
use crate::{ChildBudgetAdmission, RootBudgetLimits};
use crate::{CoordinationError, valid_text};
use acteon_core::{ExecutionContextReference, ResourceRef};
use acteon_state::{KeyKind, StateKey};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use sha2::Sha256;
use uuid::Uuid;

pub const CHILD_ADMISSION_KIND: &str = "governance_child_admission";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChildLineage {
    parent: ExecutionContextReference,
    root_execution_id: Uuid,
    depth: usize,
    limits: RootBudgetLimits,
    admission_digest: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    restrictions: Vec<ResourceRef>,
}
impl ChildLineage {
    pub(super) fn valid_shape(&self, record: &ContextRecord) -> bool {
        !self.root_execution_id.is_nil()
            && self.root_execution_id != record.execution_id
            && self.depth > 0
            && self.depth < crate::budget::MAX_BUDGET_DEPTH
            && self.parent.execution_id() != record.execution_id
            && self.parent.namespace() == record.namespace
            && self.parent.tenant() == record.tenant
            && self.parent.principal() == &record.principal
            && self.limits.deadline_ms == record.deadline_ms
            && self.limits.max_units > 0
            && self.limits.max_concurrent > 0
            && self.restrictions.len() <= crate::MAX_ATTEMPT_RESOURCES
            && self
                .restrictions
                .iter()
                .all(|r| r.namespace() == record.namespace && r.tenant() == record.tenant)
            && self
                .restrictions
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == self.restrictions.len()
            && valid_digest(&self.admission_digest)
            && (self.depth != 1 || self.parent.execution_id() == self.root_execution_id)
    }
}

/// Actual child input/effects must be qualified by the trusted workflow adapter.
/// No actor, represented party or payer is accepted from a public child body.
pub struct ChildContextAdmission<'a> {
    pub admission_key: &'a str,
    pub parent: &'a VerifiedExecutionContext,
    pub handle: ExecutionContextHandle,
    pub execution_id: Uuid,
    pub request_digest: String,
    pub accepted_effects: Vec<AcceptedEffect>,
    /// Additional enclosing resources derived from the accepted plan. These
    /// only restrict effects; they grant no operation/resource permission.
    pub restrictions: Vec<ResourceRef>,
    pub permits: &'a [PermitReference],
    pub limits: RootBudgetLimits,
    pub clock: &'a dyn acteon_time::Clock,
}
impl VerifiedExecutionContext {
    #[must_use]
    pub fn root_execution_id(&self) -> Uuid {
        self.0
            .lineage
            .as_ref()
            .map_or(self.0.execution_id, |l| l.root_execution_id)
    }
    #[must_use]
    pub fn parent_reference(&self) -> Option<&ExecutionContextReference> {
        self.0.lineage.as_ref().map(|l| &l.parent)
    }
    /// Signed enclosing-resource closures that apply to every descendant effect.
    #[must_use]
    pub fn restriction_resources(&self) -> &[ResourceRef] {
        self.0
            .lineage
            .as_ref()
            .map_or(&[], |lineage| &lineage.restrictions)
    }
    /// Complete resources checked for closures and retained in start evidence.
    /// Combining these sets does not establish effect permission.
    pub fn effect_registration_resources(
        &self,
        effect: &AcceptedEffect,
    ) -> Result<Vec<ResourceRef>, ContextError> {
        let resources: std::collections::BTreeSet<_> = effect
            .resources
            .iter()
            .chain(self.restriction_resources())
            .cloned()
            .collect();
        if resources.is_empty() || resources.len() > crate::MAX_ATTEMPT_RESOURCES {
            return Err(ContextError::Invalid);
        }
        Ok(resources.into_iter().collect())
    }
    pub(crate) fn validate_budget_binding(
        &self,
        state: &crate::CoordinatorSnapshot,
    ) -> Result<(), CoordinationError> {
        let id = self.execution_id().to_string();
        let Some(lineage) = &self.0.lineage else {
            return if state.budget_parents.contains_key(&id) {
                Err(CoordinationError::Conflict)
            } else {
                Ok(())
            };
        };
        let path = crate::budget::budget_path(state, &id)?;
        if path.get(1) != Some(&lineage.parent.execution_id().to_string())
            || path.last() != Some(&lineage.root_execution_id.to_string())
            || path.len() != lineage.depth + 1
            || state.roots[&id].limits != lineage.limits
            || path
                .iter()
                .any(|id| state.roots[id].owner_subject != self.principal().id())
        {
            return Err(CoordinationError::Conflict);
        }
        Ok(())
    }
}
impl TrustedContextStore {
    pub async fn capture_child(
        &self,
        request: ChildContextAdmission<'_>,
    ) -> Result<VerifiedExecutionContext, ContextError> {
        let now = request.clock.now().timestamp_millis();
        self.validate(&request.parent.0)?;
        let state = self.coordinator.snapshot().await?;
        request
            .parent
            .validate_inheritance(&state, request.permits, now)?;
        if request.accepted_effects.is_empty()
            || request
                .accepted_effects
                .iter()
                .any(|e| !request.parent.within_accepted_ceiling(e))
        {
            return Err(ContextError::Invalid);
        }
        let parent_limits = &state.roots[&request.parent.execution_id().to_string()].limits;
        if request.limits.max_units > parent_limits.max_units
            || request.limits.max_concurrent > parent_limits.max_concurrent
            || request.limits.deadline_ms > request.parent.deadline_ms()
        {
            return Err(ContextError::Invalid);
        }
        let restrictions = Self::child_restrictions(request.parent, &request.restrictions)?;
        let (key, digest) = self.child_admission_key(request.admission_key)?;
        let mut proposed = request.parent.0.clone();
        proposed.schema_version = FORMAT;
        proposed.handle = request.handle;
        proposed.execution_id = request.execution_id;
        proposed.request_digest = request.request_digest;
        proposed.accepted_effects = request.accepted_effects;
        proposed.deadline_ms = request.limits.deadline_ms;
        proposed.admitted_at_ms = now;
        proposed.authority = state.stamp();
        proposed.lineage = Some(ChildLineage {
            parent: request.parent.reference()?,
            root_execution_id: request.parent.root_execution_id(),
            depth: request.parent.0.lineage.as_ref().map_or(1, |l| l.depth + 1),
            limits: request.limits,
            admission_digest: digest,
            restrictions,
        });
        self.validate(&proposed)?;
        let encoded = self.seal_record(&proposed)?;
        let stored = if let Some(original) = self.store.get(&key).await? {
            original
        } else if self.store.check_and_set(&key, &encoded, None).await? {
            encoded
        } else {
            self.store.get(&key).await?.ok_or(ContextError::Missing)?
        };
        let original = self.open_record(&stored)?;
        Self::match_child(&original, &proposed)?;
        if now < original.admitted_at_ms || now >= original.deadline_ms {
            return Err(ContextError::Expired);
        }
        let state = self.coordinator.snapshot().await?;
        request.parent.validate_inheritance(
            &state,
            request.permits,
            request.clock.now().timestamp_millis(),
        )?;
        let context = self
            .recover_child_record(&original, &state, request.clock)
            .await?;
        self.coordinator
            .create_child_budget(ChildBudgetAdmission {
                parent: request.parent,
                permits: request.permits,
                execution_id: original.execution_id,
                limits: original
                    .lineage
                    .as_ref()
                    .ok_or(ContextError::Invalid)?
                    .limits
                    .clone(),
                clock: request.clock,
            })
            .await?;
        Ok(context)
    }
    fn child_restrictions(
        parent: &VerifiedExecutionContext,
        additions: &[ResourceRef],
    ) -> Result<Vec<ResourceRef>, ContextError> {
        if additions.iter().any(|resource| {
            !parent.restriction_resources().contains(resource)
                && !parent
                    .0
                    .accepted_effects
                    .iter()
                    .any(|e| e.resources.contains(resource))
        }) {
            return Err(ContextError::Invalid);
        }
        let resources: std::collections::BTreeSet<_> = parent
            .restriction_resources()
            .iter()
            .chain(additions)
            .cloned()
            .collect();
        if resources.len() > crate::MAX_ATTEMPT_RESOURCES {
            return Err(ContextError::Invalid);
        }
        Ok(resources.into_iter().collect())
    }
    async fn recover_child_record(
        &self,
        original: &ContextRecord,
        state: &crate::CoordinatorSnapshot,
        clock: &dyn acteon_time::Clock,
    ) -> Result<VerifiedExecutionContext, ContextError> {
        let binding = ContextBinding {
            execution_id: original.execution_id,
            principal: original.principal.clone(),
            request_digest: original.request_digest.clone(),
        };
        Ok(
            match self
                .recover(&original.handle, &binding, clock.now().timestamp_millis())
                .await
            {
                Ok(existing) => {
                    let mut expected = original.clone();
                    expected.authority = existing.0.authority.clone();
                    Self::check_replay(existing, &expected)?
                }
                Err(ContextError::Missing) => {
                    let mut restored = original.clone();
                    restored.authority = state.stamp();
                    self.persist_record(restored, clock.now().timestamp_millis())
                        .await?
                }
                Err(error) => return Err(error),
            },
        )
    }
    fn child_admission_key(&self, admission_key: &str) -> Result<(StateKey, String), ContextError> {
        if !valid_text(admission_key) {
            return Err(ContextError::Invalid);
        }
        let bytes = serde_json::to_vec(&(self.domain.as_str(), admission_key))
            .map_err(|_| ContextError::Invalid)?;
        let digest = format!("{:x}", Sha256::digest(bytes));
        Ok((
            StateKey::new(
                self.coordinator.key.namespace.as_str(),
                self.coordinator.key.tenant.as_str(),
                KeyKind::Custom(CHILD_ADMISSION_KIND.into()),
                &digest,
            ),
            digest,
        ))
    }
    fn match_child(original: &ContextRecord, proposed: &ContextRecord) -> Result<(), ContextError> {
        if original.lineage.is_none() {
            return Err(ContextError::Verification);
        }
        let mut expected = original.clone();
        expected.handle = proposed.handle.clone();
        expected.execution_id = proposed.execution_id;
        expected.admitted_at_ms = proposed.admitted_at_ms;
        expected.authority = proposed.authority.clone();
        expected.deadline_ms = proposed.deadline_ms;
        expected
            .lineage
            .as_mut()
            .ok_or(ContextError::Invalid)?
            .limits
            .deadline_ms = proposed.deadline_ms;
        if expected != *proposed {
            return Err(ContextError::Conflict);
        }
        Ok(())
    }
}
