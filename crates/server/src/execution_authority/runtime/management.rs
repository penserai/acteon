//! Management from original private authentication and independent deployment policy.
mod reconciliation;
mod registry;
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
pub use reconciliation::TrustedReconciliationInstallation;

#[derive(Debug, thiserror::Error)]
pub enum ManagementError {
    #[error("current management authority is required")]
    Forbidden,
    #[error("provider execution history not found")]
    NotFound,
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
    authentication_stamp: AuthorityStamp,
    effects: Vec<acteon_governance::context::AcceptedEffect>,
}
impl ExecutionAuthorityRuntime {
    fn management_now(&self, policy: &ExecutionManagerConfig) -> Result<i64, ManagementError> {
        let now_ms = self.clock.now().timestamp_millis();
        if now_ms < policy.valid_from_ms || now_ms >= policy.limits.deadline_ms {
            return Err(ManagementError::Forbidden);
        }
        Ok(now_ms)
    }

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
        self.management_now(policy)?;
        let state = scope
            .coordinator
            .snapshot()
            .await
            .map_err(|_| ManagementError::Unavailable)?;
        let authentication_stamp = authentication.verify_authentication_current().await?;
        let now_ms = self.management_now(policy)?;
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
            authentication_stamp,
            effects,
        })
    }

    async fn revalidate_management(
        &self,
        observation: &ManagementObservation<'_>,
        authentication: &AuthenticatedExecutionConfiguration,
    ) -> Result<acteon_governance::CoordinatorSnapshot, ManagementError> {
        let declaration = observation.scope.prepared.declaration();
        self.management_now(observation.policy)?;
        let state = observation
            .scope
            .coordinator
            .snapshot()
            .await
            .map_err(|_| ManagementError::Unavailable)?;
        if state.stamp() != observation.stamp {
            return Err(ManagementError::Conflict);
        }
        let authentication_stamp = authentication.verify_authentication_current().await?;
        if authentication_stamp != observation.authentication_stamp {
            return Err(ManagementError::Conflict);
        }
        let now_ms = self.management_now(observation.policy)?;
        authentication
            .management_scope(&declaration.namespace, &declaration.tenant)
            .map_err(|_| ManagementError::Forbidden)?
            .verify_management_snapshot(&state, now_ms)
            .map_err(|_| ManagementError::Forbidden)?;
        Ok(state)
    }

    /// Read verified evidence under current operator authority. Historical work
    /// may be expired or cancelled; the operator's read grant must remain current.
    pub async fn inspect_provider_history(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: uuid::Uuid,
        authentication: &AuthenticatedExecutionConfiguration,
    ) -> Result<acteon_core::ProviderExecutionHistory, ManagementError> {
        use acteon_executor::governed::GovernedProviderError;
        let observation = self
            .management_observation(namespace, tenant, authentication)
            .await?;
        if !observation.policy.can_read_history {
            return Err(ManagementError::Forbidden);
        }
        let mut history = Ok(None);
        for subject in &observation.policy.subjects {
            match observation
                .scope
                .history
                .inspect_execution(execution_id, subject)
                .await
            {
                Ok(receipt) => {
                    history = Ok(receipt);
                    break;
                }
                Err(GovernedProviderError::Ownership) => {}
                Err(GovernedProviderError::Conflict) => {
                    history = Err(ManagementError::Conflict);
                    break;
                }
                Err(_) => {
                    history = Err(ManagementError::Unavailable);
                    break;
                }
            }
        }
        // A reader that lost authority must not learn even the storage status.
        // Revalidate success, absence and error observations before projecting them.
        self.revalidate_management(&observation, authentication)
            .await?;
        let history = history?.ok_or(ManagementError::NotFound)?;
        // Both projections have an explicitly tested public wire contract.
        serde_json::from_value(
            serde_json::to_value(history).map_err(|_| ManagementError::Unavailable)?,
        )
        .map_err(|_| ManagementError::Unavailable)
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
        let state = self
            .revalidate_management(&observation, authentication)
            .await?;
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
                can_read_history: observation.policy.can_read_history,
                can_reconcile: observation.policy.can_reconcile,
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
                .chain(observation.policy.agents.iter().map(|id| {
                    acteon_core::ResourceRef::new(
                        acteon_core::ResourceKind::Agent,
                        &request.namespace,
                        &request.tenant,
                        id,
                    )
                    .expect("validated agent management bound")
                }))
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
