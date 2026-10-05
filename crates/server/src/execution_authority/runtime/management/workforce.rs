//! Workforce operations reuse private scope authentication and the configured coordinator.
use super::{
    ExecutionAuthorityRuntime, ManagementError, ManagementObservation, public_effect, receipt,
};
use crate::auth::projection::AuthenticatedExecutionConfiguration;
use acteon_core::{
    GovernanceChangeReceipt, GovernanceLimits, GovernanceRoute, GovernanceRouteView,
    RepresentedParty,
    workforce::{
        WorkforceChange, WorkforceChangeRequest, WorkforceEntry, WorkforceManagementBounds,
        WorkforceMandateView, WorkforcePermitBindingView, WorkforceScopeView,
    },
};
use acteon_governance::{
    RootBudgetLimits,
    context::AcceptedEffect,
    workforce::{
        RepresentationMandate, WorkforceManagementAuthorization, WorkforceManagementCeiling,
        WorkforceMutation,
    },
};

fn bounds(
    observation: &ManagementObservation<'_>,
) -> Result<WorkforceManagementCeiling, ManagementError> {
    let policy = observation.policy;
    let workforce = policy
        .workforce
        .as_ref()
        .ok_or(ManagementError::Forbidden)?;
    let ceiling = WorkforceManagementCeiling {
        actor: policy.principal.clone(),
        teams: workforce.teams.clone(),
        principals: policy.subjects.clone(),
        job_classes: workforce.job_classes.clone(),
        effects: observation.effects.clone(),
        valid_from_ms: policy.valid_from_ms,
        limits: policy.limits.clone(),
        can_manage_roster: workforce.can_manage_roster,
        can_issue_mandates: workforce.can_issue_mandates,
        can_issue_permits: policy.can_issue_permits,
    };
    ceiling.validate()?;
    Ok(ceiling)
}
fn party_visible(bounds: &WorkforceManagementCeiling, party: &RepresentedParty) -> bool {
    match party {
        RepresentedParty::Human { principal } => bounds.principals.contains(principal),
        RepresentedParty::Team { team } => bounds.teams.contains(team),
    }
}
fn public_limits(limits: &RootBudgetLimits) -> GovernanceLimits {
    GovernanceLimits {
        max_units: limits.max_units,
        max_concurrent: limits.max_concurrent,
        deadline_ms: limits.deadline_ms,
    }
}
fn internal_limits(limits: &GovernanceLimits) -> RootBudgetLimits {
    RootBudgetLimits {
        max_units: limits.max_units,
        max_concurrent: limits.max_concurrent,
        deadline_ms: limits.deadline_ms,
    }
}
fn effects_for_routes(
    observation: &ManagementObservation<'_>,
    routes: &[GovernanceRoute],
) -> Result<Vec<AcceptedEffect>, ManagementError> {
    if routes.is_empty() || routes.len() > 128 {
        return Err(ManagementError::Invalid);
    }
    let definitions = observation.scope.prepared.catalog().definitions(
        &observation.scope.prepared.declaration().namespace,
        &observation.scope.prepared.declaration().tenant,
    );
    let mut seen = std::collections::BTreeSet::new();
    routes
        .iter()
        .map(|route| {
            if !seen.insert((&route.provider, &route.action_type)) {
                return Err(ManagementError::Invalid);
            }
            if !observation
                .policy
                .routes
                .iter()
                .any(|r| r.provider == route.provider && r.action_type == route.action_type)
            {
                return Err(ManagementError::Forbidden);
            }
            definitions
                .iter()
                .find(|d| d.provider == route.provider && d.action_type == route.action_type)
                .map(|d| d.effect.clone())
                .ok_or(ManagementError::Forbidden)
        })
        .collect()
}
fn mutation(
    observation: &ManagementObservation<'_>,
    change: WorkforceChange,
) -> Result<WorkforceMutation, ManagementError> {
    Ok(match change {
        WorkforceChange::PutTeam { team } => WorkforceMutation::PutTeam { team },
        WorkforceChange::DisbandTeam {
            team,
            expected_revision,
        } => WorkforceMutation::DisbandTeam {
            team,
            expected_revision,
        },
        WorkforceChange::PutMembership { membership } => {
            WorkforceMutation::PutMembership { membership }
        }
        WorkforceChange::RemoveMembership {
            id,
            expected_revision,
        } => WorkforceMutation::RemoveMembership {
            id,
            expected_revision,
        },
        WorkforceChange::PutOwnership { ownership } => {
            WorkforceMutation::PutOwnership { ownership }
        }
        WorkforceChange::PutAssignment { assignment } => {
            WorkforceMutation::PutAssignment { assignment }
        }
        WorkforceChange::RemoveAssignment {
            id,
            expected_revision,
        } => WorkforceMutation::RemoveAssignment {
            id,
            expected_revision,
        },
        WorkforceChange::RevokeMandate {
            id,
            expected_revision,
        } => WorkforceMutation::RevokeMandate {
            id,
            expected_revision,
        },
        WorkforceChange::PutMandate { mandate } => WorkforceMutation::PutMandate {
            mandate: RepresentationMandate {
                effects: effects_for_routes(observation, &mandate.routes)?,
                id: mandate.id,
                revision: mandate.revision,
                represented: mandate.represented,
                actor: mandate.actor,
                job_class: mandate.job_class,
                eligible_initiators: mandate.eligible_initiators,
                ownership: mandate.ownership,
                dependencies: mandate.dependencies,
                valid_from_ms: mandate.valid_from_ms,
                limits: internal_limits(&mandate.limits),
            },
        },
        WorkforceChange::PublishRepresentedPermit { permit, mandate } => {
            WorkforceMutation::PublishRepresentedPermit {
                permit: acteon_governance::permit::ExecutionPermit {
                    effects: effects_for_routes(observation, &permit.routes)?,
                    id: permit.id,
                    revision: permit.revision,
                    subject: permit.subject,
                    valid_from_ms: permit.valid_from_ms,
                    limits: internal_limits(&permit.limits),
                },
                mandate,
            }
        }
    })
}
impl ExecutionAuthorityRuntime {
    pub async fn change_workforce(
        &self,
        request: WorkforceChangeRequest,
        authentication: &AuthenticatedExecutionConfiguration,
    ) -> Result<GovernanceChangeReceipt, ManagementError> {
        let observation = self
            .management_observation(&request.namespace, &request.tenant, authentication)
            .await?;
        let ceiling = bounds(&observation)?;
        let mutation = mutation(&observation, request.change)?;
        let record = observation
            .scope
            .coordinator
            .change_workforce(
                &request.change_id,
                mutation,
                &request.reason,
                WorkforceManagementAuthorization {
                    ceiling: &ceiling,
                    evaluated_authority: &observation.stamp,
                    clock: self.clock.as_ref(),
                },
            )
            .await?;
        Ok(receipt(
            request.namespace,
            request.tenant,
            request.change_id,
            record,
        ))
    }

