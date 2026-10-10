//! Authenticated cross-domain admission. Importing signed authority remains
//! inert: it creates no task and starts no effect.

use super::{ExecutionAuthorityRuntime, ManagementError};
use crate::auth::projection::AuthenticatedExecutionConfiguration;
use acteon_core::{FederationImportReceipt, Task, TaskMessage};
use acteon_governance::{
    federation::{
        FederatedDelegationImport, FederationCapabilityCard, FederationEnvelope,
        SignedFederationCapabilityCard, SignedFederationEnvelope, SignedFederationInvocation,
    },
    permit::matches_effect,
};

pub struct FederationImportAdmission<'a> {
    pub namespace: &'a str,
    pub tenant: &'a str,
    pub signed: &'a SignedFederationEnvelope,
    pub authentication: &'a AuthenticatedExecutionConfiguration,
}

pub struct FederationAgentInvocation<'a> {
    pub namespace: &'a str,
    pub tenant: &'a str,
    pub agent_id: &'a str,
    pub signed: &'a SignedFederationInvocation,
    pub message: &'a TaskMessage,
    pub authentication: &'a AuthenticatedExecutionConfiguration,
}

pub struct FederationCapabilityAdmission<'a> {
    pub namespace: &'a str,
    pub tenant: &'a str,
    pub agent_id: &'a str,
    pub authentication: &'a AuthenticatedExecutionConfiguration,
}

impl ExecutionAuthorityRuntime {
    /// Publish a short-lived card only to a currently authenticated configured
    /// peer. The caller identity selects the audience; request data cannot.
    pub async fn federation_capability_card(
        &self,
        request: FederationCapabilityAdmission<'_>,
    ) -> Result<SignedFederationCapabilityCard, ManagementError> {
        let scope = self
            .scopes
            .get(&(request.namespace.into(), request.tenant.into()))
            .ok_or(ManagementError::NotFound)?;
        request
            .authentication
            .verify_authentication_current()
            .await
            .map_err(|_| ManagementError::Forbidden)?;
        let binding = request
            .authentication
            .scope(request.namespace, request.tenant)
            .map_err(|_| ManagementError::Forbidden)?;
        let actor = binding.authentication_source().principal();
        scope
            .prepared
            .verify_authenticated_scope_typed(
                &binding,
                &scope.coordinator,
                self.clock.now().timestamp_millis(),
            )
            .await
            .map_err(|_| ManagementError::Forbidden)?;
        let federation = scope
            .prepared
            .declaration()
            .federation
            .as_ref()
            .ok_or(ManagementError::NotFound)?;
        let peer = federation
            .peers
            .iter()
            .find(|peer| peer.authentication_principal == *actor)
            .ok_or(ManagementError::Forbidden)?;
        let publisher = scope
            .federation_capability
            .as_ref()
            .ok_or(ManagementError::NotFound)?;
        let agent = scope
            .prepared
            .agents
            .get(request.agent_id)
            .ok_or(ManagementError::NotFound)?;
        let now_ms = self.clock.now().timestamp_millis();
        let expires_at_ms = now_ms
            .checked_add(publisher.lifetime_ms)
            .ok_or(ManagementError::Unavailable)?;
        if now_ms < 0 {
            return Err(ManagementError::Unavailable);
        }
        let agent_resource = acteon_core::ResourceRef::new(
            acteon_core::ResourceKind::Agent,
            request.namespace,
            request.tenant,
            request.agent_id,
        )
        .map_err(|_| ManagementError::Invalid)?;
        publisher
            .signer
            .sign(&FederationCapabilityCard::new(
                &federation.local_audience,
                &peer.foreign_domain,
                &publisher.key_id,
                publisher.issuer_epoch,
                now_ms,
                expires_at_ms,
                agent_resource,
                agent.binding.digest(),
                vec![agent.declaration.skill.clone()],
                &publisher.protocol_revision,
                true,
                publisher.peer_finality,
                publisher.authentication_schemes.clone(),
            ))
            .map_err(ManagementError::from)
    }

