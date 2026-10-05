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
}

/// Strict host adapter retaining one durable executor per qualified route.
/// Executors share their configured local concurrency across invocations; they
/// are never rebuilt for each request. All persistence uses their `StateStore`.
pub struct GovernedProviderMediator {
    executors: BTreeMap<(String, String, String, String), GovernedProviderExecutor>,
}
impl GovernedProviderMediator {
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
            Ok(receipt) => match receipt.status {
                GovernedProviderStatus::Completed { outcome } => outcome,
                GovernedProviderStatus::ReconciliationRequired { .. } => {
                    refused("GOVERNED_RECONCILIATION_REQUIRED", receipt.attempts)
                }
                GovernedProviderStatus::InFlight { .. } => {
                    refused("GOVERNED_IN_FLIGHT", receipt.attempts)
                }
                GovernedProviderStatus::AwaitingRetry { .. } => {
                    refused("GOVERNED_AWAITING_RETRY", receipt.attempts)
                }
                GovernedProviderStatus::Prepared => refused("GOVERNED_PREPARED", receipt.attempts),
            },
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