    pub async fn inspect_workforce(
        &self,
        namespace: &str,
        tenant: &str,
        authentication: &AuthenticatedExecutionConfiguration,
    ) -> Result<WorkforceScopeView, ManagementError> {
        let observation = self
            .management_observation(namespace, tenant, authentication)
            .await?;
        let ceiling = bounds(&observation)?;
        let state = observation.scope.coordinator.snapshot().await?;
        if state.stamp() != observation.stamp {
            return Err(ManagementError::Conflict);
        }
        let now = self.clock.now().timestamp_millis();
        if now < ceiling.valid_from_ms || now >= ceiling.limits.deadline_ms {
            return Err(ManagementError::Forbidden);
        }
        authentication
            .management_scope(namespace, tenant)
            .map_err(|_| ManagementError::Forbidden)?
            .verify_management_snapshot(&state, now)
            .map_err(|_| ManagementError::Forbidden)?;
        let workforce = &state.workforce;
        let VisibleRoster {
            teams,
            memberships,
            ownership,
            assignments,
        } = visible_roster(workforce, &ceiling);
        let mandates: Vec<_> = workforce
            .mandates
            .values()
            .filter(|r| mandate_visible(&ceiling, &r.value))
            .map(|r| WorkforceEntry {
                value: public_mandate(&r.value),
                revoked: r.revoked,
            })
            .collect();
        let permit_bindings = workforce
            .permit_bindings
            .values()
            .filter(|b| {
                mandates.iter().any(|m| m.value.id == b.mandate.id)
                    && state.permits.get(&b.permit_id).is_some_and(|p| {
                        ceiling.principals.contains(&p.permit.subject)
                            && p.permit.effects.iter().all(|e| {
                                ceiling
                                    .effects
                                    .iter()
                                    .any(|c| acteon_governance::permit::matches_effect(c, e))
                            })
                    })
            })
            .map(|b| WorkforcePermitBindingView {
                permit_id: b.permit_id.clone(),
                permit_revision: b.permit_revision,
                mandate: b.mandate.clone(),
            })
            .collect();
        let routes = observation
            .scope
            .prepared
            .catalog()
            .definitions(namespace, tenant)
            .into_iter()
            .filter(|d| observation.effects.contains(&d.effect))
            .map(|d| GovernanceRouteView {
                route: GovernanceRoute {
                    provider: d.provider,
                    action_type: d.action_type,
                },
                closed: d
                    .effect
                    .resources
                    .iter()
                    .any(|r| state.closed_resources.contains(r)),
                effect: public_effect(&d.effect),
            })
            .collect();
        Ok(WorkforceScopeView {
            namespace: namespace.into(),
            tenant: tenant.into(),
            incarnation: state.incarnation,
            generation: state.generation,
            management: WorkforceManagementBounds {
                teams: ceiling.teams,
                principals: ceiling.principals,
                job_classes: ceiling.job_classes,
                can_manage_roster: ceiling.can_manage_roster,
                can_issue_mandates: ceiling.can_issue_mandates,
                can_issue_permits: ceiling.can_issue_permits,
                valid_from_ms: ceiling.valid_from_ms,
                limits: public_limits(&ceiling.limits),
            },
            routes,
            teams,
            memberships,
            ownership,
            assignments,
            mandates,
            permit_bindings,
        })
    }
}
fn mandate_visible(bounds: &WorkforceManagementCeiling, mandate: &RepresentationMandate) -> bool {
    party_visible(bounds, &mandate.represented)
        && bounds.principals.contains(&mandate.actor)
        && bounds.job_classes.contains(&mandate.job_class)
        && mandate
            .eligible_initiators
            .iter()
            .all(|p| bounds.principals.contains(p))
        && mandate.effects.iter().all(|e| {
            bounds
                .effects
                .iter()
                .any(|c| acteon_governance::permit::matches_effect(c, e))
        })
}
fn public_mandate(mandate: &RepresentationMandate) -> WorkforceMandateView {
    WorkforceMandateView {
        id: mandate.id.clone(),
        revision: mandate.revision,
        represented: mandate.represented.clone(),
        actor: mandate.actor.clone(),
        job_class: mandate.job_class.clone(),
        eligible_initiators: mandate.eligible_initiators.clone(),
        ownership: mandate.ownership.clone(),
        dependencies: mandate.dependencies.clone(),
        effects: mandate.effects.iter().map(public_effect).collect(),
        valid_from_ms: mandate.valid_from_ms,
        limits: public_limits(&mandate.limits),
    }
}

