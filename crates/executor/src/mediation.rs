//! One host-owned selected-provider boundary. Request metadata is not authority.
use std::{collections::BTreeMap, sync::Arc};

use acteon_core::{
    Action, ActionError, ActionOutcome, ExecutionContextReference, PrincipalIdentity,
};
use acteon_governance::permit::PermitReference;
use acteon_provider::{DispatchContext, DynProvider};
use async_trait::async_trait;

use crate::ActionExecutor;
use crate::governed::{GovernedProviderExecutor, GovernedProviderStatus};

/// Per-invocation authority supplied exclusively by authenticated host code.
/// A public context reference alone is not authentication. Hosts must derive
/// `actor` independently from their private authentication proof and admit the
/// exact selected work before constructing this value. No wire deserializer.
pub struct ProviderExecutionAuthority {
    reference: ExecutionContextReference,
    actor: PrincipalIdentity,
    permits: Vec<PermitReference>,
}
impl ProviderExecutionAuthority {
    #[must_use]
    pub fn new_trusted(
        reference: ExecutionContextReference,
        actor: PrincipalIdentity,
        permits: Vec<PermitReference>,
    ) -> Self {
        Self {
            reference,
            actor,
            permits,
        }
    }
}

/// Host-selected call site, including effects synthesized by infrastructure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderInvocationOrigin {
    Dispatch,
    Fallback,
    Reroute,
    ApprovalNotification,
    ApprovalRetry,
    ChainStep,
    ChainCancellation,
}

/// Actual immutable selected instance and actual work presented for mediation.
/// Labels in the Action are inputs, never qualification or authorization proof.
pub struct ProviderInvocation<'a> {
    pub action: &'a Action,
    pub selected: &'a Arc<dyn DynProvider>,
    pub context: Option<&'a DispatchContext>,
    pub origin: ProviderInvocationOrigin,
    pub authority: Option<&'a ProviderExecutionAuthority>,
}

/// Request-scoped trusted admission for the final work selected by the gateway.
/// Implementations retain private authentication proof and durable admission
/// identity. This interface is never reconstructed from action metadata.
#[async_trait]
pub trait ProviderExecutionAdmission: Send + Sync {
    async fn admit(
        &self,
        invocation: ProviderInvocation<'_>,
    ) -> Result<ProviderExecutionAuthority, ActionError>;

    /// Inspect already retained provider evidence through trusted owned work.
    /// Default hosts have no observation path. `None` permits ordinary admission;
    /// an error must never trigger an execution fallback.
    async fn observe(
        &self,
        _invocation: ProviderInvocation<'_>,
    ) -> Result<Option<ActionOutcome>, ActionError> {
        Ok(None)
    }

    /// Admit complete host-selected chain work before indexing it. Unsupported
    /// hosts refuse rather than borrowing ordinary dispatch authority.
    async fn admit_chain(
        &self,
        _job_id: uuid::Uuid,
        _action: &Action,
        _entry: &str,
        _definitions: &BTreeMap<String, acteon_core::ChainConfig>,
    ) -> Result<(), ActionError> {
        Err(ActionError {
            code: "CHAIN_ADMISSION_UNSUPPORTED".into(),
            message: "This host has no qualified chain admission".into(),
            retryable: false,
            attempts: 0,
        })
    }
}

/// Strict host adapter retaining one durable executor per qualified route.
/// Executors share their configured local concurrency across invocations; they
/// are never rebuilt for each request. All persistence uses their `StateStore`.
pub struct GovernedProviderMediator {
    executors: BTreeMap<(String, String, String, String), GovernedProviderExecutor>,
}
impl GovernedProviderMediator {
    /// An explicitly non-executing installation, such as a retained-history host.
    /// It cannot select an executor and never invokes the supplied provider.
    #[must_use]
    pub fn deny_all() -> Self {
        Self {
            executors: BTreeMap::new(),
        }
    }

    pub fn new(executors: Vec<GovernedProviderExecutor>) -> Result<Self, &'static str> {
        if executors.is_empty() || executors.len() > 4096 {
            return Err("invalid governed mediator capacity");
        }
        let mut routes = BTreeMap::new();
        for executor in executors {
            let definition = executor.bound_provider().catalog_definition();
            if routes
                .insert(
                    (
                        definition.namespace,
                        definition.tenant,
                        definition.provider,
                        definition.action_type,
                    ),
                    executor.require_credential_authority(),
                )
                .is_some()
            {
                return Err("ambiguous governed mediator route");
            }
        }
        Ok(Self { executors: routes })
    }
}

/// Convert retained receipt state without authorizing or invoking a provider.
pub(crate) fn receipt_outcome(receipt: crate::governed::GovernedProviderReceipt) -> ActionOutcome {
    let state = match receipt.status {
        GovernedProviderStatus::Completed { outcome } => return outcome,
        GovernedProviderStatus::Prepared => return refused("GOVERNED_PREPARED", receipt.attempts),
        GovernedProviderStatus::InFlight { attempt_id } => {
            acteon_core::ProviderWorkState::InFlight { attempt_id }
        }
        GovernedProviderStatus::ReconciliationRequired { attempt_id } => {
            acteon_core::ProviderWorkState::ReconciliationRequired { attempt_id }
        }
        GovernedProviderStatus::AwaitingRetry { not_before_ms } => {
            acteon_core::ProviderWorkState::AwaitingRetry { not_before_ms }
        }
    };
    ActionOutcome::ProviderPending(acteon_core::ProviderWorkPending {
        execution_id: receipt.execution_id,
        attempts: receipt.attempts,
        state,
    })
}