    /// Invoke one exact imported agent message under current private peer
    /// authentication. Generic recovery never calls this path.
    pub async fn invoke_federated_agent(
        &self,
        request: FederationAgentInvocation<'_>,
    ) -> Result<Task, ManagementError> {
        let scope = self
            .scopes
            .get(&(request.namespace.into(), request.tenant.into()))
            .ok_or(ManagementError::NotFound)?;
        request
            .authentication
            .verify_authentication_current()
            .await
            .map_err(|_| ManagementError::Forbidden)?;
        let binding = request
            .authentication
            .scope(request.namespace, request.tenant)
            .map_err(|_| ManagementError::Forbidden)?;
        let actor = binding.authentication_source().principal();
        scope
            .prepared
            .verify_authenticated_scope_typed(
                &binding,
                &scope.coordinator,
                self.clock.now().timestamp_millis(),
            )
            .await
            .map_err(|_| ManagementError::Forbidden)?;
        let invocation = request.signed.invocation()?;
        let snapshot = scope.coordinator.snapshot().await?;
        let imported = snapshot
            .federation_imports
            .get(&invocation.import.id)
            .filter(|record| record.envelope_digest == invocation.import.envelope_digest)
            .ok_or(ManagementError::Forbidden)?;
        let federation = scope
            .prepared
            .declaration()
            .federation
            .as_ref()
            .ok_or(ManagementError::Forbidden)?;
        if !federation.peers.iter().any(|peer| {
            peer.foreign_domain == imported.issuer_domain
                && peer.authentication_principal == *actor
                && peer.trust_ids.contains(&imported.trust_id)
        }) || imported.agent_resource.id() != request.agent_id
        {
            return Err(ManagementError::Forbidden);
        }
        let runtime = scope
            .agent_bindings
            .get(&(request.agent_id.into(), imported.binding_digest.clone()))
            .ok_or(ManagementError::NotFound)?;
        let task_id = runtime
            .federated_task_id(request.signed, request.message)
            .map_err(federation_runtime_error)?;
        let current = scope
            .prepared
            .agents
            .get(request.agent_id)
            .is_some_and(|agent| agent.binding.digest() == imported.binding_digest);
        if current {
            Self::bind_federated_service(scope, &imported.signed_envelope.envelope()?)?;
            runtime
                .accept_federated(request.signed, request.message)
                .await
                .map_err(federation_runtime_error)?;
        }
        // Recheck the peer credential immediately before any path that can
        // register an unstarted attempt. The coordinator separately rechecks
        // signed trust and budget in the provider-start CAS.
        request
            .authentication
            .verify_authentication_current()
            .await
            .map_err(|_| ManagementError::Forbidden)?;
        scope
            .prepared
            .verify_authenticated_scope_typed(
                &binding,
                &scope.coordinator,
                self.clock.now().timestamp_millis(),
            )
            .await
            .map_err(|_| ManagementError::Forbidden)?;
        runtime
            .resume_federated(task_id, request.signed)
            .await
            .map(|receipt| receipt.task)
            .map_err(federation_runtime_error)
    }

