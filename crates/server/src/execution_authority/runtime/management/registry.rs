//! Registry control from original private operator authentication and exact agent bounds.
use super::{ExecutionAuthorityRuntime, ManagementError, ManagementObservation};
use crate::auth::projection::AuthenticatedExecutionConfiguration;
use acteon_core::{
    Agent, AgentCard, GovernanceRegistryMutationReceipt, GovernanceRegistryMutationRequest,
    GovernanceRegistryProjection, GovernanceRegistryProjectionView, ResourceKind, ResourceRef,
};
use acteon_governance::{
    AuthorityChange,
    control::{ControlChangeAuthorization, ControlChangeCeiling},
    registry::{RegistryProjectionKind, registry_projection_digest},
};
use acteon_state::{KeyKind, StateKey};

fn projection_kind(projection: GovernanceRegistryProjection) -> RegistryProjectionKind {
    match projection {
        GovernanceRegistryProjection::Agent => RegistryProjectionKind::Agent,
        GovernanceRegistryProjection::Card => RegistryProjectionKind::Card,
    }
}
fn projection_key(
    namespace: &str,
    tenant: &str,
    agent: &str,
    projection: GovernanceRegistryProjection,
) -> StateKey {
    StateKey::new(
        namespace,
        tenant,
        match projection {
            GovernanceRegistryProjection::Agent => KeyKind::BusAgent,
            GovernanceRegistryProjection::Card => KeyKind::BusAgentCard,
        },
        agent,
    )
}
fn require_agent(
    observation: &ManagementObservation<'_>,
    agent: &str,
) -> Result<(), ManagementError> {
    if !observation.policy.can_intervene || !observation.policy.agents.iter().any(|id| id == agent)
    {
        return Err(ManagementError::Forbidden);
    }
    Ok(())
}

fn normalize_projection(
    request: &GovernanceRegistryMutationRequest,
    actor: &str,
) -> Result<Option<String>, ManagementError> {
    let Some(value) = &request.value else {
        return Ok(None);
    };
    let value = match request.projection {
        GovernanceRegistryProjection::Agent => {
            let mut agent: Agent =
                serde_json::from_value(value.clone()).map_err(|_| ManagementError::Invalid)?;
            agent.validate().map_err(|_| ManagementError::Invalid)?;
            if agent.agent_id != request.agent_id
                || agent.namespace != request.namespace
                || agent.tenant != request.tenant
                || agent.labels.len() > 32
                || agent
                    .labels
                    .iter()
                    .any(|(k, v)| k.len() > 256 || v.len() > 4096 || k.starts_with("_sys."))
            {
                return Err(ManagementError::Invalid);
            }
            // Descriptive audit fields cannot impersonate a different operator.
            if agent.admin_set_by.is_some()
                || agent.admin_state != acteon_core::AgentAdminState::Active
            {
                agent.admin_set_by = Some(actor.into());
            }
            serde_json::to_value(agent).map_err(|_| ManagementError::Invalid)?
        }
        GovernanceRegistryProjection::Card => {
            let card: AgentCard =
                serde_json::from_value(value.clone()).map_err(|_| ManagementError::Invalid)?;
            card.validate().map_err(|_| ManagementError::Invalid)?;
            if card.agent_id != request.agent_id
                || card.namespace != request.namespace
                || card.tenant != request.tenant
            {
                return Err(ManagementError::Invalid);
            }
            serde_json::to_value(card).map_err(|_| ManagementError::Invalid)?
        }
    };
    serde_json::to_string(&value)
        .map(Some)
        .map_err(|_| ManagementError::Invalid)
}

