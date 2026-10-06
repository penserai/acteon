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
    grant: Option<crate::delegation_policy::DelegationGrantReference>,
    authority: AuthorityStamp,
    target: PrincipalIdentity,
    checked_at_ms: i64,
}
impl DelegationEligibility {
    #[must_use]
    pub fn grant(&self) -> Option<&crate::delegation_policy::DelegationGrantReference> {
        self.grant.as_ref()
    }
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
    /// Refuse a disallowed source before a host looks up recipient authority.
    /// This is a read-only preflight; combined eligibility still checks both
    /// participants again after registry and recipient reads.
    pub async fn check_delegation_source(
        &self,
        context: &VerifiedExecutionContext,
        permits: &[PermitReference],
        effect: &AcceptedEffect,
        clock: &dyn Clock,
    ) -> Result<(), CoordinationError> {
        let state = self.snapshot().await?;
        check_participant(
            &state,
            context,
            permits,
            effect,
            clock,
            clock.now().timestamp_millis(),
        )
    }
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
            check_participant(&state, context, permits, request.effect, request.clock, now)?;
        }
        Ok(DelegationEligibility {
            grant: None,
            authority: state.stamp(),
            target: request.target.clone(),
            checked_at_ms: now,
        })
    }
}

fn check_participant(
    state: &crate::CoordinatorSnapshot,
    context: &VerifiedExecutionContext,
    permits: &[PermitReference],
    effect: &AcceptedEffect,
    clock: &dyn Clock,
    now: i64,
) -> Result<(), CoordinationError> {
    if context.credential_authority().is_none() {
        return Err(CoordinationError::Restricted);
    }
    context.validate_inheritance(state, permits, now)?;
    let reference = context
        .reference()
        .map_err(|_| CoordinationError::Restricted)?;
    crate::permit::evaluate(
        state,
        &PermittedAttempt {
            id: "delegation-discovery",
            context,
            permits,
            effect,
            request_digest: reference.request_digest(),
            units: 1,
            clock,
        },
        now,
    )?;
    let resources = context
        .effect_registration_resources(effect)
        .map_err(|_| CoordinationError::Restricted)?;
    if resources.iter().any(|r| state.closed_resources.contains(r)) {
        return Err(CoordinationError::Restricted);
    }
    check_root_reservation(
        state,
        &RootReservation {
            root_id: context.execution_id().to_string(),
            units: 1,
        },
        now,
    )?;
    Ok(())
}

/// Complete operator-qualified service plan. Descriptive registry claims cannot
/// supply this host binding or establish a caller's accepted grant.
pub struct ServiceDiscoveryBinding<'a> {
    pub target: &'a PrincipalIdentity,
    pub agent_resource: &'a ResourceRef,
    pub binding_digest: &'a str,
    pub skill: &'a str,
    pub ingress: &'a AcceptedEffect,
    pub intent: &'a [AcceptedEffect],
    pub direct_effects: &'a [AcceptedEffect],
}

pub struct ServiceDelegationDiscoveryRequest<'a> {
    pub parent: &'a VerifiedExecutionContext,
    pub parent_permits: &'a [PermitReference],
    /// Independently authenticated preview; never reused as recipient acceptance.
    pub recipient: &'a VerifiedExecutionContext,
    pub recipient_permits: &'a [PermitReference],
    pub binding: ServiceDiscoveryBinding<'a>,
    pub clock: &'a dyn Clock,
}

impl AuthorityCoordinator {
    /// Refuse missing or retired service grants before private recipient lookup.
    pub async fn check_service_delegation_source(
        &self,
        parent: &VerifiedExecutionContext,
        permits: &[PermitReference],
        binding: ServiceDiscoveryBinding<'_>,
        clock: &dyn Clock,
    ) -> Result<crate::delegation_policy::DelegationGrantReference, CoordinationError> {
        let state = self.snapshot().await?;
        check_service_source(
            &state,
            parent,
            permits,
            &binding,
            clock,
            clock.now().timestamp_millis(),
        )
    }

    /// Source ingress and recipient private operations are checked independently
    /// at one current snapshot. Discovery writes nothing and allocates no budget.
    pub async fn discover_service_delegation_eligibility(
        &self,
        request: ServiceDelegationDiscoveryRequest<'_>,
    ) -> Result<DelegationEligibility, CoordinationError> {
        let state = self.snapshot().await?;
        let now = request.clock.now().timestamp_millis();
        let grant = check_service_source(
            &state,
            request.parent,
            request.parent_permits,
            &request.binding,
            request.clock,
            now,
        )?;
        if request.recipient.principal() != request.binding.target {
            return Err(CoordinationError::Restricted);
        }
        for effect in request.binding.direct_effects {
            check_participant(
                &state,
                request.recipient,
                request.recipient_permits,
                effect,
                request.clock,
                now,
            )?;
        }
        Ok(DelegationEligibility {
            grant: Some(grant),
            authority: state.stamp(),
            target: request.binding.target.clone(),
            checked_at_ms: now,
        })
    }
}

fn check_service_source(
    state: &crate::CoordinatorSnapshot,
    parent: &VerifiedExecutionContext,
    permits: &[PermitReference],
    binding: &ServiceDiscoveryBinding<'_>,
    clock: &dyn Clock,
    now: i64,
) -> Result<crate::delegation_policy::DelegationGrantReference, CoordinationError> {
    use crate::permit::{matches_effect, valid_effects};
    if binding.target.kind() != PrincipalKind::Agent
        || parent.principal() == binding.target
        || binding.agent_resource.kind() != ResourceKind::Agent
        || binding.agent_resource.namespace() != state.namespace
        || binding.agent_resource.tenant() != state.tenant
        || binding.ingress.operation != "agent.invoke"
        || !binding.ingress.resources.contains(binding.agent_resource)
        || !valid_effects(std::slice::from_ref(binding.ingress))
        || !valid_effects(binding.intent)
        || !valid_effects(binding.direct_effects)
        || binding
            .direct_effects
            .iter()
            .any(|e| !binding.intent.iter().any(|a| matches_effect(a, e)))
        || binding
            .intent
            .iter()
            .flat_map(|e| &e.resources)
            .any(|r| !binding.ingress.resources.contains(r) || state.closed_resources.contains(r))
    {
        return Err(CoordinationError::Restricted);
    }
    check_participant(state, parent, permits, binding.ingress, clock, now)?;
    let path = crate::budget::budget_path(state, &parent.execution_id().to_string())?;
    if path.len() >= crate::MAX_BUDGET_DEPTH
        || path
            .iter()
            .any(|id| state.roots[id].owner_subject == binding.target.id())
    {
        return Err(CoordinationError::Restricted);
    }
    for reference in parent.accepted_delegation_grants() {
        let Some(original) = crate::delegation_policy::original(state, reference) else {
            continue;
        };
        if original.target == *binding.target
            && original.agent_resource == *binding.agent_resource
            && original.binding_digest == binding.binding_digest
            && original.skill == binding.skill
            && matches_effect(&original.ingress_effect, binding.ingress)
            && parent
                .check_service_intent(state, reference, binding.intent, now)
                .is_ok()
        {
            return Ok(reference.clone());
        }
    }
    Err(CoordinationError::Restricted)
}
