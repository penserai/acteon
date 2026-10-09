//! Authenticated cross-domain admission. Importing signed authority remains
//! inert: it creates no task and starts no effect.

use super::{ExecutionAuthorityRuntime, ManagementError};
use crate::auth::projection::AuthenticatedExecutionConfiguration;
use acteon_core::FederationImportReceipt;
use acteon_governance::{
    federation::{FederatedDelegationImport, FederationEnvelope, SignedFederationEnvelope},
    permit::matches_effect,
};

pub struct FederationImportAdmission<'a> {
    pub namespace: &'a str,
    pub tenant: &'a str,
    pub signed: &'a SignedFederationEnvelope,
    pub authentication: &'a AuthenticatedExecutionConfiguration,
}

impl ExecutionAuthorityRuntime {
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