impl ExecutionAuthorityRuntime {
    /// Read the original projection and its backend version under current exact control bounds.
    pub async fn inspect_registry_projection(
        &self,
        namespace: &str,
        tenant: &str,
        agent_id: &str,
        projection: GovernanceRegistryProjection,
        authentication: &AuthenticatedExecutionConfiguration,
    ) -> Result<GovernanceRegistryProjectionView, ManagementError> {
        let observation = self
            .management_observation(namespace, tenant, authentication)
            .await?;
        require_agent(&observation, agent_id)?;
        let resource = ResourceRef::new(ResourceKind::Agent, namespace, tenant, agent_id)
            .map_err(|_| ManagementError::Invalid)?;
        let snapshot = self
            .revalidate_management(&observation, authentication)
            .await?;
        let actual = self
            .state
            .get_versioned(&projection_key(namespace, tenant, agent_id, projection))
            .await
            .map_err(|_| ManagementError::Unavailable)?;
        let (value, version) = if let Some((raw, version)) = actual {
            if raw.len() > 256 * 1024 {
                return Err(ManagementError::Unavailable);
            }
            (
                Some(serde_json::from_str(&raw).map_err(|_| ManagementError::Unavailable)?),
                Some(version),
            )
        } else {
            (None, None)
        };
        let qualified = snapshot.agent_registry.get(agent_id);
        Ok(GovernanceRegistryProjectionView {
            namespace: namespace.into(),
            tenant: tenant.into(),
            agent_id: agent_id.into(),
            agent_resource: resource,
            projection,
            registry_revision: qualified.map_or(0, |r| r.qualification.revision),
            qualification_retired: qualified.map(|r| r.retired),
            value,
            version,
        })
    }

    pub async fn mutate_registry_projection(
        &self,
        request: GovernanceRegistryMutationRequest,
        authentication: &AuthenticatedExecutionConfiguration,
    ) -> Result<GovernanceRegistryMutationReceipt, ManagementError> {
        let observation = self
            .management_observation(&request.namespace, &request.tenant, authentication)
            .await?;
        require_agent(&observation, &request.agent_id)?;
        let agent = ResourceRef::new(
            ResourceKind::Agent,
            &request.namespace,
            &request.tenant,
            &request.agent_id,
        )
        .map_err(|_| ManagementError::Invalid)?;
        if request.expected_projection_version == Some(0) {
            return Err(ManagementError::Invalid);
        }
        let raw = normalize_projection(&request, observation.policy.principal.id())?;
        let projection = projection_kind(request.projection);
        let digest = registry_projection_digest(
            projection,
            request.expected_projection_version,
            raw.as_deref(),
        );
        self.revalidate_management(&observation, authentication)
            .await?;
        let ceiling = ControlChangeCeiling {
            actor: observation.policy.principal.clone(),
            subjects: observation.policy.subjects.clone(),
            resources: vec![agent.clone()],
            valid_from_ms: observation.policy.valid_from_ms,
            deadline_ms: observation.policy.limits.deadline_ms,
        };
        let record = observation
            .scope
            .coordinator
            .change_evaluated(
                &request.change_id,
                AuthorityChange::BeginAgentRegistryMutation {
                    agent,
                    expected_revision: request.expected_registry_revision,
                    projection,
                    expected_projection_version: request.expected_projection_version,
                    input_digest: digest.clone(),
                },
                &request.reason,
                ControlChangeAuthorization {
                    ceiling: &ceiling,
                    evaluated_authority: &observation.stamp,
                    clock: self.clock.as_ref(),
                },
            )
            .await?;
        // Reauthenticate after staging; no stale role or scope projection authorizes delivery.
        let current = self
            .management_observation(&request.namespace, &request.tenant, authentication)
            .await?;
        require_agent(&current, &request.agent_id)?;
        self.revalidate_management(&current, authentication).await?;
        observation
            .scope
            .coordinator
            .execute_agent_registry_mutation(
                &request.change_id,
                raw.as_deref(),
                ControlChangeAuthorization {
                    ceiling: &ceiling,
                    evaluated_authority: &current.stamp,
                    clock: self.clock.as_ref(),
                },
            )
            .await?;
        Ok(GovernanceRegistryMutationReceipt {
            namespace: request.namespace,
            tenant: request.tenant,
            agent_id: request.agent_id,
            change_id: request.change_id,
            projection: request.projection,
            expected_registry_revision: request.expected_registry_revision,
            input_digest: digest,
            actor: record.actor,
            delivery_complete: true,
            applied: true,
        })
    }
}
