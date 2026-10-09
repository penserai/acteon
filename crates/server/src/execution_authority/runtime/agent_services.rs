//! Inbound service admission from original private caller and recipient proofs.
use super::{AgentServiceError, ExecutionAuthorityRuntime, InstalledScope};
use crate::{
    auth::{
        AuthProvider,
        projection::{AuthenticatedExecutionConfiguration, ScopedCredentialBinding},
    },
    execution_authority::{
        PreparedExecutionScope,
        agent_services::{AgentServiceGrantDeclaration, PreparedAgentService},
    },
};
use acteon_core::{ExecutionContextReference, PrincipalKind, Task, TaskMessage};
use acteon_executor::governed::governed_provider_input_digest;
use acteon_gateway::agent_runtime::{
    ACCEPTANCE_KIND, AgentProviderRuntime, AgentTaskStopReceipt, accepted_agent_binding_digest,
};
use acteon_governance::{
    AuthorityCoordinator, RootBudgetLimits,
    context::{
        ContextBinding, DelegatedContextAdmission, DelegatingRootAdmission, ExecutionContextHandle,
        RootContextAdmission,
    },
    delegation_policy::{
        DelegationGrant, DelegationGrantReference, DelegationIssuanceCeiling,
        EvaluatedDelegationPublication,
    },
    permit::PermitReference,
    registry::{AgentRegistryIssuanceCeiling, AgentRegistryQualification},
};
use acteon_state::{KeyKind, StateKey};
use acteon_time::Clock;
use sha2::{Digest, Sha256};

/// Original private middleware proof and actual message. No deserializer.
pub struct AgentServiceRequest<'a> {
    pub namespace: &'a str,
    pub tenant: &'a str,
    pub agent_id: &'a str,
    pub message: &'a TaskMessage,
    pub authentication: &'a AuthenticatedExecutionConfiguration,
    pub auth_provider: &'a AuthProvider,
    pub parent: Option<AgentServiceParent<'a>>,
}
/// Transport receipt; the host retains source provenance separately from model data.
pub struct AgentServiceAcceptance {
    pub task: Task,
    pub task_version: u64,
    pub source_context: ExecutionContextReference,
}

pub struct AgentServiceTaskObservation {
    pub task: Task,
    pub task_version: u64,
}

pub struct AgentServiceObservation<'a> {
    pub namespace: &'a str,
    pub tenant: &'a str,
    pub agent_id: &'a str,
    pub task_id: uuid::Uuid,
    pub authentication: &'a AuthenticatedExecutionConfiguration,
    pub source_context: Option<&'a ExecutionContextReference>,
}

/// One exact response to the task's active `InputRequired` challenge. The
/// authenticated original source is recovered through the same private
/// observation boundary before the runtime consumes any message content.
pub struct AgentServiceContinuation<'a> {
    pub observation: AgentServiceObservation<'a>,
    pub challenge_id: &'a str,
    pub response: &'a TaskMessage,
}

/// Opaque references are verified against original private caller authentication.
pub struct AgentServiceParent<'a> {
    pub context: &'a ExecutionContextReference,
    pub permits: &'a [PermitReference],
}