struct VisibleRoster {
    teams: Vec<WorkforceEntry<acteon_core::WorkforceTeam>>,
    memberships: Vec<WorkforceEntry<acteon_core::WorkforceMembership>>,
    ownership: Vec<WorkforceEntry<acteon_core::AgentOwnership>>,
    assignments: Vec<WorkforceEntry<acteon_core::WorkforceAssignment>>,
}
fn visible_roster(
    workforce: &acteon_governance::workforce::WorkforceState,
    ceiling: &WorkforceManagementCeiling,
) -> VisibleRoster {
    let teams = workforce
        .teams
        .values()
        .filter(|r| ceiling.teams.contains(&r.value.team))
        .map(|r| WorkforceEntry {
            value: r.value.clone(),
            revoked: r.revoked,
        })
        .collect();
    let memberships = workforce
        .memberships
        .values()
        .filter(|r| {
            ceiling.teams.contains(&r.value.team) && ceiling.principals.contains(&r.value.human)
        })
        .map(|r| WorkforceEntry {
            value: r.value.clone(),
            revoked: r.revoked,
        })
        .collect();
    let ownership = workforce
        .ownership
        .values()
        .filter(|r| {
            ceiling.principals.contains(&r.value.agent) && party_visible(ceiling, &r.value.owner)
        })
        .map(|r| WorkforceEntry {
            value: r.value.clone(),
            revoked: r.revoked,
        })
        .collect();
    let assignments = workforce
        .assignments
        .values()
        .filter(|r| {
            ceiling.teams.contains(&r.value.team)
                && ceiling.principals.contains(&r.value.agent)
                && r.value
                    .job_classes
                    .iter()
                    .all(|c| ceiling.job_classes.contains(c))
        })
        .map(|r| WorkforceEntry {
            value: r.value.clone(),
            revoked: r.revoked,
        })
        .collect();
    VisibleRoster {
        teams,
        memberships,
        ownership,
        assignments,
    }
}