fn refused(code: &str, attempts: u32) -> ActionOutcome {
    ActionOutcome::Failed(ActionError {
        code: code.into(),
        message: "Governed provider execution did not complete".into(),
        retryable: false,
        attempts,
    })
}

#[async_trait]
impl ProviderExecutionMediator for GovernedProviderMediator {
    fn requires_authority(&self) -> bool {
        true
    }

    async fn inspect(
        &self,
        action: &Action,
        selected: &Arc<dyn DynProvider>,
        reference: &ExecutionContextReference,
        actor: &PrincipalIdentity,
    ) -> Result<Option<crate::governed::GovernedProviderReceipt>, ActionError> {
        let denied = || ActionError {
            code: "GOVERNED_OBSERVATION_REFUSED".into(),
            message: "Provider evidence could not be verified".into(),
            retryable: false,
            attempts: 0,
        };
        if crate::governed::governed_provider_input_digest(action).map_err(|_| denied())?
            != reference.request_digest()
        {
            return Err(denied());
        }
        let route = (
            action.namespace.as_str().into(),
            action.tenant.as_str().into(),
            selected.name().into(),
            action.action_type.clone(),
        );
        let executor = self
            .executors
            .get(&route)
            .filter(|executor| executor.bound_provider().is_provider(selected))
            .ok_or_else(denied)?;
        executor
            .inspect(reference, actor)
            .await
            .map_err(|_| denied())
    }

    async fn execute(&self, invocation: ProviderInvocation<'_>) -> ActionOutcome {
        let Some(authority) = invocation.authority else {
            return refused("EXECUTION_AUTHORITY_REQUIRED", 0);
        };
        // Attachments/context must not silently disappear when executing the
        // admitted action through an executor that cannot yet bind their bytes.
        if invocation.context.is_some() {
            return refused("GOVERNED_CONTEXT_UNSUPPORTED", 0);
        }
        let route = (
            invocation.action.namespace.as_str().into(),
            invocation.action.tenant.as_str().into(),
            invocation.selected.name().into(),
            invocation.action.action_type.clone(),
        );
        let Some(executor) = self
            .executors
            .get(&route)
            .filter(|executor| executor.bound_provider().is_provider(invocation.selected))
        else {
            return refused("PROVIDER_UNQUALIFIED", 0);
        };
        match executor
            .execute(
                &authority.reference,
                &authority.permits,
                invocation.action,
                &authority.actor,
            )
            .await
        {
            Ok(receipt) => receipt_outcome(receipt),
            Err(_) => refused("GOVERNED_EXECUTION_REFUSED", 0),
        }
    }
}

/// Trusted host implementation owns complete execution, including each attempt,
/// retry and settlement. Refusal is final; gateways must never fall back to an
/// ungated executor after this boundary refuses or becomes unavailable.
#[async_trait]
pub trait ProviderExecutionMediator: Send + Sync {
    /// Refuse an unauthenticated dispatch before rules can publish work or
    /// consume deduplication/throttle state. Background effects are still gated
    /// separately at invocation. Compatibility adapters do not require proof.
    fn requires_authority(&self) -> bool {
        false
    }

    /// Historical receipt inspection. The host establishes ownership separately;
    /// implementations must never invoke a provider from this method.
    async fn inspect(
        &self,
        _action: &Action,
        _selected: &Arc<dyn DynProvider>,
        _reference: &ExecutionContextReference,
        _actor: &PrincipalIdentity,
    ) -> Result<Option<crate::governed::GovernedProviderReceipt>, ActionError> {
        Err(ActionError {
            code: "GOVERNED_OBSERVATION_UNSUPPORTED".into(),
            message: "This host has no provider receipt inspection".into(),
            retryable: false,
            attempts: 0,
        })
    }

    async fn execute(&self, invocation: ProviderInvocation<'_>) -> ActionOutcome;
}

/// Default compatibility adapter. Keep one executor so configured local
/// concurrency, clock, retry policy and DLQ behavior remain shared across calls.
pub struct LegacyProviderMediator {
    executor: ActionExecutor,
}
impl LegacyProviderMediator {
    #[must_use]
    pub fn new(executor: ActionExecutor) -> Self {
        Self { executor }
    }
}
#[async_trait]
impl ProviderExecutionMediator for LegacyProviderMediator {
    async fn execute(&self, invocation: ProviderInvocation<'_>) -> ActionOutcome {
        if let Some(context) = invocation.context {
            self.executor
                .execute_with_context(invocation.action, invocation.selected.as_ref(), context)
                .await
        } else {
            self.executor
                .execute(invocation.action, invocation.selected.as_ref())
                .await
        }
    }
}
