//! Advisory delegation discovery from independently verified participant contexts.
//!
//! A candidate is not a child context, reservation, lease or permission to send.
//! Trusted registry adapters must qualify the actual peer/skill/endpoint binding;
//! child admission and every external effect must evaluate authority again.
use acteon_core::{PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef};
use acteon_time::Clock;

use crate::{
    AuthorityCoordinator, AuthorityStamp, CoordinationError, RootReservation,
    budget::check_root_reservation,
    context::{AcceptedEffect, VerifiedExecutionContext},
    permit::{PermitReference, PermittedAttempt},
};

/// Identity and effects come from a trusted approved registry binding, not a
/// model's URL or claimed recipient. Both contexts require credentialed capture.
/// They are independently admitted participant ceilings, not delegated lineage.
pub struct DelegationDiscoveryRequest<'a> {
    pub parent: &'a VerifiedExecutionContext,
    pub parent_permits: &'a [PermitReference],
    pub recipient: &'a VerifiedExecutionContext,
    pub recipient_permits: &'a [PermitReference],
    pub target: &'a PrincipalIdentity,
    pub agent_resource: &'a ResourceRef,
    /// Complete qualified ingress effect; must explicitly include the agent.
    pub effect: &'a AcceptedEffect,
    pub clock: &'a dyn Clock,
}

/// Advisory evaluation at one coordinator snapshot. No `Deserialize`: request
/// metadata cannot create this host result or turn it into execution authority.
#[derive(Debug, Clone)]
pub struct DelegationEligibility {
    authority: AuthorityStamp,
    target: PrincipalIdentity,
    checked_at_ms: i64,
}
impl DelegationEligibility {
    #[must_use]
    pub fn authority(&self) -> &AuthorityStamp {
        &self.authority
    }
    #[must_use]
    pub fn target(&self) -> &PrincipalIdentity {
        &self.target
    }
    #[must_use]
    pub const fn checked_at_ms(&self) -> i64 {
        self.checked_at_ms
    }
}

impl AuthorityCoordinator {
    /// Evaluate both original accepted ceilings and both current participant
    /// entitlements against this coordinator's configured `StateStore`. This
    /// writes nothing, including when a candidate is refused. Budget availability
    /// is advisory and is never deducted here. A later acceptance must implement
    /// cross-principal provenance and atomic sponsorship, not reuse either root.
    pub async fn discover_delegation_eligibility(
        &self,
        request: DelegationDiscoveryRequest<'_>,
    ) -> Result<DelegationEligibility, CoordinationError> {
        let state = self.snapshot().await?;
        // Sample after the backend read; a delayed snapshot cannot freeze expiry.
        let now = request.clock.now().timestamp_millis();
        if request.target.kind() != PrincipalKind::Agent
            || request.recipient.principal() != request.target
            || request.parent.principal() == request.target
            || request.parent.credential_authority().is_none()
            || request.recipient.credential_authority().is_none()
            || request.agent_resource.kind() != ResourceKind::Agent
            || !request.effect.resources.contains(request.agent_resource)
        {
            return Err(CoordinationError::Restricted);
        }
        self.validate_resource_scope(request.agent_resource)?;
        let parent_path =
            crate::budget::budget_path(&state, &request.parent.execution_id().to_string())?;
        // Discovery never proposes a cycle through a participant already paying
        // for this branch. Explicit bounded revisits require a separate policy.
        if parent_path
            .iter()
            .any(|id| state.roots[id].owner_subject == request.target.id())
        {
            return Err(CoordinationError::Restricted);
        }
        for (context, permits) in [
            (request.parent, request.parent_permits),
            (request.recipient, request.recipient_permits),
        ] {
            context.validate_inheritance(&state, permits, now)?;
            let reference = context
                .reference()
                .map_err(|_| CoordinationError::Restricted)?;
            crate::permit::evaluate(
                &state,
                &PermittedAttempt {
                    id: "delegation-discovery",
                    context,
                    permits,
                    effect: request.effect,
                    request_digest: reference.request_digest(),
                    units: 1,
                    clock: request.clock,
                },
                now,
            )?;
            let resources = context
                .effect_registration_resources(request.effect)
                .map_err(|_| CoordinationError::Restricted)?;
            if resources.iter().any(|r| state.closed_resources.contains(r)) {
                return Err(CoordinationError::Restricted);
            }
            check_root_reservation(
                &state,
                &RootReservation {
                    root_id: context.execution_id().to_string(),
                    units: 1,
                },
                now,
            )?;
        }
        Ok(DelegationEligibility {
            authority: state.stamp(),
            target: request.target.clone(),
            checked_at_ms: now,
        })
    }
}
