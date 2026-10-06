//! Host installation and private-authentication freshness for reconciliation.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use acteon_executor::governed::reconciliation::{
    ProviderReconciliationStore, ProviderReconciliationVerifier,
};
use acteon_governance::{CoordinationError, reconciliation::ReconciliationAuthorityGuard};

use super::{ExecutionAuthorityRuntime, ManagementError, ManagementObservation};
use crate::auth::projection::AuthenticatedExecutionConfiguration;

/// Trusted host configuration, deliberately not deserializable. Qualification
/// must cover the original external source, exact immutable binding digest and
/// irrevocable finality for all possible deliveries. An HMAC key alone is not
/// qualification. Public request fields cannot install verifiers.
pub struct TrustedReconciliationInstallation {
    pub namespace: String,
    pub tenant: String,
    pub bindings: BTreeMap<String, Arc<dyn ProviderReconciliationVerifier>>,
}

impl ExecutionAuthorityRuntime {
    /// Install before serving requests. Consumes the runtime so qualifications
    /// cannot change underneath an in-flight request. No backend records change.
    pub fn with_trusted_reconciliation_verifiers(
        mut self,
        installations: Vec<TrustedReconciliationInstallation>,
    ) -> Result<Self, String> {
        if installations.is_empty() || installations.len() > 128 {
            return Err("invalid reconciliation installation".into());
        }
        let mut selected = BTreeSet::new();
        for installation in installations {
            let key = (installation.namespace, installation.tenant);
            if !selected.insert(key.clone()) {
                return Err("duplicate reconciliation scope".into());
            }
            let scope = self
                .scopes
                .get_mut(&key)
                .ok_or("undeclared reconciliation scope")?;
            if scope.reconciliation.is_some() {
                return Err("reconciliation scope already installed".into());
            }
            scope.reconciliation = Some(
                ProviderReconciliationStore::new_trusted(
                    self.state.clone(),
                    scope.coordinator.clone(),
                    scope.contexts.clone(),
                    self.clock.clone(),
                    self.encryptor.clone(),
                    installation.bindings,
                )
                .map_err(|_| "invalid reconciliation qualification")?,
            );
        }
        Ok(self)
    }

    /// Trusted host accessor; not an HTTP authorization path. Every mutation
    /// through this store still requires independently evaluated authority.
    #[must_use]
    pub fn reconciliation_store(
        &self,
        namespace: &str,
        tenant: &str,
    ) -> Option<&ProviderReconciliationStore> {
        self.scopes
            .get(&(namespace.into(), tenant.into()))?
            .reconciliation
            .as_ref()
    }

    /// Resolve retained ownership from trusted storage before transport methods
    /// request correlation or settlement. Request labels never choose an owner.
    pub async fn provider_reconciliation_context(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: uuid::Uuid,
        authentication: &AuthenticatedExecutionConfiguration,
    ) -> Result<acteon_core::ExecutionContextReference, ManagementError> {
        let observation = self
            .management_observation(namespace, tenant, authentication)
            .await?;
        if !observation.policy.can_reconcile {
            return Err(ManagementError::Forbidden);
        }
        let store = observation
            .scope
            .reconciliation
            .as_ref()
            .ok_or(ManagementError::Unavailable)?;
        let mut result = Ok(None);
        for subject in &observation.policy.subjects {
            match store.owned_execution_reference(execution_id, subject).await {
                Ok(reference) => {
                    result = Ok(reference);
                    break;
                }
                Err(acteon_executor::governed::GovernedProviderError::Ownership) => {}
                Err(error) => {
                    result = Err(management_error(&error));
                    break;
                }
            }
        }
        self.revalidate_management(&observation, authentication)
            .await?;
        result?.ok_or(ManagementError::NotFound)
    }

