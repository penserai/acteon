//! Management from original private authentication and independent deployment policy.
mod workforce;
use super::{ExecutionAuthorityRuntime, InstalledScope};
use crate::{
    auth::projection::AuthenticatedExecutionConfiguration, config::ExecutionManagerConfig,
};
use acteon_core::{
    GovernanceChangeReceipt, GovernanceEffect, GovernanceIntervention,
    GovernanceInterventionRequest, GovernanceLimits, GovernancePermitView, GovernanceRoute,
    GovernanceRouteView, GovernanceScopeView, PublishGovernancePermitRequest,
};
use acteon_governance::{
    AuthorityChange, AuthorityStamp, CoordinationError,
    control::{ControlChangeAuthorization, ControlChangeCeiling},
    permit::{EvaluatedPermitPublication, ExecutionPermit, PermitIssuanceCeiling},
};

#[derive(Debug, thiserror::Error)]
pub enum ManagementError {
    #[error("current management authority is required")]
    Forbidden,
    #[error("invalid governance request")]
    Invalid,
    #[error("governance state changed; evaluate authority again")]
    Conflict,
    #[error("governance state unavailable")]
    Unavailable,
}
impl From<CoordinationError> for ManagementError {
    fn from(error: CoordinationError) -> Self {
        match error {
            CoordinationError::Invalid(_) => Self::Invalid,
            CoordinationError::Conflict | CoordinationError::StaleAuthority => Self::Conflict,
            CoordinationError::Restricted | CoordinationError::PermitDenied(_) => Self::Forbidden,
            _ => Self::Unavailable,
        }
    }
}
struct ManagementObservation<'a> {
    scope: &'a InstalledScope,
    policy: &'a ExecutionManagerConfig,
    stamp: AuthorityStamp,
    effects: Vec<acteon_governance::context::AcceptedEffect>,
}
impl ExecutionAuthorityRuntime {
    async fn management_observation<'a>(
        &'a self,
        namespace: &str,
        tenant: &str,
        authentication: &AuthenticatedExecutionConfiguration,
    ) -> Result<ManagementObservation<'a>, ManagementError> {
        let scope = self
            .scopes
            .get(&(namespace.into(), tenant.into()))
            .ok_or(ManagementError::Forbidden)?;
        let binding = authentication
            .management_scope(namespace, tenant)
            .map_err(|_| ManagementError::Forbidden)?;
        if !binding.matches_deployment_policy(scope.prepared.policy_fingerprint()) {
            return Err(ManagementError::Forbidden);
        }
        let policy = scope
            .prepared
            .declaration()
            .managers
            .iter()
            .find(|manager| manager.principal == *binding.authentication_source().principal())
            .ok_or(ManagementError::Forbidden)?;
        let state = scope.coordinator.snapshot().await?;
        let now_ms = self.clock.now().timestamp_millis();
        if now_ms < policy.valid_from_ms || now_ms >= policy.limits.deadline_ms {
            return Err(ManagementError::Forbidden);
        }
        let stamp = binding
            .verify_management_snapshot(&state, now_ms)
            .map_err(|_| ManagementError::Forbidden)?;
        let definitions = scope.prepared.catalog().definitions(namespace, tenant);
        let effects = policy
            .routes
            .iter()
            .map(|route| {
                definitions
                    .iter()
                    .find(|d| d.provider == route.provider && d.action_type == route.action_type)
                    .map(|d| d.effect.clone())
                    .ok_or(ManagementError::Unavailable)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ManagementObservation {
            scope,
            policy,
            stamp,
            effects,
        })
    }

    pub async fn inspect_governance(
        &self,
        namespace: &str,
        tenant: &str,
        authentication: &AuthenticatedExecutionConfiguration,
    ) -> Result<GovernanceScopeView, ManagementError> {
        let observation = self
            .management_observation(namespace, tenant, authentication)
            .await?;
        let state = observation.scope.coordinator.snapshot().await?;
        if state.stamp() != observation.stamp {
            return Err(ManagementError::Conflict);
        }
        let now_ms = self.clock.now().timestamp_millis();
        if now_ms < observation.policy.valid_from_ms
            || now_ms >= observation.policy.limits.deadline_ms
        {
            return Err(ManagementError::Forbidden);
        }
        authentication
            .management_scope(namespace, tenant)
            .map_err(|_| ManagementError::Forbidden)?
            .verify_management_snapshot(&state, now_ms)
            .map_err(|_| ManagementError::Forbidden)?;
        let resources = observation
            .effects
            .iter()
            .flat_map(|e| &e.resources)
            .collect::<std::collections::BTreeSet<_>>();
        let definitions = observation
            .scope
            .prepared
            .catalog()
            .definitions(namespace, tenant);
        let routes = definitions
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
        let permits = state
            .permits
            .values()
            .filter(|record| {
                observation.policy.subjects.contains(&record.permit.subject)
                    && record
                        .permit
                        .effects
                        .iter()
                        .all(|e| observation.effects.contains(e))
            })
            .map(|record| GovernancePermitView {
                id: record.permit.id.clone(),
                revision: record.permit.revision,
                subject: record.permit.subject.clone(),
                effects: record.permit.effects.iter().map(public_effect).collect(),
                valid_from_ms: record.permit.valid_from_ms,
                limits: GovernanceLimits {
                    max_units: record.permit.limits.max_units,
                    max_concurrent: record.permit.limits.max_concurrent,
                    deadline_ms: record.permit.limits.deadline_ms,
                },
                revoked: record.revoked,
            })
            .collect();
        Ok(GovernanceScopeView {
            management: acteon_core::GovernanceManagementBounds {
                subjects: observation.policy.subjects.clone(),
                can_issue_permits: observation.policy.can_issue_permits,
                can_intervene: observation.policy.can_intervene,
                valid_from_ms: observation.policy.valid_from_ms,
                limits: GovernanceLimits {
                    max_units: observation.policy.limits.max_units,
                    max_concurrent: observation.policy.limits.max_concurrent,
                    deadline_ms: observation.policy.limits.deadline_ms,
                },
            },
            namespace: namespace.into(),
            tenant: tenant.into(),
            incarnation: state.incarnation,
            generation: state.generation,
            routes,
            permits,
            closed_resources: state
                .closed_resources
                .into_iter()
                .filter(|r| resources.contains(r))
                .collect(),
            revoked_subjects: state
                .revoked_subjects
                .into_iter()
                .filter(|id| observation.policy.subjects.iter().any(|s| s.id() == id))
                .collect(),
        })
    }

    pub async fn publish_governance_permit(
        &self,
        request: PublishGovernancePermitRequest,
        authentication: &AuthenticatedExecutionConfiguration,
    ) -> Result<GovernanceChangeReceipt, ManagementError> {
        let observation = self
            .management_observation(&request.namespace, &request.tenant, authentication)
            .await?;
        if !observation.policy.can_issue_permits
            || request.permit.routes.is_empty()
            || request.permit.routes.len() > 128
        {
            return Err(ManagementError::Forbidden);
        }
        let mut routes = std::collections::BTreeSet::new();
        let definitions = observation
            .scope
            .prepared
            .catalog()
            .definitions(&request.namespace, &request.tenant);
        let effects = request
            .permit
            .routes
            .iter()
            .map(|route| {
                if !routes.insert((&route.provider, &route.action_type)) {
                    return Err(ManagementError::Invalid);
                }
                definitions
                    .iter()
                    .find(|d| d.provider == route.provider && d.action_type == route.action_type)
                    .map(|d| d.effect.clone())
                    .ok_or(ManagementError::Forbidden)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let permit = ExecutionPermit {
            id: request.permit.id,
            revision: request.permit.revision,
            subject: request.permit.subject,
            effects,
            valid_from_ms: request.permit.valid_from_ms,
            limits: acteon_governance::RootBudgetLimits {
                max_units: request.permit.limits.max_units,
                max_concurrent: request.permit.limits.max_concurrent,
                deadline_ms: request.permit.limits.deadline_ms,
            },
        };
        let ceiling = PermitIssuanceCeiling {
            issuer: observation.policy.principal.clone(),
            subjects: observation.policy.subjects.clone(),
            effects: observation.effects,
            valid_from_ms: observation.policy.valid_from_ms,
            limits: observation.policy.limits.clone(),
        };
        let record = observation
            .scope
            .coordinator
            .publish_permit_evaluated(EvaluatedPermitPublication {
                change_id: &request.change_id,
                permit,
                expected_revision: request.expected_revision,
                ceiling: &ceiling,
                evaluated_authority: &observation.stamp,
                reason: &request.reason,
                clock: self.clock.as_ref(),
            })
            .await?;
        Ok(receipt(
            request.namespace,
            request.tenant,
            request.change_id,
            record,
        ))
    }

    pub async fn intervene_governance(
        &self,
        request: GovernanceInterventionRequest,
        authentication: &AuthenticatedExecutionConfiguration,
    ) -> Result<GovernanceChangeReceipt, ManagementError> {
        let observation = self
            .management_observation(&request.namespace, &request.tenant, authentication)
            .await?;
        if !observation.policy.can_intervene {
            return Err(ManagementError::Forbidden);
        }
        let change = match request.change {
            GovernanceIntervention::CloseResource { resource } => {
                AuthorityChange::CloseResource { resource }
            }
            GovernanceIntervention::ReopenResource { resource } => {
                AuthorityChange::ReopenResource { resource }
            }
            GovernanceIntervention::RevokePermit {
                permit_id,
                expected_revision,
            } => AuthorityChange::RevokePermit {
                permit_id,
                expected_revision,
            },
            GovernanceIntervention::RevokeCredential {
                credential_id,
                expected_revision,
            } => AuthorityChange::RevokeCredential {
                credential_id,
                expected_revision,
            },
            GovernanceIntervention::RevokeSubject { subject } => {
                if !observation.policy.subjects.contains(&subject) {
                    return Err(ManagementError::Forbidden);
                }
                AuthorityChange::RevokeSubject {
                    subject: subject.id().into(),
                }
            }
        };
        let ceiling = ControlChangeCeiling {
            actor: observation.policy.principal.clone(),
            subjects: observation.policy.subjects.clone(),
            resources: observation
                .effects
                .into_iter()
                .flat_map(|e| e.resources)
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
            valid_from_ms: observation.policy.valid_from_ms,
            deadline_ms: observation.policy.limits.deadline_ms,
        };
        let record = observation
            .scope
            .coordinator
            .change_evaluated(
                &request.change_id,
                change,
                &request.reason,
                ControlChangeAuthorization {
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
}
fn public_effect(effect: &acteon_governance::context::AcceptedEffect) -> GovernanceEffect {
    GovernanceEffect {
        operation: effect.operation.clone(),
        resources: effect.resources.clone(),
    }
}
fn receipt(
    namespace: String,
    tenant: String,
    change_id: String,
    record: acteon_governance::ChangeRecord,
) -> GovernanceChangeReceipt {
    GovernanceChangeReceipt {
        namespace,
        tenant,
        change_id,
        actor: record.actor,
        reason: record.reason,
        generation: record.generation,
        pending: record.pending,
    }
}