    /// Authenticate one configured foreign peer, bind its signed assertion to
    /// the currently installed local service, and retain the resulting import.
    /// The receipt deliberately states that no execution has started.
    pub async fn import_federation_envelope(
        &self,
        request: FederationImportAdmission<'_>,
    ) -> Result<FederationImportReceipt, ManagementError> {
        let scope = self
            .scopes
            .get(&(request.namespace.into(), request.tenant.into()))
            .ok_or(ManagementError::Forbidden)?;
        request
            .authentication
            .verify_authentication_current()
            .await
            .map_err(|_| ManagementError::Forbidden)?;
        let binding = request
            .authentication
            .scope(request.namespace, request.tenant)
            .map_err(|_| ManagementError::Forbidden)?;
        let actor = binding.authentication_source().principal();
        let stamp = scope
            .prepared
            .verify_authenticated_scope_typed(
                &binding,
                &scope.coordinator,
                self.clock.now().timestamp_millis(),
            )
            .await
            .map_err(|_| ManagementError::Forbidden)?;
        let envelope = request.signed.envelope()?;
        let federation = scope
            .prepared
            .declaration()
            .federation
            .as_ref()
            .ok_or(ManagementError::Forbidden)?;
        let peer = federation
            .peers
            .iter()
            .find(|peer| {
                peer.foreign_domain == envelope.issuer_domain
                    && peer.authentication_principal == *actor
                    && peer.trust_ids.contains(&envelope.trust_id)
            })
            .ok_or(ManagementError::Forbidden)?;
        if envelope.audience != federation.local_audience
            || peer.foreign_domain == federation.local_audience
        {
            return Err(ManagementError::Forbidden);
        }
        Self::bind_federated_service(scope, &envelope)?;
        let imported = scope
            .coordinator
            .import_federated_delegation(FederatedDelegationImport {
                signed: request.signed,
                expected_authority: &stamp,
                clock: self.clock.as_ref(),
            })
            .await?;
        Ok(FederationImportReceipt {
            namespace: request.namespace.into(),
            tenant: request.tenant.into(),
            import_id: imported.id,
            envelope_digest: imported.envelope_digest,
            trust_id: imported.trust_id,
            trust_revision: imported.trust_revision,
            issuer_domain: imported.issuer_domain,
            source: imported.source,
            target: imported.target,
            agent_resource: imported.agent_resource,
            binding_digest: imported.binding_digest,
            skill: imported.skill,
            root_id: imported.root_id,
            expires_at_ms: imported.expires_at_ms,
            accepted_at_ms: imported.accepted_at_ms,
            execution_started: false,
        })
    }

    fn bind_federated_service(
        scope: &super::InstalledScope,
        envelope: &FederationEnvelope,
    ) -> Result<(), ManagementError> {
        let agent = scope
            .prepared
            .agents
            .get(envelope.agent_resource.id())
            .ok_or(ManagementError::Forbidden)?;
        let direct = agent
            .binding
            .service_plan()
            .ok_or(ManagementError::Unavailable)?
            .direct_effects();
        if agent.declaration.principal != envelope.target
            || agent.declaration.skill != envelope.skill
            || agent.binding.agent_resource() != &envelope.agent_resource
            || agent.binding.digest() != envelope.binding_digest
            || agent.binding.ingress_effect() != &envelope.ingress_effect
            || envelope
                .effects
                .iter()
                .any(|effect| !direct.iter().any(|allowed| matches_effect(allowed, effect)))
        {
            return Err(ManagementError::Forbidden);
        }
        Ok(())
    }
}

fn federation_runtime_error(
    error: acteon_gateway::agent_runtime::AgentRuntimeError,
) -> ManagementError {
    use acteon_executor::governed::GovernedProviderError;
    use acteon_gateway::agent_runtime::AgentRuntimeError;
    match error {
        AgentRuntimeError::Invalid => ManagementError::Invalid,
        AgentRuntimeError::Conflict => ManagementError::Conflict,
        AgentRuntimeError::Missing => ManagementError::NotFound,
        AgentRuntimeError::State(_)
        | AgentRuntimeError::Encoding(_)
        | AgentRuntimeError::Task(_) => ManagementError::Unavailable,
        AgentRuntimeError::Context(_) | AgentRuntimeError::Authority(_) => {
            ManagementError::Forbidden
        }
        AgentRuntimeError::Execution(error) => match error {
            GovernedProviderError::Invalid => ManagementError::Invalid,
            GovernedProviderError::Ownership | GovernedProviderError::Admission(_) => {
                ManagementError::Forbidden
            }
            GovernedProviderError::Conflict => ManagementError::Conflict,
            GovernedProviderError::Unavailable => ManagementError::Unavailable,
        },
    }
}