    async fn reconciliation_observation<'a>(
        &'a self,
        context: &acteon_core::ExecutionContextReference,
        authentication: &AuthenticatedExecutionConfiguration,
    ) -> Result<ManagementObservation<'a>, ManagementError> {
        let observation = self
            .management_observation(context.namespace(), context.tenant(), authentication)
            .await?;
        if !observation.policy.can_reconcile {
            return Err(ManagementError::Forbidden);
        }
        if !observation.policy.subjects.contains(context.principal()) {
            return Err(ManagementError::NotFound);
        }
        if observation.scope.reconciliation.is_none() {
            return Err(ManagementError::Unavailable);
        }
        Ok(observation)
    }

    /// Original private authentication plus independent finality-management
    /// policy; no caller-supplied identity, verifier or execution grant is used.
    pub async fn provider_reconciliation_attempt(
        &self,
        context: &acteon_core::ExecutionContextReference,
        ordinal: u32,
        authentication: &AuthenticatedExecutionConfiguration,
    ) -> Result<acteon_executor::governed::reconciliation::ReconciliationAttempt, ManagementError>
    {
        let observation = self
            .reconciliation_observation(context, authentication)
            .await?;
        let ceiling = reconciliation_ceiling(&observation);
        let guard = ManagementReconciliationGuard {
            runtime: self,
            observation: &observation,
            authentication,
        };
        let result = observation
            .scope
            .reconciliation
            .as_ref()
            .ok_or(ManagementError::Unavailable)?
            .reconciliation_attempt_evaluated(
                context,
                context.principal(),
                ordinal,
                acteon_governance::reconciliation::ReconciliationAuthorization {
                    ceiling: &ceiling,
                    evaluated_authority: &observation.stamp,
                    clock: self.clock.as_ref(),
                    guard: Some(&guard),
                },
            )
            .await;
        self.revalidate_management(&observation, authentication)
            .await?;
        result.map_err(|error| management_error(&error))
    }

    /// Accept independent qualified finality with freshness checks before
    /// staging and on each settlement CAS attempt, including accepted replay.
    pub async fn reconcile_provider_attempt(
        &self,
        context: &acteon_core::ExecutionContextReference,
        ordinal: u32,
        proof: &[u8],
        authentication: &AuthenticatedExecutionConfiguration,
    ) -> Result<acteon_executor::governed::GovernedProviderReceipt, ManagementError> {
        let observation = self
            .reconciliation_observation(context, authentication)
            .await?;
        let ceiling = reconciliation_ceiling(&observation);
        let guard = ManagementReconciliationGuard {
            runtime: self,
            observation: &observation,
            authentication,
        };
        let result = observation
            .scope
            .reconciliation
            .as_ref()
            .ok_or(ManagementError::Unavailable)?
            .reconcile_evaluated(
                context,
                context.principal(),
                ordinal,
                proof,
                acteon_governance::reconciliation::ReconciliationAuthorization {
                    ceiling: &ceiling,
                    evaluated_authority: &observation.stamp,
                    clock: self.clock.as_ref(),
                    guard: Some(&guard),
                },
            )
            .await;
        self.revalidate_management(&observation, authentication)
            .await?;
        result.map_err(|error| management_error(&error))
    }
}

/// Revalidates the original private authentication source, frozen deployment
/// policy and execution-scope stamp. Separate source and execution reads are
/// not a cross-record transaction. Closures/revocations in the execution scope
/// remain fenced by the settlement's generation-checked CAS.
pub(super) struct ManagementReconciliationGuard<'a> {
    pub runtime: &'a ExecutionAuthorityRuntime,
    pub observation: &'a ManagementObservation<'a>,
    pub authentication: &'a AuthenticatedExecutionConfiguration,
}
#[async_trait::async_trait]
impl ReconciliationAuthorityGuard for ManagementReconciliationGuard<'_> {
    async fn check_current(&self) -> Result<(), CoordinationError> {
        self.runtime
            .revalidate_management(self.observation, self.authentication)
            .await
            .map(|_| ())
            .map_err(|error| match error {
                ManagementError::Forbidden => CoordinationError::Restricted,
                ManagementError::Conflict => CoordinationError::StaleAuthority,
                _ => CoordinationError::Contention,
            })
    }
}

fn reconciliation_ceiling(
    observation: &ManagementObservation<'_>,
) -> acteon_governance::reconciliation::ReconciliationCeiling {
    acteon_governance::reconciliation::ReconciliationCeiling {
        actor: observation.policy.principal.clone(),
        subjects: observation
            .policy
            .subjects
            .iter()
            .map(|s| s.id().into())
            .collect(),
        resources: observation.policy.reconciliation_resources.clone(),
        valid_from_ms: observation.policy.valid_from_ms,
        deadline_ms: observation.policy.limits.deadline_ms,
    }
}
fn management_error(error: &acteon_executor::governed::GovernedProviderError) -> ManagementError {
    use acteon_executor::governed::GovernedProviderError;
    match error {
        GovernedProviderError::Ownership => ManagementError::NotFound,
        GovernedProviderError::Conflict => ManagementError::Conflict,
        GovernedProviderError::Admission(_) => ManagementError::Forbidden,
        GovernedProviderError::Invalid => ManagementError::Invalid,
        GovernedProviderError::Unavailable => ManagementError::Unavailable,
    }
}