impl ExecutionAuthorityRuntime {
    // Preflight all services before any deployment permit or grant is published.
    // Private authentication enrollment may exist already; it is never a permit.
    pub(super) async fn validate_agent_credentials(
        &self,
        authentication: Option<&AuthProvider>,
    ) -> Result<(), String> {
        for scope in self.scopes.values() {
            for agent in scope.prepared.agents.values() {
                self.authenticate_agent_recipient(
                    scope,
                    agent,
                    authentication.ok_or("agent services require private authentication")?,
                )
                .await
                .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    async fn authenticate_agent_recipient(
        &self,
        scope: &InstalledScope,
        agent: &PreparedAgentService,
        authentication: &AuthProvider,
    ) -> Result<ScopedCredentialBinding, AgentServiceError> {
        let secret = zeroize::Zeroizing::new(
            std::env::var(&agent.declaration.recipient_key_env)
                .map_err(|_| AgentServiceError::Unavailable)?,
        );
        let recipient = authentication
            .authenticate_service_key(&secret)
            .await
            .map_err(|_| AgentServiceError::Unavailable)?;
        recipient
            .verify_authentication_current()
            .await
            .map_err(|_| AgentServiceError::Unavailable)?;
        let declaration = scope.prepared.declaration();
        let binding = recipient
            .scope(&declaration.namespace, &declaration.tenant)
            .map_err(|_| AgentServiceError::Unavailable)?;
        if binding.authentication_source().principal() != &agent.declaration.principal {
            return Err(AgentServiceError::Unavailable);
        }
        scope
            .prepared
            .verify_authenticated_scope_typed(
                &binding,
                &scope.coordinator,
                self.clock.now().timestamp_millis(),
            )
            .await
            .map_err(|_| AgentServiceError::Unavailable)?;
        let snapshot = scope
            .coordinator
            .snapshot()
            .await
            .map_err(AgentServiceError::from)?;
        let credential = snapshot
            .credentials
            .get(&binding.credential_reference().id)
            .ok_or(AgentServiceError::Unavailable)?;
        if !credential
            .authority
            .ceiling
            .effects
            .iter()
            .any(|effect| acteon_governance::permit::matches_effect(effect, agent.bound.effect()))
        {
            return Err(AgentServiceError::Unavailable);
        }
        Ok(binding)
    }

    fn registry_qualification(agent: &PreparedAgentService) -> AgentRegistryQualification {
        AgentRegistryQualification {
            agent: agent.binding.agent_resource().clone(),
            target: agent.declaration.principal.clone(),
            revision: agent.declaration.registry_revision,
            bindings: std::collections::BTreeMap::from([(
                agent.declaration.skill.clone(),
                agent.binding.digest().into(),
            )]),
        }
    }

    pub(super) async fn publish_agent_registry(
        prepared: &PreparedExecutionScope,
        coordinator: &AuthorityCoordinator,
        clock: &dyn Clock,
    ) -> Result<(), String> {
        let scope = prepared.declaration();
        for agent in prepared.agents.values() {
            let qualification = Self::registry_qualification(agent);
            let ceiling = AgentRegistryIssuanceCeiling {
                issuer: scope.publisher.clone(),
                approved: vec![qualification.clone()],
                valid_from_ms: scope.valid_from_ms,
                deadline_ms: scope.credential_limits.deadline_ms,
            };
            let change = format!(
                "deployment-agent-registry/{:x}",
                Sha256::digest(
                    serde_json::to_vec(&(qualification.agent.id(), qualification.revision,))
                        .map_err(|_| "invalid registry qualification")?
                )
            );
            coordinator
                .publish_agent_registry(
                    &change,
                    qualification.clone(),
                    qualification.revision - 1,
                    &ceiling,
                    &coordinator
                        .snapshot()
                        .await
                        .map_err(|_| "registry authority unavailable")?
                        .stamp(),
                    "explicit deployment registry qualification",
                    clock,
                )
                .await
                .map_err(|_| "registry qualification publication refused")?;
            let snapshot = coordinator
                .snapshot()
                .await
                .map_err(|_| "registry authority unavailable")?;
            let current = snapshot
                .agent_registry
                .get(qualification.agent.id())
                .ok_or("registry qualification missing")?;
            if current.retired || current.qualification != qualification {
                return Err("registry qualification retired or replaced".into());
            }
        }
        Ok(())
    }

    pub(super) async fn publish_agent_grants(
        prepared: &PreparedExecutionScope,
        coordinator: &AuthorityCoordinator,
        clock: &dyn Clock,
    ) -> Result<(), String> {
        let scope = prepared.declaration();
        for agent in prepared.agents.values() {
            let ceiling = DelegationIssuanceCeiling {
                issuer: scope.publisher.clone(),
                sources: agent
                    .declaration
                    .grants
                    .iter()
                    .map(|g| g.source.clone())
                    .collect(),
                targets: vec![agent.declaration.principal.clone()],
                binding_digests: vec![agent.binding.digest().into()],
                ingress_effects: vec![agent.binding.ingress_effect().clone()],
                effects: agent
                    .binding
                    .service_plan()
                    .ok_or("missing service plan")?
                    .intent()
                    .to_vec(),
                valid_from_ms: scope.valid_from_ms,
                limits: scope.credential_limits.clone(),
                max_depth: 15,
            };
            ceiling
                .validate()
                .map_err(|_| "invalid service publication ceiling")?;
            for declared in &agent.declaration.grants {
                let grant = DelegationGrant {
                    id: declared.id.clone(),
                    revision: declared.revision,
                    source: declared.source.clone(),
                    target: agent.declaration.principal.clone(),
                    agent_resource: agent.binding.agent_resource().clone(),
                    binding_digest: agent.binding.digest().into(),
                    skill: agent.declaration.skill.clone(),
                    ingress_effect: agent.binding.ingress_effect().clone(),
                    effects: ceiling.effects.clone(),
                    valid_from_ms: declared.valid_from_ms,
                    limits: declared.limits.clone(),
                    max_depth: declared.max_depth,
                };
                let change = format!(
                    "deployment-service-grant/{:x}",
                    Sha256::digest(
                        serde_json::to_vec(&(&grant.id, grant.revision))
                            .map_err(|_| "invalid service grant")?
                    )
                );
                coordinator
                    .publish_delegation_grant(EvaluatedDelegationPublication {
                        change_id: &change,
                        grant,
                        expected_revision: declared.revision - 1,
                        ceiling: &ceiling,
                        evaluated_authority: &coordinator
                            .snapshot()
                            .await
                            .map_err(|_| "service authority unavailable")?
                            .stamp(),
                        reason: "explicit deployment service grant",
                        clock,
                    })
                    .await
                    .map_err(|_| "service grant publication refused")?;
            }
        }
        Ok(())
    }

    /// Persist bounded service work. Every actual provider call still uses the
    /// governed executor; acceptance is not an effect-start reservation.
    // Keep the ordered qualification and authority checks visible together.
    #[allow(clippy::too_many_lines)]
    pub async fn accept_agent_service(
        &self,
        request: AgentServiceRequest<'_>,
    ) -> Result<AgentServiceAcceptance, AgentServiceError> {
        let scope = self
            .scopes
            .get(&(request.namespace.into(), request.tenant.into()))
            .ok_or(AgentServiceError::NotFound)?;
        request
            .authentication
            .verify_authentication_current()
            .await
            .map_err(|e| AgentServiceError::authentication(&e))?;
        let source = request
            .authentication
            .scope(request.namespace, request.tenant)
            .map_err(|_| AgentServiceError::Forbidden)?;
        let actor = source.authentication_source().principal();
        if let Some(replayed) = self
            .replay_accepted_agent_service(scope, &request, &source)
            .await?
        {
            return Ok(replayed);
        }
        let stamp = scope
            .prepared
            .verify_authenticated_scope_typed(
                &source,
                &scope.coordinator,
                self.clock.now().timestamp_millis(),
            )
            .await?;
        let (_, agent, runtime) =
            self.service(request.namespace, request.tenant, request.agent_id)?;
        let qualification = Self::registry_qualification(agent);
        let snapshot = scope
            .coordinator
            .snapshot()
            .await
            .map_err(AgentServiceError::from)?;
        let current = snapshot
            .agent_registry
            .get(qualification.agent.id())
            .ok_or(AgentServiceError::Unavailable)?;
        if current.retired || current.qualification != qualification {
            return Err(AgentServiceError::Forbidden);
        }
        let declared = agent
            .declaration
            .grants
            .iter()
            .find(|grant| &grant.source == actor)
            .ok_or(AgentServiceError::Forbidden)?;
        let action = runtime
            .prepare_message(request.message)
            .map_err(|_| AgentServiceError::Invalid)?;
        let provider_digest =
            governed_provider_input_digest(&action).map_err(|_| AgentServiceError::Invalid)?;
        let source_digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&serde_json::json!({"domain":"acteon.agent-service-input.v1", "binding":agent.binding.digest(), "input":provider_digest})).map_err(|_| AgentServiceError::Invalid)?));
        let binding = self
            .authenticate_agent_recipient(scope, agent, request.auth_provider)
            .await?;
        let parent = if let Some(parent) = &request.parent {
            let context = scope
                .contexts
                .recover_reference_for_observation(parent.context)
                .await
                .map_err(AgentServiceError::from)?;
            if context.principal() != actor
                || context.credential_authority() != Some(source.credential_reference())
            {
                return Err(AgentServiceError::Forbidden);
            }
            context
        } else {
            let mut admission = RootContextAdmission {
                handle: ExecutionContextHandle::new(),
                binding: ContextBinding {
                    execution_id: uuid::Uuid::new_v4(),
                    principal: actor.clone(),
                    request_digest: source_digest,
                },
                credential_id: source.credential_reference().id.clone(),
                auth_method: source.authentication_source().auth_method().into(),
                accepted_ceiling_revision: String::new(),
                accepted_effects: vec![agent.binding.ingress_effect().clone()],
                deadline_ms: self
                    .service_deadline(scope, declared)
                    .map_err(|_| AgentServiceError::Unavailable)?,
                evaluated_authority: stamp,
            };
            let limits = scope
                .coordinator
                .attenuate_permitted_root_limits(
                    &admission,
                    &declared.source_permits,
                    self.service_limits(scope, declared)
                        .map_err(|_| AgentServiceError::Unavailable)?,
                    self.clock.now().timestamp_millis(),
                )
                .await
                .map_err(AgentServiceError::from)?;
            admission.deadline_ms = limits.deadline_ms;
            let representation = scope
                .coordinator
                .evaluate_permit_representation(
                    acteon_governance::workforce::WorkforcePermitAdmission {
                        permits: &declared.source_permits,
                        initiator: actor,
                        job_class: &agent.declaration.skill,
                        admission: &admission,
                        limits: &limits,
                        clock: self.clock.as_ref(),
                    },
                )
                .await
                .map_err(AgentServiceError::from)?;
            let key = service_key(
                "root",
                &[actor.id(), request.agent_id, &request.message.message_id],
            );
            scope
                .contexts
                .capture_delegating_root(DelegatingRootAdmission {
                    admission_key: &key,
                    admission,
                    permits: &declared.source_permits,
                    credential: source.credential_reference().clone(),
                    representation: representation.as_ref(),
                    grants: vec![grant_reference(declared)],
                    limits,
                    clock: self.clock.as_ref(),
                })
                .await
                .map_err(AgentServiceError::from)?
        };
        let stamp = scope
            .prepared
            .verify_authenticated_scope_typed(
                &binding,
                &scope.coordinator,
                self.clock.now().timestamp_millis(),
            )
            .await?;
        let mut admission = RootContextAdmission {
            handle: ExecutionContextHandle::new(),
            binding: ContextBinding {
                execution_id: uuid::Uuid::new_v4(),
                principal: agent.declaration.principal.clone(),
                request_digest: provider_digest,
            },
            credential_id: binding.credential_reference().id.clone(),
            auth_method: binding.authentication_source().auth_method().into(),
            accepted_ceiling_revision: String::new(),
            accepted_effects: agent
                .binding
                .service_plan()
                .ok_or(AgentServiceError::Unavailable)?
                .direct_effects()
                .to_vec(),
            deadline_ms: self
                .service_deadline(scope, declared)
                .map_err(|_| AgentServiceError::Unavailable)?,
            evaluated_authority: stamp,
        };
        let parent_limits = scope
            .coordinator
            .snapshot()
            .await
            .map_err(AgentServiceError::from)?
            .roots
            .get(&parent.execution_id().to_string())
            .ok_or(AgentServiceError::Conflict)?
            .limits
            .clone();
        let mut limits = self
            .service_limits(scope, declared)
            .map_err(|_| AgentServiceError::Unavailable)?;
        limits.max_units = limits.max_units.min(parent_limits.max_units);
        limits.max_concurrent = limits.max_concurrent.min(parent_limits.max_concurrent);
        limits.deadline_ms = limits.deadline_ms.min(parent_limits.deadline_ms);
        admission.deadline_ms = limits.deadline_ms;
        let representation = scope
            .coordinator
            .evaluate_permit_representation(
                acteon_governance::workforce::WorkforcePermitAdmission {
                    permits: &agent.declaration.recipient_permits,
                    initiator: parent.principal(),
                    job_class: agent.bound.action_type(),
                    admission: &admission,
                    limits: &limits,
                    clock: self.clock.as_ref(),
                },
            )
            .await
            .map_err(AgentServiceError::from)?;
        let key = service_key(
            "recipient",
            &[
                &parent.execution_id().to_string(),
                request.agent_id,
                &request.message.message_id,
            ],
        );
        let source_permits = request
            .parent
            .as_ref()
            .map_or(declared.source_permits.as_slice(), |p| p.permits);
        let onward_grants = agent
            .declaration
            .onward_agents
            .iter()
            .map(|target_id| {
                scope
                    .prepared
                    .agents
                    .get(target_id)
                    .and_then(|target| {
                        target
                            .declaration
                            .grants
                            .iter()
                            .find(|grant| grant.source == agent.declaration.principal)
                    })
                    .map(grant_reference)
                    .ok_or(AgentServiceError::Unavailable)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let child = scope
            .contexts
            .capture_delegated_child(DelegatedContextAdmission {
                admission_key: &key,
                parent: &parent,
                parent_permits: source_permits,
                grant: grant_reference(declared),
                binding_digest: agent.binding.digest(),
                recipient: admission,
                recipient_permits: &agent.declaration.recipient_permits,
                credential: binding.credential_reference().clone(),
                representation: representation.as_ref(),
                intent_effects: agent
                    .binding
                    .service_plan()
                    .ok_or(AgentServiceError::Unavailable)?
                    .intent()
                    .to_vec(),
                onward_grants,
                limits,
                clock: self.clock.as_ref(),
            })
            .await
            .map_err(AgentServiceError::from)?;
        let task = runtime
            .accept(
                &child,
                &agent.declaration.recipient_permits,
                request.message,
            )
            .await
            .map_err(AgentServiceError::from)?;
        self.accepted_service_snapshot(
            request.namespace,
            request.tenant,
            child.execution_id(),
            &task,
            parent
                .reference()
                .map_err(|_| AgentServiceError::Unavailable)?,
        )
        .await
    }

    async fn replay_source_context(
        scope: &InstalledScope,
        request: &AgentServiceRequest<'_>,
        caller: &ScopedCredentialBinding,
    ) -> Result<Option<acteon_governance::context::VerifiedExecutionContext>, AgentServiceError>
    {
        let actor = caller.authentication_source().principal();
        if let Some(parent) = &request.parent {
            let context = scope
                .contexts
                .recover_reference_for_observation(parent.context)
                .await
                .map_err(AgentServiceError::from)?;
            if !Self::service_source_matches(&context, caller) {
                return Err(AgentServiceError::Forbidden);
            }
            Ok(Some(context))
        } else {
            let key = service_key(
                "root",
                &[actor.id(), request.agent_id, &request.message.message_id],
            );
            let source = scope
                .contexts
                .inspect_root_admission(&key)
                .await
                .map_err(AgentServiceError::from)?;
            Ok(source)
        }
    }

    fn service_source_matches(
        source: &acteon_governance::context::VerifiedExecutionContext,
        caller: &ScopedCredentialBinding,
    ) -> bool {
        source.principal() == caller.authentication_source().principal()
            && source
                .credential_authority()
                .is_some_and(|credential| credential.id == caller.credential_reference().id)
            && source.auth_method() == caller.authentication_source().auth_method()
    }

    async fn replay_accepted_agent_service(
        &self,
        scope: &InstalledScope,
        request: &AgentServiceRequest<'_>,
        caller: &ScopedCredentialBinding,
    ) -> Result<Option<AgentServiceAcceptance>, AgentServiceError> {
        let source = Self::replay_source_context(scope, request, caller).await?;
        let Some(source) = source else {
            return Ok(None);
        };
        if !Self::service_source_matches(&source, caller) {
            return Err(AgentServiceError::Conflict);
        }
        let child_key = service_key(
            "recipient",
            &[
                &source.execution_id().to_string(),
                request.agent_id,
                &request.message.message_id,
            ],
        );
        let Some(child) = scope
            .contexts
            .inspect_child_admission(&child_key)
            .await
            .map_err(AgentServiceError::from)?
        else {
            return Ok(None);
        };
        let child_source = child
            .immediate_service_source()
            .ok_or(AgentServiceError::Conflict)?;
        if child_source
            .reference()
            .map_err(|_| AgentServiceError::Conflict)?
            != source
                .reference()
                .map_err(|_| AgentServiceError::Conflict)?
        {
            return Err(AgentServiceError::Conflict);
        }
        let runtime = self
            .service_runtime_for_task(
                request.namespace,
                request.tenant,
                request.agent_id,
                child.execution_id(),
            )
            .await?;
        let action = runtime
            .prepare_message(request.message)
            .map_err(AgentServiceError::from)?;
        let provider_digest =
            governed_provider_input_digest(&action).map_err(|_| AgentServiceError::Invalid)?;
        if provider_digest
            != child
                .reference()
                .map_err(|_| AgentServiceError::Conflict)?
                .request_digest()
        {
            return Err(AgentServiceError::Conflict);
        }
        if request.parent.is_none() {
            let expected_source = format!(
                "{:x}",
                Sha256::digest(
                    serde_json::to_vec(&serde_json::json!({
                        "domain":"acteon.agent-service-input.v1",
                        "binding":runtime.binding_digest(),
                        "input":provider_digest,
                    }))
                    .map_err(|_| AgentServiceError::Invalid)?
                )
            );
            if source
                .reference()
                .map_err(|_| AgentServiceError::Conflict)?
                .request_digest()
                != expected_source
            {
                return Err(AgentServiceError::Conflict);
            }
        }
        let observed = runtime
            .observe(child.execution_id())
            .await
            .map_err(AgentServiceError::observation)?;
        self.accepted_service_snapshot(
            request.namespace,
            request.tenant,
            child.execution_id(),
            &observed.task,
            source
                .reference()
                .map_err(|_| AgentServiceError::Unavailable)?,
        )
        .await
        .map(Some)
    }

    async fn accepted_service_snapshot(
        &self,
        namespace: &str,
        tenant: &str,
        task_id: uuid::Uuid,
        observed: &Task,
        source_context: ExecutionContextReference,
    ) -> Result<AgentServiceAcceptance, AgentServiceError> {
        let versioned = self
            .load_versioned_service_task(namespace, tenant, task_id, observed)
            .await?;
        Ok(AgentServiceAcceptance {
            task: versioned.task,
            task_version: versioned.task_version,
            source_context,
        })
    }

    /// Observe only the authenticated original source. Shared agents must also
    /// present the exact job context, so one agent identity cannot join requesters.
    pub async fn observe_agent_service(
        &self,
        request: AgentServiceObservation<'_>,
    ) -> Result<AgentServiceTaskObservation, AgentServiceError> {
        let task_id = request.task_id;
        let (runtime, _) = self.authorize_service_observation(request).await?;
        let observed = runtime
            .observe(task_id)
            .await
            .map_err(AgentServiceError::observation)?;
        self.load_versioned_service_task(
            &observed.task.namespace,
            &observed.task.tenant,
            task_id,
            &observed.task,
        )
        .await
    }

    /// Resume an accepted agent service with a durable, separately governed
    /// provider invocation that receives the response and complete task history.
    pub async fn continue_agent_service(
        &self,
        request: AgentServiceContinuation<'_>,
    ) -> Result<AgentServiceTaskObservation, AgentServiceError> {
        let task_id = request.observation.task_id;
        let (runtime, source) = self
            .authorize_service_observation(request.observation)
            .await?;
        let continued = runtime
            .continue_input(task_id, request.challenge_id, request.response, &source)
            .await
            .map_err(AgentServiceError::from)?;
        self.load_versioned_service_task(
            &continued.task.namespace,
            &continued.task.tenant,
            task_id,
            &continued.task,
        )
        .await
    }

    async fn load_versioned_service_task(
        &self,
        namespace: &str,
        tenant: &str,
        task_id: uuid::Uuid,
        observed: &Task,
    ) -> Result<AgentServiceTaskObservation, AgentServiceError> {
        let key = StateKey::new(namespace, tenant, KeyKind::A2aTask, task_id.to_string());
        let (raw, task_version) = self
            .state
            .get_versioned(&key)
            .await
            .map_err(|_| AgentServiceError::Unavailable)?
            .ok_or(AgentServiceError::NotFound)?;
        let task: Task = serde_json::from_str(&raw).map_err(|_| AgentServiceError::Unavailable)?;
        if task.validate().is_err()
            || task.id != task_id.to_string()
            || task.namespace != namespace
            || task.tenant != tenant
            || task.updated_at < observed.updated_at
        {
            return Err(AgentServiceError::Conflict);
        }
        Ok(AgentServiceTaskObservation { task, task_version })
    }

    /// Stop only the original accepted recipient subtree. Current private
    /// authentication and exact source provenance are required, as for reads.
    pub async fn stop_agent_service(
        &self,
        request: AgentServiceObservation<'_>,
    ) -> Result<AgentTaskStopReceipt, AgentServiceError> {
        let task_id = request.task_id;
        let (runtime, source) = self.authorize_service_observation(request).await?;
        runtime
            .stop(task_id, &source)
            .await
            .map_err(AgentServiceError::from)
    }

    async fn authorize_service_observation<'a>(
        &'a self,
        request: AgentServiceObservation<'_>,
    ) -> Result<
        (
            &'a AgentProviderRuntime,
            acteon_governance::context::VerifiedExecutionContext,
        ),
        AgentServiceError,
    > {
        request
            .authentication
            .verify_authentication_current()
            .await
            .map_err(|e| AgentServiceError::authentication(&e))?;
        let caller = request
            .authentication
            .scope(request.namespace, request.tenant)
            .map_err(|_| AgentServiceError::Forbidden)?;
        let runtime = self
            .service_runtime_for_task(
                request.namespace,
                request.tenant,
                request.agent_id,
                request.task_id,
            )
            .await?;
        let source = runtime
            .source_context(request.task_id)
            .await
            .map_err(AgentServiceError::observation)?;
        let reference = source
            .reference()
            .map_err(|_| AgentServiceError::NotFound)?;
        let actor = caller.authentication_source().principal();
        if source.principal() != actor
            || source.credential_authority().map(|c| c.id.as_str())
                != Some(caller.credential_reference().id.as_str())
            || source.auth_method() != caller.authentication_source().auth_method()
            || request.source_context.is_some_and(|r| r != &reference)
            || (actor.kind() == PrincipalKind::Agent && request.source_context != Some(&reference))
        {
            return Err(AgentServiceError::NotFound);
        }
        Ok((runtime, source))
    }

    pub(super) async fn service_runtime_for_task(
        &self,
        namespace: &str,
        tenant: &str,
        agent_id: &str,
        task_id: uuid::Uuid,
    ) -> Result<&AgentProviderRuntime, AgentServiceError> {
        let scope = self
            .scopes
            .get(&(namespace.into(), tenant.into()))
            .ok_or(AgentServiceError::NotFound)?;
        let raw = self
            .state
            .get(&StateKey::new(
                namespace,
                tenant,
                KeyKind::Custom(ACCEPTANCE_KIND.into()),
                task_id.to_string(),
            ))
            .await
            .map_err(|_| AgentServiceError::Unavailable)?
            .ok_or(AgentServiceError::NotFound)?;
        let digest = accepted_agent_binding_digest(&raw).map_err(AgentServiceError::observation)?;
        scope
            .agent_bindings
            .get(&(agent_id.to_owned(), digest))
            .map(AsRef::as_ref)
            .ok_or(AgentServiceError::NotFound)
    }

    fn service(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
    ) -> Result<
        (
            &InstalledScope,
            &PreparedAgentService,
            &AgentProviderRuntime,
        ),
        AgentServiceError,
    > {
        let scope = self
            .scopes
            .get(&(namespace.into(), tenant.into()))
            .ok_or(AgentServiceError::NotFound)?;
        Ok((
            scope,
            scope
                .prepared
                .agents
                .get(id)
                .ok_or(AgentServiceError::NotFound)?,
            scope
                .agents
                .get(id)
                .ok_or(AgentServiceError::Unavailable)?
                .as_ref(),
        ))
    }
    fn service_deadline(
        &self,
        scope: &InstalledScope,
        grant: &AgentServiceGrantDeclaration,
    ) -> Result<i64, String> {
        let declaration = scope.prepared.declaration();
        self.clock
            .now()
            .timestamp_millis()
            .checked_add(
                i64::try_from(declaration.root_lifetime_ms)
                    .map_err(|_| "invalid service lifetime")?,
            )
            .map(|deadline| {
                deadline
                    .min(declaration.credential_limits.deadline_ms)
                    .min(grant.limits.deadline_ms)
            })
            .ok_or_else(|| "service deadline overflow".into())
    }
    fn service_limits(
        &self,
        scope: &InstalledScope,
        grant: &AgentServiceGrantDeclaration,
    ) -> Result<RootBudgetLimits, String> {
        let declaration = scope.prepared.declaration();
        Ok(RootBudgetLimits {
            max_units: declaration.root_max_units.min(grant.limits.max_units),
            max_concurrent: declaration
                .root_max_concurrent
                .min(grant.limits.max_concurrent),
            deadline_ms: self.service_deadline(scope, grant)?,
        })
    }
}
fn service_key(kind: &str, fields: &[&str]) -> String {
    format!(
        "agent-service-{kind}/{:x}",
        Sha256::digest(serde_json::to_vec(fields).expect("string-only admission key"))
    )
}
fn grant_reference(grant: &AgentServiceGrantDeclaration) -> DelegationGrantReference {
    DelegationGrantReference {
        id: grant.id.clone(),
        accepted_revision: grant.revision,
    }
}
