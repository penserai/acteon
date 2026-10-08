//! Host-controlled outbound delegation from an accepted agent context.

use super::ExecutionAuthorityRuntime;
use crate::auth::projection::AuthenticatedExecutionConfiguration;
use acteon_core::{ExecutionContextReference, TaskMessage};
use acteon_executor::delegation::{PeerCancelReceipt, PeerSendReceipt, PeerTransportError};

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

/// Agent-facing host-tool input. The accepted source task is an opaque lookup
/// handle; current private authentication must match its recipient context.
/// No execution context, permit, endpoint, credential or binding is accepted.
pub struct AgentPeerToolRequest<'a> {
    pub namespace: &'a str,
    pub tenant: &'a str,
    pub source_agent_id: &'a str,
    pub source_task_id: uuid::Uuid,
    pub target_agent_id: &'a str,
    pub skill: &'a str,
    pub message: &'a TaskMessage,
    pub authentication: &'a AuthenticatedExecutionConfiguration,
}

/// Agent-facing lifecycle refresh. The submission id is an opaque journal
/// lookup and carries no authority by itself.
pub struct AgentPeerRefreshRequest<'a> {
    pub namespace: &'a str,
    pub tenant: &'a str,
    pub source_agent_id: &'a str,
    pub source_task_id: uuid::Uuid,
    pub target_agent_id: &'a str,
    pub skill: &'a str,
    pub submission_id: uuid::Uuid,
    pub authentication: &'a AuthenticatedExecutionConfiguration,
}

/// Trusted host input after the source task and caller have been matched.
pub struct AgentPeerRefreshInvocation<'a> {
    pub namespace: &'a str,
    pub tenant: &'a str,
    pub source_agent_id: &'a str,
    pub target_agent_id: &'a str,
    pub skill: &'a str,
    pub parent: &'a ExecutionContextReference,
    pub submission_id: uuid::Uuid,
}

/// Agent-facing lifecycle cancellation. The submission id only selects a
/// durable send record; current source authority is recovered independently.
pub struct AgentPeerCancelRequest<'a> {
    pub namespace: &'a str,
    pub tenant: &'a str,
    pub source_agent_id: &'a str,
    pub source_task_id: uuid::Uuid,
    pub target_agent_id: &'a str,
    pub skill: &'a str,
    pub submission_id: uuid::Uuid,
    pub authentication: &'a AuthenticatedExecutionConfiguration,
}

