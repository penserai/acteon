//! Host boundary for owned durable chain work. State labels are not authority.
use super::handoff::{PlanHandoffStore, RecoveredPlanJob, ReservePlanCall};
use super::{PlanCallSite, PlanProviderAdmission};
use crate::ProviderExecutionAuthority;
use crate::catalog::QualifiedProviderCatalog;
use crate::{
    ProviderExecutionAdmission, ProviderExecutionMediator, ProviderInvocation,
    ProviderInvocationOrigin,
};
use acteon_core::{Action, ActionError, ActionOutcome};
use acteon_governance::{AuthorityCoordinator, context::TrustedContextStore};
use acteon_provider::DynProvider;
use acteon_time::Clock;
use async_trait::async_trait;
use std::sync::Arc;
use uuid::Uuid;

/// Actual selected provider and host work-record identity. No Deserialize.
#[derive(Clone, Copy)]
pub struct ChainProviderCall<'a> {
    pub namespace: &'a str,
    pub tenant: &'a str,
    pub job_id: Uuid,
    pub chain_name: &'a str,
    pub origin: &'a Action,
    pub step_path: &'a [String],
    pub logical_attempt: Uuid,
    pub action: &'a Action,
    pub selected: &'a Arc<dyn DynProvider>,
}

#[async_trait]
pub trait ChainExecutionMediator: Send + Sync {
    /// Recover only using identity loaded from authoritative owned chain state.
    /// This is observational; every actual provider call needs fresh admission.
    async fn recover_job(
        &self,
        namespace: &str,
        tenant: &str,
        job_id: Uuid,
    ) -> Result<RecoveredPlanJob, ActionError>;
    /// Commit a permanent instance fence from authoritative owned work identity.
    /// Must precede publishing cancellation; does not settle outstanding effects.
    async fn cancel_job(
        &self,
        namespace: &str,
        tenant: &str,
        job_id: Uuid,
    ) -> Result<(), ActionError>;
    /// Observe retained evidence without creating a call or admitting an effect.
    async fn observe(
        &self,
        call: ChainProviderCall<'_>,
    ) -> Result<Option<ActionOutcome>, ActionError>;
    async fn admit(
        &self,
        call: ChainProviderCall<'_>,
    ) -> Result<ProviderExecutionAuthority, ActionError>;
    async fn execute(&self, call: ChainProviderCall<'_>) -> ActionOutcome;
}

/// One configured scope, sharing the existing provider mediator and `StateStore`.
/// Construction is a trusted host installation boundary, not an API payload.
pub struct StoredChainExecution {
    handoffs: Arc<PlanHandoffStore>,
    contexts: Arc<TrustedContextStore>,
    coordinator: AuthorityCoordinator,
    catalog: QualifiedProviderCatalog,
    providers: Arc<dyn ProviderExecutionMediator>,
    clock: Arc<dyn Clock>,
}
fn refused() -> ActionError {
    ActionError {
        code: "CHAIN_EXECUTION_AUTHORITY_REFUSED".into(),
        message: "Chain work has no verified current execution authority".into(),
        retryable: false,
        attempts: 0,
    }
}
#[async_trait]
impl ChainExecutionMediator for StoredChainExecution {
    async fn recover_job(
        &self,
        namespace: &str,
        tenant: &str,
        job_id: Uuid,
    ) -> Result<RecoveredPlanJob, ActionError> {
        let job = self
            .handoffs
            .recover(job_id, self.catalog.clone(), &self.contexts)
            .await
            .map_err(|_| refused())?;
        let reference = job.root().reference().map_err(|_| refused())?;
        if reference.namespace() != namespace || reference.tenant() != tenant {
            return Err(refused());
        }
        Ok(job)
    }
    async fn cancel_job(
        &self,
        namespace: &str,
        tenant: &str,
        job_id: Uuid,
    ) -> Result<(), ActionError> {
        let root = self
            .handoffs
            .recover_root_for_cancellation(job_id, &self.contexts)
            .await
            .map_err(|_| refused())?;
        let reference = root.reference().map_err(|_| refused())?;
        if reference.namespace() != namespace || reference.tenant() != tenant {
            return Err(refused());
        }
        let execution_id = root.execution_id().to_string();
        self.coordinator
            .change(
                &format!("execution-cancel/{execution_id}"),
                acteon_governance::AuthorityChange::CancelExecution { execution_id },
                "acteon.chain-engine",
                "owned execution cancelled",
            )
            .await
            .map_err(|_| refused())?;
        Ok(())
    }
    async fn observe(
        &self,
        call: ChainProviderCall<'_>,
    ) -> Result<Option<ActionOutcome>, ActionError> {
        self.observe_owned(call).await
    }
    async fn admit(
        &self,
        call: ChainProviderCall<'_>,
    ) -> Result<ProviderExecutionAuthority, ActionError> {
        self.admit_owned(call).await
    }
    async fn execute(&self, call: ChainProviderCall<'_>) -> ActionOutcome {
        match self.execute_owned(call).await {
            Ok(outcome) => outcome,
            Err(error) => ActionOutcome::Failed(error),
        }
    }
}
impl StoredChainExecution {
    pub fn new_trusted(
        handoffs: Arc<PlanHandoffStore>,
        contexts: Arc<TrustedContextStore>,
        coordinator: AuthorityCoordinator,
        catalog: QualifiedProviderCatalog,
        providers: Arc<dyn ProviderExecutionMediator>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, ActionError> {
        if !providers.requires_authority() {
            return Err(refused());
        }
        Ok(Self {
            handoffs,
            contexts,
            coordinator,
            catalog,
            providers,
            clock,
        })
    }
    async fn observe_owned(
        &self,
        call: ChainProviderCall<'_>,
    ) -> Result<Option<ActionOutcome>, ActionError> {
        let job = self
            .recover_job(call.namespace, call.tenant, call.job_id)
            .await?;
        if job.invocation().plan.entry != call.chain_name
            || crate::governed::governed_provider_input_digest(job.origin())
                .map_err(|_| refused())?
                != crate::governed::governed_provider_input_digest(call.origin)
                    .map_err(|_| refused())?
        {
            return Err(refused());
        }
        let snapshot = self.coordinator.snapshot().await.map_err(|_| refused())?;
        let limits = snapshot
            .roots
            .get(&job.root().execution_id().to_string())
            .ok_or_else(refused)?
            .limits
            .clone();
        let site = PlanCallSite::Step {
            chain: call.chain_name.into(),
            path: call.step_path.to_vec(),
        };
        let path = vec![call.chain_name.into()];
        let Some(retained) = self
            .handoffs
            .recover_provider_call(
                &job,
                ReservePlanCall {
                    logical_attempt: call.logical_attempt,
                    parent: job.root(),
                    call_site: &site,
                    chain_path: &path,
                    action: call.action,
                    selected: call.selected,
                    limits,
                },
            )
            .await
            .map_err(|_| refused())?
        else {
            return Ok(None);
        };
        let Some(context) = retained
            .observe_context(&self.contexts)
            .await
            .map_err(|_| refused())?
        else {
            return Ok(None);
        };
        let reference = context.reference().map_err(|_| refused())?;
        let Some(receipt) = self
            .providers
            .inspect(
                call.action,
                call.selected,
                &reference,
                job.root().principal(),
            )
            .await?
        else {
            return Ok(None);
        };
        match &receipt.status {
            crate::governed::GovernedProviderStatus::Prepared
            | crate::governed::GovernedProviderStatus::AwaitingRetry { .. } => Ok(None),
            _ => Ok(Some(crate::mediation::receipt_outcome(receipt))),
        }
    }
    async fn admit_owned(
        &self,
        call: ChainProviderCall<'_>,
    ) -> Result<ProviderExecutionAuthority, ActionError> {
        let job = self
            .recover_job(call.namespace, call.tenant, call.job_id)
            .await?;
        let plan = job.invocation();
        if plan.plan.entry != call.chain_name
            || crate::governed::governed_provider_input_digest(job.origin())
                .map_err(|_| refused())?
                != crate::governed::governed_provider_input_digest(call.origin)
                    .map_err(|_| refused())?
        {
            return Err(refused());
        }
        let snapshot = self.coordinator.snapshot().await.map_err(|_| refused())?;
        let limits = snapshot
            .roots
            .get(&job.root().execution_id().to_string())
            .ok_or_else(refused)?
            .limits
            .clone();
        let site = PlanCallSite::Step {
            chain: call.chain_name.into(),
            path: call.step_path.to_vec(),
        };
        let path = vec![call.chain_name.into()];
        let reserved = self
            .handoffs
            .reserve_provider_call(
                &job,
                ReservePlanCall {
                    logical_attempt: call.logical_attempt,
                    parent: job.root(),
                    call_site: &site,
                    chain_path: &path,
                    action: call.action,
                    selected: call.selected,
                    limits: limits.clone(),
                },
            )
            .await
            .map_err(|_| refused())?;
        let admission = plan.provider_admission(
            &self.contexts,
            PlanProviderAdmission {
                root: job.root(),
                parent: job.root(),
                call_site: &site,
                chain_path: &path,
                admission_key: reserved.admission_key(),
                handle: reserved.handle().clone(),
                execution_id: reserved.execution_id(),
                permits: job.permits(),
                limits,
                clock: self.clock.as_ref(),
            },
        );
        let authority = admission
            .admit(ProviderInvocation {
                action: call.action,
                selected: call.selected,
                context: None,
                origin: ProviderInvocationOrigin::ChainStep,
                authority: None,
            })
            .await?;
        Ok(authority)
    }
    async fn execute_owned(
        &self,
        call: ChainProviderCall<'_>,
    ) -> Result<ActionOutcome, ActionError> {
        if let Some(outcome) = self.observe_owned(call).await? {
            return Ok(outcome);
        }
        let action = call.action;
        let selected = call.selected;
        let authority = self.admit_owned(call).await?;
        Ok(self
            .providers
            .execute(ProviderInvocation {
                action,
                selected,
                context: None,
                origin: ProviderInvocationOrigin::ChainStep,
                authority: Some(&authority),
            })
            .await)
    }
}
impl RecoveredPlanJob {
    /// Definition reconstructed from the admitted complete plan, not live config.
    pub fn root_definition(&self) -> &acteon_core::ChainConfig {
        &self.invocation().plan.definitions[&self.invocation().plan.entry]
    }
}
