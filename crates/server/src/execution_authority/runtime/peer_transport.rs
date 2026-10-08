//! Host-controlled outbound delegation from an accepted agent context.

use super::ExecutionAuthorityRuntime;
use acteon_core::{ExecutionContextReference, TaskMessage};
use acteon_executor::delegation::{PeerSendReceipt, PeerTransportError};

/// Trusted host input. This type intentionally has no wire deserializer: a
/// model-facing tool supplies only target, skill and message while the host
/// injects the source agent and opaque context retained with accepted work.
pub struct AgentPeerInvocation<'a> {
    pub namespace: &'a str,
    pub tenant: &'a str,
    pub source_agent_id: &'a str,
    pub target_agent_id: &'a str,
    pub skill: &'a str,
    pub parent: &'a ExecutionContextReference,
    pub message: &'a TaskMessage,
}

#[derive(Debug, thiserror::Error)]
pub enum AgentPeerTransportError {
    #[error("invalid agent peer invocation")]
    Invalid,
    #[error("agent peer invocation is not authorized")]
    Forbidden,
    #[error("agent peer invocation conflicts with durable state")]
    Conflict,
    #[error("agent peer transport is unavailable")]
    Unavailable,
}

enum Operation {
    Submit,
    Observe,
    ReplayIdempotent,
}

impl ExecutionAuthorityRuntime {
    /// Submit once through the exact installed source/target binding. Repeated
    /// calls return journal state and never implicitly retry an ambiguous send.
    pub async fn submit_agent_peer(
        &self,
        invocation: AgentPeerInvocation<'_>,
    ) -> Result<PeerSendReceipt, AgentPeerTransportError> {
        self.agent_peer_operation(invocation, Operation::Submit)
            .await
    }

    /// Observe the durable send journal without issuing network traffic.
    pub async fn observe_agent_peer(
        &self,
        invocation: AgentPeerInvocation<'_>,
    ) -> Result<PeerSendReceipt, AgentPeerTransportError> {
        self.agent_peer_operation(invocation, Operation::Observe)
            .await
    }

    /// Explicit recovery for endpoints qualified as replay-idempotent.
    pub async fn replay_agent_peer_idempotent(
        &self,
        invocation: AgentPeerInvocation<'_>,
    ) -> Result<PeerSendReceipt, AgentPeerTransportError> {
        self.agent_peer_operation(invocation, Operation::ReplayIdempotent)
            .await
    }

    // Keep context recovery, runtime-binding verification and transport lookup
    // in one ordered authority boundary.
    #[allow(clippy::too_many_lines)]
    async fn agent_peer_operation(
        &self,
        invocation: AgentPeerInvocation<'_>,
        operation: Operation,
    ) -> Result<PeerSendReceipt, AgentPeerTransportError> {
        if invocation.namespace.is_empty()
            || invocation.tenant.is_empty()
            || invocation.source_agent_id.is_empty()
            || invocation.target_agent_id.is_empty()
            || invocation.skill.is_empty()
            || invocation.parent.namespace() != invocation.namespace
            || invocation.parent.tenant() != invocation.tenant
        {
            return Err(AgentPeerTransportError::Invalid);
        }
        let scope = self
            .scopes
            .get(&(invocation.namespace.into(), invocation.tenant.into()))
            .ok_or(AgentPeerTransportError::Unavailable)?;
        let source = scope
            .prepared
            .agents
            .get(invocation.source_agent_id)
            .ok_or(AgentPeerTransportError::Forbidden)?;
        if !source
            .declaration
            .onward_agents
            .iter()
            .any(|target| target == invocation.target_agent_id)
        {
            return Err(AgentPeerTransportError::Forbidden);
        }
        let parent = scope
            .contexts
            .recover_reference(invocation.parent, self.clock.now().timestamp_millis())
            .await
            .map_err(|_| AgentPeerTransportError::Forbidden)?;
        if parent.principal() != &source.declaration.principal {
            return Err(AgentPeerTransportError::Forbidden);
        }
        let direct = source
            .binding
            .service_plan()
            .ok_or(AgentPeerTransportError::Unavailable)?
            .direct_effects();
        scope
            .coordinator
            .verify_service_runtime_binding(&parent, source.binding.digest(), direct)
            .await
            .map_err(|_| AgentPeerTransportError::Forbidden)?;
        let registry = scope
            .peer_mesh
            .registry
            .as_ref()
            .ok_or(AgentPeerTransportError::Unavailable)?;
        let transport = scope
            .peer_mesh
            .transports
            .get(&(
                invocation.source_agent_id.into(),
                invocation.target_agent_id.into(),
                invocation.skill.into(),
            ))
            .ok_or(AgentPeerTransportError::Forbidden)?;
        let permits = &source.declaration.recipient_permits;
        let result = match operation {
            Operation::Submit => {
                transport
                    .submit(
                        registry,
                        invocation.target_agent_id,
                        invocation.skill,
                        &parent,
                        permits,
                        invocation.message,
                    )
                    .await
            }
            Operation::Observe => {
                transport
                    .observe(
                        registry,
                        invocation.target_agent_id,
                        invocation.skill,
                        &parent,
                        permits,
                        invocation.message,
                    )
                    .await
            }
            Operation::ReplayIdempotent => {
                transport
                    .replay_idempotent(
                        registry,
                        invocation.target_agent_id,
                        invocation.skill,
                        &parent,
                        permits,
                        invocation.message,
                    )
                    .await
            }
        };
        result.map_err(AgentPeerTransportError::from)
    }
}

impl From<PeerTransportError> for AgentPeerTransportError {
    fn from(error: PeerTransportError) -> Self {
        match error {
            PeerTransportError::Invalid => Self::Invalid,
            PeerTransportError::Conflict => Self::Conflict,
            PeerTransportError::Refused => Self::Forbidden,
            PeerTransportError::Unavailable => Self::Unavailable,
        }
    }
}