/// Trusted host input after the source task and caller have been matched.
pub struct AgentPeerCancelInvocation<'a> {
    pub namespace: &'a str,
    pub tenant: &'a str,
    pub source_agent_id: &'a str,
    pub target_agent_id: &'a str,
    pub skill: &'a str,
    pub parent: &'a ExecutionContextReference,
    pub submission_id: uuid::Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AgentPeerTransportError {
    #[error("invalid agent peer invocation")]
    Invalid,
    #[error("agent peer invocation is not authorized")]
    Forbidden,
    #[error("agent peer source task is unavailable")]
    NotFound,
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
    /// Resolve the source authority from an accepted task and current private
    /// agent authentication, then enter the trusted peer transport boundary.
    pub async fn submit_agent_peer_tool(
        &self,
        request: AgentPeerToolRequest<'_>,
    ) -> Result<PeerSendReceipt, AgentPeerTransportError> {
        let parent = self
            .agent_peer_tool_parent(
                request.namespace,
                request.tenant,
                request.source_agent_id,
                request.source_task_id,
                request.authentication,
            )
            .await?;
        let reference = parent
            .reference()
            .map_err(|_| AgentPeerTransportError::Unavailable)?;
        self.submit_agent_peer(AgentPeerInvocation {
            namespace: request.namespace,
            tenant: request.tenant,
            source_agent_id: request.source_agent_id,
            target_agent_id: request.target_agent_id,
            skill: request.skill,
            parent: &reference,
            message: request.message,
        })
        .await
    }

    /// Refresh one accepted remote task through its exact durable send record.
    pub async fn refresh_agent_peer_tool(
        &self,
        request: AgentPeerRefreshRequest<'_>,
    ) -> Result<PeerSendReceipt, AgentPeerTransportError> {
        let parent = self
            .agent_peer_tool_parent(
                request.namespace,
                request.tenant,
                request.source_agent_id,
                request.source_task_id,
                request.authentication,
            )
            .await?;
        let reference = parent
            .reference()
            .map_err(|_| AgentPeerTransportError::Unavailable)?;
        self.refresh_agent_peer(AgentPeerRefreshInvocation {
            namespace: request.namespace,
            tenant: request.tenant,
            source_agent_id: request.source_agent_id,
            target_agent_id: request.target_agent_id,
            skill: request.skill,
            parent: &reference,
            submission_id: request.submission_id,
        })
        .await
    }

    /// Cancel one accepted remote task through its exact durable send record.
    pub async fn cancel_agent_peer_tool(
        &self,
        request: AgentPeerCancelRequest<'_>,
    ) -> Result<PeerCancelReceipt, AgentPeerTransportError> {
        let parent = self
            .agent_peer_tool_parent(
                request.namespace,
                request.tenant,
                request.source_agent_id,
                request.source_task_id,
                request.authentication,
            )
            .await?;
        let reference = parent
            .reference()
            .map_err(|_| AgentPeerTransportError::Unavailable)?;
        self.cancel_agent_peer(AgentPeerCancelInvocation {
            namespace: request.namespace,
            tenant: request.tenant,
            source_agent_id: request.source_agent_id,
            target_agent_id: request.target_agent_id,
            skill: request.skill,
            parent: &reference,
            submission_id: request.submission_id,
        })
        .await
    }

    async fn agent_peer_tool_parent(
        &self,
        namespace: &str,
        tenant: &str,
        source_agent_id: &str,
        source_task_id: uuid::Uuid,
        authentication: &AuthenticatedExecutionConfiguration,
    ) -> Result<acteon_governance::context::VerifiedExecutionContext, AgentPeerTransportError> {
        authentication
            .verify_authentication_current()
            .await
            .map_err(
                |error| match super::AgentServiceError::authentication(&error) {
                    super::AgentServiceError::Unavailable => AgentPeerTransportError::Unavailable,
                    _ => AgentPeerTransportError::Forbidden,
                },
            )?;
        let caller = authentication
            .scope(namespace, tenant)
            .map_err(|_| AgentPeerTransportError::Forbidden)?;
        let scope = self
            .scopes
            .get(&(namespace.into(), tenant.into()))
            .ok_or(AgentPeerTransportError::NotFound)?;
        let source = scope
            .prepared
            .agents
            .get(source_agent_id)
            .ok_or(AgentPeerTransportError::NotFound)?;
        let runtime = self
            .service_runtime_for_task(namespace, tenant, source_agent_id, source_task_id)
            .await
            .map_err(|error| match error {
                super::AgentServiceError::Unavailable => AgentPeerTransportError::Unavailable,
                _ => AgentPeerTransportError::NotFound,
            })?;
        if runtime.binding_digest() != source.binding.digest() {
            return Err(AgentPeerTransportError::NotFound);
        }
        let parent = runtime
            .recipient_context(source_task_id)
            .await
            .map_err(|error| match super::AgentServiceError::observation(error) {
                super::AgentServiceError::Unavailable => AgentPeerTransportError::Unavailable,
                _ => AgentPeerTransportError::NotFound,
            })?;
        let actor = caller.authentication_source();
        if parent.principal() != &source.declaration.principal
            || parent.principal() != actor.principal()
            || parent.credential_authority() != Some(caller.credential_reference())
            || parent.auth_method() != actor.auth_method()
        {
            return Err(AgentPeerTransportError::NotFound);
        }
        Ok(parent)
    }

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

    /// Refresh a remote task using only the installed transport and the
    /// accepted journal entry selected by `submission_id`.
    pub async fn refresh_agent_peer(
        &self,
        invocation: AgentPeerRefreshInvocation<'_>,
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
        transport
            .refresh_task(
                registry,
                invocation.target_agent_id,
                invocation.skill,
                &parent,
                &source.declaration.recipient_permits,
                invocation.submission_id,
            )
            .await
            .map_err(Into::into)
    }

    /// Persist and deliver at most one cancellation for a remote task. Every
    /// call revalidates the current source binding, permits and peer grant.
    pub async fn cancel_agent_peer(
        &self,
        invocation: AgentPeerCancelInvocation<'_>,
    ) -> Result<PeerCancelReceipt, AgentPeerTransportError> {
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
        transport
            .cancel_task(
                registry,
                invocation.target_agent_id,
                invocation.skill,
                &parent,
                &source.declaration.recipient_permits,
                invocation.submission_id,
            )
            .await
            .map_err(Into::into)
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
