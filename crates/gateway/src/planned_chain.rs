//! Private adapter from authoritative chain state to the final provider boundary.
use acteon_core::{ActionError, ChainState};
use acteon_executor::plan::engine::{ChainExecutionMediator, ChainProviderCall};
use acteon_executor::{
    ProviderExecutionAdmission, ProviderExecutionAuthority, ProviderInvocation,
    ProviderInvocationOrigin,
};
use async_trait::async_trait;
use uuid::Uuid;

pub(crate) struct PlannedChainAdmission<'a> {
    pub boundary: &'a dyn ChainExecutionMediator,
    pub chain: &'a ChainState,
    pub step_path: &'a [String],
    pub job_id: Uuid,
    pub logical_attempt: Uuid,
}
#[async_trait]
impl ProviderExecutionAdmission for PlannedChainAdmission<'_> {
    async fn observe(
        &self,
        invocation: ProviderInvocation<'_>,
    ) -> Result<Option<acteon_core::ActionOutcome>, ActionError> {
        if invocation.context.is_some()
            || invocation.authority.is_some()
            || !matches!(
                invocation.origin,
                ProviderInvocationOrigin::Dispatch | ProviderInvocationOrigin::Fallback
            )
        {
            return Err(ActionError {
                code: "CHAIN_OBSERVATION_REFUSED".into(),
                message: "Unsupported planned chain observation".into(),
                retryable: false,
                attempts: 0,
            });
        }
        self.boundary
            .observe(ChainProviderCall {
                namespace: &self.chain.namespace,
                tenant: &self.chain.tenant,
                job_id: self.job_id,
                chain_name: &self.chain.chain_name,
                origin: &self.chain.origin_action,
                step_path: self.step_path,
                logical_attempt: self.logical_attempt,
                action: invocation.action,
                selected: invocation.selected,
            })
            .await
    }
    async fn admit(
        &self,
        invocation: ProviderInvocation<'_>,
    ) -> Result<ProviderExecutionAuthority, ActionError> {
        if invocation.context.is_some()
            || invocation.authority.is_some()
            || !matches!(
                invocation.origin,
                ProviderInvocationOrigin::Dispatch | ProviderInvocationOrigin::Fallback
            )
        {
            return Err(ActionError {
                code: "CHAIN_INVOCATION_REFUSED".into(),
                message: "Unsupported planned chain invocation".into(),
                retryable: false,
                attempts: 0,
            });
        }
        self.boundary
            .admit(ChainProviderCall {
                namespace: &self.chain.namespace,
                tenant: &self.chain.tenant,
                job_id: self.job_id,
                chain_name: &self.chain.chain_name,
                origin: &self.chain.origin_action,
                step_path: self.step_path,
                logical_attempt: self.logical_attempt,
                action: invocation.action,
                selected: invocation.selected,
            })
            .await
    }
}
