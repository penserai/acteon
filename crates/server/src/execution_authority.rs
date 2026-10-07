//! Production preparation from validated declarations and actual registrations.
//! Preparation is read-only; publication is a later, explicitly ordered stage.
pub mod agent_services;
mod runtime;
pub use runtime::{
    AgentServiceAcceptance, AgentServiceDriver, AgentServiceError, AgentServiceObservation,
    AgentServiceParent, AgentServiceRequest, ExecutionAuthorityRuntime,
    ExecutionRuntimeDependencies, ManagementError, TrustedReconciliationInstallation,
};
use std::{collections::BTreeMap, sync::Arc};

use acteon_core::Action;
use acteon_executor::{
    ProviderExecutionAdmission, ProviderExecutionAuthority, ProviderInvocation,
    ProviderInvocationOrigin, catalog::QualifiedProviderCatalog,
    governed::governed_provider_input_digest,
};
use acteon_governance::context::{
    ContextBinding, ExecutionContextHandle, RootContextAdmission, TrustedContextStore,
    VerifiedExecutionContext,
};
use acteon_governance::permit::PermitReference;
use acteon_governance::{AuthorityCoordinator, permit::PermitIssuanceCeiling};
use acteon_provider::DynProvider;
use sha2::{Digest, Sha256};

use crate::{
    auth::projection::{CredentialPolicyProjector, ScopedCredentialBinding},
    config::{ExecutionAuthorityConfig, ExecutionScopeConfig},
    provider_factory::StaticWebhook,
};

// Resolve intervention footprints before publishing any authority. A valid
// route count can still exceed the bounded control ceiling.
fn validate_management_footprints(
    declaration: &ExecutionScopeConfig,
    catalog: &QualifiedProviderCatalog,
) -> Result<(), String> {
    let definitions = catalog.definitions(&declaration.namespace, &declaration.tenant);
    for manager in declaration.managers.iter().filter(|m| m.can_intervene) {
        let resources = definitions
            .iter()
            .filter(|d| {
                manager
                    .routes
                    .iter()
                    .any(|r| r.provider == d.provider && r.action_type == d.action_type)
            })
            .flat_map(|d| d.effect.resources.iter().cloned())
            .chain(manager.agents.iter().map(|id| {
                acteon_core::ResourceRef::new(
                    acteon_core::ResourceKind::Agent,
                    &declaration.namespace,
                    &declaration.tenant,
                    id,
                )
                .expect("validated manager agent")
            }))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        acteon_governance::control::ControlChangeCeiling {
            actor: manager.principal.clone(),
            subjects: manager.subjects.clone(),
            resources,
            valid_from_ms: manager.valid_from_ms,
            deadline_ms: manager.limits.deadline_ms,
        }
        .validate()
        .map_err(|_| "execution manager exceeds control footprint capacity")?;
    }
    Ok(())
}

struct Registration {
    actual: Arc<dyn DynProvider>,
    webhook: Option<StaticWebhook>,
}

/// Host-owned registrations. Request labels cannot substitute a provider
/// instance or manufacture qualification settings.
#[derive(Default)]
pub struct ExecutionProviderRegistry {
    entries: BTreeMap<String, Registration>,
}

impl ExecutionProviderRegistry {
    pub fn register(
        &mut self,
        actual: Arc<dyn DynProvider>,
        webhook: Option<StaticWebhook>,
    ) -> Result<(), String> {
        if self.entries.contains_key(actual.name()) {
            return Err("provider registration names must be unique".into());
        }
        if webhook
            .as_ref()
            .is_some_and(|w| !Arc::ptr_eq(&actual, &w.provider()))
        {
            return Err("webhook qualification differs from actual registered instance".into());
        }
        self.entries
            .insert(actual.name().into(), Registration { actual, webhook });
        Ok(())
    }

    /// Resolve and validate all scopes before any coordinator is initialized or
    /// any source epoch is published. Unsupported adapters remain unqualified.
    // Keep the ordered qualification and authority checks visible together.
    #[allow(clippy::too_many_lines)]
    pub fn prepare(
        &self,
        configuration: &ExecutionAuthorityConfig,
        control_scope: (&str, &str),
        key: &[u8],
    ) -> Result<Vec<PreparedExecutionScope>, String> {
        configuration.validate(control_scope)?;
        if key.len() < 32 {
            return Err(
                "execution qualification requires a deployment key of at least 32 bytes".into(),
            );
        }
        configuration
            .scopes
            .iter()
            .map(|scope| {
                let mut declaration = scope.clone();
                canonicalize_declaration(&mut declaration);
                let mut bindings = Vec::new();
                for route in &declaration.routes {
                    let registration = self
                        .entries
                        .get(&route.provider)
                        .ok_or("execution route has no actual registered provider")?;
                    let factory = registration
                        .webhook
                        .as_ref()
                        .ok_or("execution route uses an unqualified adapter")?;
                    if !Arc::ptr_eq(&registration.actual, &factory.provider()) {
                        return Err("execution provider registration lost qualification".into());
                    }
                    bindings.push(factory.binding(
                        &declaration.namespace,
                        &declaration.tenant,
                        &route.action_type,
                        key,
                    )?);
                }
                let catalog = if declaration.retained_only() {
                    QualifiedProviderCatalog::for_history()
                } else {
                    QualifiedProviderCatalog::new_trusted(bindings)
                        .map_err(|_| "invalid execution scope catalog")?
                };
                let mut agents = BTreeMap::new();
                for service in &declaration.agent_services {
                    let actual = &self
                        .entries
                        .get(&service.route.provider)
                        .ok_or("agent provider unavailable")?
                        .actual;
                    let action = Action::new(
                        declaration.namespace.as_str(),
                        declaration.tenant.as_str(),
                        service.route.provider.as_str(),
                        &service.route.action_type,
                        serde_json::Value::Null,
                    );
                    let bound = catalog
                        .resolve(&action, actual)
                        .map_err(|_| "agent operation is not qualified")?
                        .clone();
                    let binding = service.qualify(&bound)?;
                    agents.insert(
                        service.card.agent_id.clone(),
                        agent_services::PreparedAgentService {
                            declaration: service.clone(),
                            binding,
                            bound,
                        },
                    );
                }
                let mut effects: Vec<_> = catalog
                    .definitions(&declaration.namespace, &declaration.tenant)
                    .into_iter()
                    .map(|d| d.effect)
                    .collect();
                for chain in &declaration.chains {
                    effects.push(chain.effect(&declaration.namespace, &declaration.tenant)?);
                }
                effects.extend(declaration.historical_effects.clone());
                effects.extend(
                    agents
                        .values()
                        .map(|agent| agent.binding.ingress_effect().clone()),
                );
                effects.sort_by(|a, b| {
                    a.operation
                        .cmp(&b.operation)
                        .then(a.resources.cmp(&b.resources))
                });
                effects.dedup();
                let issuance = PermitIssuanceCeiling {
                    issuer: declaration.publisher.clone(),
                    subjects: declaration.subjects.clone(),
                    effects,
                    valid_from_ms: declaration.valid_from_ms,
                    limits: declaration.credential_limits.clone(),
                };
                issuance
                    .validate()
                    .map_err(|_| "invalid independently declared execution ceiling")?;
                validate_management_footprints(&declaration, &catalog)?;
                // Bootstrap is an operational initialization choice, not policy.
                let mut policy = serde_json::json!({
                    "format": "acteon.execution_scope.policy.v1",
                    "namespace": declaration.namespace, "tenant": declaration.tenant,
                    "issuance": {"issuer": issuance.issuer, "subjects": issuance.subjects,
                        "effects": issuance.effects, "valid_from_ms": issuance.valid_from_ms,
                        "limits": issuance.limits},
                    "root_max_units": declaration.root_max_units,
                    "root_max_concurrent": declaration.root_max_concurrent,
                    "root_lifetime_ms": declaration.root_lifetime_ms,
                });
                if declaration.history_only {
                    policy["history_only"] = serde_json::json!(true);
                }
                if declaration.reconciliation_only {
                    policy["reconciliation_only"] = serde_json::json!(true);
                }
                if !declaration.agent_services.is_empty() {
                    policy["agent_services"] = serde_json::json!(declaration.agent_services);
                }
                if !declaration.chains.is_empty() {
                    policy["chains"] = serde_json::json!(declaration.chains);
                }
                if !declaration.managers.is_empty() {
                    policy["managers"] = serde_json::json!(declaration.managers);
                }
                let bytes = serde_json::to_vec(&policy)
                    .map_err(|_| "invalid execution deployment policy")?;
                Ok(PreparedExecutionScope {
                    declaration,
                    catalog,
                    issuance,
                    agents,
                    policy_fingerprint: format!("{:x}", Sha256::digest(bytes)),
                })
            })
            .collect()
    }
}

/// Trusted host allocation for one root. The durable admission key pins the
/// first candidate handle and execution ID across retries. Never derive new
/// authority from a public context reference.
/// This type deliberately has no deserializer.
pub struct RootExecutionRequest<'a> {
    pub admission_key: &'a str,
    pub handle: ExecutionContextHandle,
    pub execution_id: uuid::Uuid,
    pub action: &'a Action,
    pub selected: &'a Arc<dyn DynProvider>,
    pub authentication: &'a ScopedCredentialBinding,
    pub permits: &'a [PermitReference],
}

/// Private authentication and actual original work for a complete chain plan.
/// Each enclosed chain and provider effect requires explicit current authority.
pub struct RootPlanRequest<'a> {
    pub admission_key: &'a str,
    pub handle: ExecutionContextHandle,
    pub execution_id: uuid::Uuid,
    pub action: &'a Action,
    pub entry: &'a str,
    pub definitions: &'a BTreeMap<String, acteon_core::ChainConfig>,
    pub authentication: &'a ScopedCredentialBinding,
    pub permits: &'a [PermitReference],
}
/// Host-only accepted plan and its verified root, ready for durable handoff.
pub struct CapturedChainPlan {
    pub invocation: acteon_executor::plan::QualifiedChainInvocation,
    pub root: VerifiedExecutionContext,
}
struct QualifiedRootInput {
    digest: String,
    effects: Vec<acteon_governance::context::AcceptedEffect>,
    job_class: String,
}

/// Stable trusted request identity retained while rules select the actual work.
/// Authentication is the original private middleware binding, never metadata.
pub struct ProviderAdmissionRequest<'a> {
    pub admission_key: &'a str,
    pub handle: ExecutionContextHandle,
    pub execution_id: uuid::Uuid,
    pub authentication: &'a ScopedCredentialBinding,
    pub permits: &'a [PermitReference],
}

pub struct AuthenticatedProviderAdmission<'a> {
    scope: &'a PreparedExecutionScope,
    request: ProviderAdmissionRequest<'a>,
    coordinator: &'a AuthorityCoordinator,
    contexts: &'a TrustedContextStore,
    clock: &'a dyn acteon_time::Clock,
    handoffs: Option<&'a acteon_executor::plan::handoff::PlanHandoffStore>,
}

#[async_trait::async_trait]
impl ProviderExecutionAdmission for AuthenticatedProviderAdmission<'_> {
    async fn admit(
        &self,
        invocation: ProviderInvocation<'_>,
    ) -> Result<ProviderExecutionAuthority, acteon_core::ActionError> {
        let refused = || acteon_core::ActionError {
            code: "EXECUTION_ADMISSION_REFUSED".into(),
            message: "Selected work has no current execution authority".into(),
            retryable: false,
            attempts: 0,
        };
        if invocation.context.is_some()
            || !matches!(
                invocation.origin,
                ProviderInvocationOrigin::Dispatch
                    | ProviderInvocationOrigin::Reroute
                    | ProviderInvocationOrigin::Fallback
            )
        {
            return Err(refused());
        }
        self.scope
            .capture_provider_authority(
                RootExecutionRequest {
                    admission_key: self.request.admission_key,
                    handle: self.request.handle.clone(),
                    execution_id: self.request.execution_id,
                    action: invocation.action,
                    selected: invocation.selected,
                    authentication: self.request.authentication,
                    permits: self.request.permits,
                },
                self.coordinator,
                self.contexts,
                self.clock,
            )
            .await
            .map_err(|_| refused())
    }
    async fn admit_chain(
        &self,
        job_id: uuid::Uuid,
        action: &Action,
        entry: &str,
        definitions: &BTreeMap<String, acteon_core::ChainConfig>,
    ) -> Result<(), acteon_core::ActionError> {
        let denied = || acteon_core::ActionError {
            code: "CHAIN_PLAN_ADMISSION_REFUSED".into(),
            message: "Complete chain plan could not be admitted".into(),
            retryable: false,
            attempts: 0,
        };
        let handoffs = self.handoffs.ok_or_else(denied)?;
        // The connected engine currently supports provider, timer, signal and
        // flat provider-parallel steps. Other adapters remain a delivery gate.
        let plan = self
            .scope
            .qualify_chain_plan(entry, definitions)
            .map_err(|_| denied())?;
        for definition in plan.definitions().values() {
            if definition.on_cancel.is_some() {
                return Err(denied());
            }
            for step in &definition.steps {
                match step.kind() {
                    acteon_core::StepKind::Provider
                    | acteon_core::StepKind::Timer(_)
                    | acteon_core::StepKind::Signal(_) => {}
                    acteon_core::StepKind::Parallel(group)
                        if group.steps.iter().all(|child| {
                            matches!(child.kind(), acteon_core::StepKind::Provider)
                        }) => {}
                    _ => return Err(denied()),
                }
            }
        }
        self.scope
            .admit_pinned_chain_job(
                job_id,
                RootPlanRequest {
                    admission_key: self.request.admission_key,
                    handle: self.request.handle.clone(),
                    execution_id: self.request.execution_id,
                    action,
                    entry,
                    definitions,
                    authentication: self.request.authentication,
                    permits: self.request.permits,
                },
                self.coordinator,
                self.contexts,
                handoffs,
                self.clock,
            )
            .await
            .map_err(|_| denied())?;
        Ok(())
    }
}

impl<'a> AuthenticatedProviderAdmission<'a> {
    pub(crate) fn with_chain_handoffs(
        mut self,
        handoffs: &'a acteon_executor::plan::handoff::PlanHandoffStore,
    ) -> Self {
        self.handoffs = Some(handoffs);
        self
    }
}

/// Private construction keeps preparation evidence separate from wire metadata.
pub struct PreparedExecutionScope {
    declaration: ExecutionScopeConfig,
    catalog: QualifiedProviderCatalog,
    agents: BTreeMap<String, agent_services::PreparedAgentService>,
    issuance: PermitIssuanceCeiling,
    policy_fingerprint: String,
}

impl PreparedExecutionScope {
    /// Qualify complete pinned plans against this scope's actual registrations.
    /// This is preparation metadata; authentication and current permits are
    /// still required for root admission and every subsequent effect.
    pub fn qualify_chain_plan(
        &self,
        entry: &str,
        definitions: &BTreeMap<String, acteon_core::ChainConfig>,
    ) -> Result<acteon_executor::plan::QualifiedChainPlan, acteon_executor::plan::PlanError> {
        acteon_executor::plan::QualifiedChainPlan::new_trusted(
            &self.declaration.namespace,
            &self.declaration.tenant,
            entry,
            definitions,
            self.catalog.clone(),
        )
    }
    /// Bind one authenticated request to final-work admission. Borrowing the
    /// proof explicitly prevents automatic inheritance by detached/child work.
    pub fn provider_admission<'a>(
        &'a self,
        request: ProviderAdmissionRequest<'a>,
        coordinator: &'a AuthorityCoordinator,
        contexts: &'a TrustedContextStore,
        clock: &'a dyn acteon_time::Clock,
    ) -> AuthenticatedProviderAdmission<'a> {
        AuthenticatedProviderAdmission {
            scope: self,
            request,
            coordinator,
            contexts,
            clock,
            handoffs: None,
        }
    }
    /// Produce invocation authority from the original private authentication
    /// proof and durable admission, never from action labels or wire references.
    pub async fn capture_provider_authority(
        &self,
        request: RootExecutionRequest<'_>,
        coordinator: &AuthorityCoordinator,
        contexts: &TrustedContextStore,
        clock: &dyn acteon_time::Clock,
    ) -> Result<acteon_executor::ProviderExecutionAuthority, String> {
        let actor = request
            .authentication
            .authentication_source()
            .principal()
            .clone();
        let permits = request.permits.to_vec();
        let verified = self
            .capture_root(request, coordinator, contexts, clock)
            .await?;
        Ok(acteon_executor::ProviderExecutionAuthority::new_trusted(
            verified
                .reference()
                .map_err(|_| "admitted context reference unavailable")?,
            actor,
            permits,
        ))
    }

    /// Verify that the original authentication publication describes this exact
    /// prepared runtime, then observe current eligibility from the target scope.
    /// Admission must use this stamp at its CAS; this observation is not a permit.
    pub async fn verify_authenticated_scope(
        &self,
        binding: &ScopedCredentialBinding,
        coordinator: &AuthorityCoordinator,
        now_ms: i64,
    ) -> Result<acteon_governance::AuthorityStamp, String> {
        if !binding.matches_deployment_policy(&self.policy_fingerprint) {
            return Err("authenticated scope differs from prepared execution policy".into());
        }
        binding.verify_execution_scope(coordinator, now_ms).await
    }

    pub(crate) async fn verify_authenticated_scope_typed(
        &self,
        binding: &ScopedCredentialBinding,
        coordinator: &AuthorityCoordinator,
        now_ms: i64,
    ) -> Result<acteon_governance::AuthorityStamp, acteon_governance::CoordinationError> {
        if !binding.matches_deployment_policy(&self.policy_fingerprint) {
            return Err(acteon_governance::CoordinationError::Restricted);
        }
        binding
            .verify_execution_scope_typed(coordinator, now_ms)
            .await
    }

    /// Admit actual selected work with original authentication and explicit
    /// current permits. No permit is inferred or minted from credential grants.
    pub async fn capture_root(
        &self,
        request: RootExecutionRequest<'_>,
        coordinator: &AuthorityCoordinator,
        contexts: &TrustedContextStore,
        clock: &dyn acteon_time::Clock,
    ) -> Result<VerifiedExecutionContext, String> {
        self.catalog
            .resolve(request.action, request.selected)
            .map_err(|_| "root selected provider is not qualified")?;
        let definition = self
            .catalog
            .definitions(&self.declaration.namespace, &self.declaration.tenant)
            .into_iter()
            .find(|d| {
                d.provider == request.selected.name() && d.action_type == request.action.action_type
            })
            .ok_or("root effect is not qualified")?;
        let request_digest =
            governed_provider_input_digest(request.action).map_err(|_| "invalid root input")?;
        self.capture_qualified_root(
            ProviderAdmissionRequest {
                admission_key: request.admission_key,
                handle: request.handle,
                execution_id: request.execution_id,
                authentication: request.authentication,
                permits: request.permits,
            },
            QualifiedRootInput {
                digest: request_digest,
                effects: vec![definition.effect],
                job_class: definition.action_type,
            },
            coordinator,
            contexts,
            clock,
        )
        .await
    }

    pub(crate) async fn admit_pinned_chain_job(
        &self,
        job_id: uuid::Uuid,
        request: RootPlanRequest<'_>,
        coordinator: &AuthorityCoordinator,
        contexts: &TrustedContextStore,
        handoffs: &acteon_executor::plan::handoff::PlanHandoffStore,
        clock: &dyn acteon_time::Clock,
    ) -> Result<CapturedChainPlan, String> {
        if job_id.is_nil() {
            return Err("invalid host chain job identity".into());
        }
        let admission_key = format!("acteon.server.chain-root.v1/{job_id}");
        let origin = request.action;
        let permits = request.permits;
        let captured = self
            .capture_plan_root(
                RootPlanRequest {
                    admission_key: &admission_key,
                    ..request
                },
                coordinator,
                contexts,
                clock,
            )
            .await?;
        handoffs
            .persist(
                job_id,
                &captured.invocation,
                origin,
                &captured.root,
                permits,
            )
            .await
            .map_err(|_| "accepted plan handoff unavailable or conflicting")?;
        Ok(captured)
    }

    /// Admit the complete actual plan using independently declared chain bounds.
    /// Sub-chains do not acquire authority merely by being referenced by a plan.
    pub async fn capture_plan_root(
        &self,
        request: RootPlanRequest<'_>,
        coordinator: &AuthorityCoordinator,
        contexts: &TrustedContextStore,
        clock: &dyn acteon_time::Clock,
    ) -> Result<CapturedChainPlan, String> {
        let plan = Arc::new(
            self.qualify_chain_plan(request.entry, request.definitions)
                .map_err(|_| "chain plan is not fully qualified")?,
        );
        let principal = request.authentication.authentication_source().principal();
        for name in plan.definitions().keys() {
            if !self
                .declaration
                .chains
                .iter()
                .any(|chain| chain.name == *name && chain.subjects.contains(principal))
            {
                return Err("chain plan exceeds independently declared actor bounds".into());
            }
        }
        let invocation = plan
            .bind_input(request.action)
            .map_err(|_| "invalid original plan input")?;
        let root = self
            .capture_qualified_root(
                ProviderAdmissionRequest {
                    admission_key: request.admission_key,
                    handle: request.handle,
                    execution_id: request.execution_id,
                    authentication: request.authentication,
                    permits: request.permits,
                },
                QualifiedRootInput {
                    digest: invocation.request_digest().into(),
                    effects: plan.required_effects().to_vec(),
                    job_class: request.entry.into(),
                },
                coordinator,
                contexts,
                clock,
            )
            .await?;
        invocation
            .verify_root(&root)
            .map_err(|_| "captured root differs from qualified plan")?;
        Ok(CapturedChainPlan { invocation, root })
    }

    async fn capture_qualified_root(
        &self,
        request: ProviderAdmissionRequest<'_>,
        input: QualifiedRootInput,
        coordinator: &AuthorityCoordinator,
        contexts: &TrustedContextStore,
        clock: &dyn acteon_time::Clock,
    ) -> Result<VerifiedExecutionContext, String> {
        let now_ms = clock.now().timestamp_millis();
        let stamp = self
            .verify_authenticated_scope(request.authentication, coordinator, now_ms)
            .await?;
        let lifetime = i64::try_from(self.declaration.root_lifetime_ms)
            .map_err(|_| "invalid root lifetime")?;
        let deadline_ms = now_ms
            .checked_add(lifetime)
            .ok_or("root deadline overflow")?
            .min(self.declaration.credential_limits.deadline_ms);
        let source = request.authentication.authentication_source();
        let mut admission = RootContextAdmission {
            handle: request.handle,
            binding: ContextBinding {
                execution_id: request.execution_id,
                principal: source.principal().clone(),
                request_digest: input.digest,
            },
            credential_id: request.authentication.credential_reference().id.clone(),
            auth_method: source.auth_method().into(),
            accepted_ceiling_revision: String::new(), // Derived from exact permits by the store.
            accepted_effects: input.effects,
            deadline_ms,
            evaluated_authority: stamp,
        };
        let limits = coordinator
            .attenuate_permitted_root_limits(
                &admission,
                request.permits,
                acteon_governance::RootBudgetLimits {
                    max_units: self.declaration.root_max_units,
                    max_concurrent: self.declaration.root_max_concurrent,
                    deadline_ms,
                },
                now_ms,
            )
            .await
            .map_err(|e| format!("root limits denied: {e}"))?;
        admission.deadline_ms = limits.deadline_ms;
        // The initiator is the actual private authentication principal. A public
        // dispatch cannot claim another human's identity or delegated authority.
        // Job class is derived by the qualified host path, never request labels.
        let representation = coordinator
            .evaluate_permit_representation(
                acteon_governance::workforce::WorkforcePermitAdmission {
                    permits: request.permits,
                    initiator: source.principal(),
                    job_class: &input.job_class,
                    admission: &admission,
                    limits: &limits,
                    clock,
                },
            )
            .await
            .map_err(|e| format!("workforce admission denied: {e}"))?;
        let captured = if let Some(proof) = representation {
            contexts
                .capture_idempotent_represented_credentialed_root(
                    acteon_governance::context::IdempotentRootAdmission {
                        admission_key: request.admission_key,
                        admission,
                        permits: request.permits,
                        credential: request.authentication.credential_reference().clone(),
                        limits,
                        clock,
                    },
                    &proof,
                )
                .await
        } else {
            contexts
                .capture_idempotent_credentialed_root(
                    request.admission_key,
                    admission,
                    request.permits,
                    request.authentication.credential_reference().clone(),
                    limits,
                    clock,
                )
                .await
        };
        captured.map_err(|e| format!("root admission denied: {e}"))
    }

    #[must_use]
    pub fn declaration(&self) -> &ExecutionScopeConfig {
        &self.declaration
    }
    #[must_use]
    pub fn catalog(&self) -> &QualifiedProviderCatalog {
        &self.catalog
    }
    #[must_use]
    pub fn policy_fingerprint(&self) -> &str {
        &self.policy_fingerprint
    }

    /// Construct the scope publisher against the actual configured coordinator.
    /// Initialization and complete multi-scope publication remain host duties.
    pub async fn projector(
        &self,
        coordinator: AuthorityCoordinator,
    ) -> Result<CredentialPolicyProjector, String> {
        let snapshot = coordinator
            .snapshot()
            .await
            .map_err(|_| "scope authority unavailable")?;
        if snapshot.namespace != self.declaration.namespace
            || snapshot.tenant != self.declaration.tenant
        {
            return Err("prepared execution scope differs from coordinator".into());
        }
        coordinator
            .reserve_scope(acteon_governance::ScopePurpose::Execution)
            .await
            .map_err(|_| "execution scope is already owned or contains unclaimed work")?;
        let issuance = PermitIssuanceCeiling {
            issuer: self.issuance.issuer.clone(),
            subjects: self.issuance.subjects.clone(),
            effects: self.issuance.effects.clone(),
            valid_from_ms: self.issuance.valid_from_ms,
            limits: self.issuance.limits.clone(),
        };
        let projector = if self.declaration.retained_only() {
            CredentialPolicyProjector::for_history(
                coordinator,
                issuance,
                self.declaration.valid_from_ms,
                self.declaration.credential_limits.clone(),
            )
            .await?
        } else {
            CredentialPolicyProjector::new_trusted(
                coordinator,
                self.catalog.clone(),
                issuance,
                self.declaration.valid_from_ms,
                self.declaration.credential_limits.clone(),
            )
            .await?
        };
        projector
            .with_deployment_policy_fingerprint(self.policy_fingerprint.clone())
            .with_chain_admission_bounds(
                self.declaration
                    .chains
                    .iter()
                    .map(|chain| (chain.name.clone(), chain.subjects.clone()))
                    .collect(),
            )?
            .with_agent_admission_bounds(
                self.agents
                    .iter()
                    .map(|(id, agent)| {
                        (
                            id.clone(),
                            (
                                agent.binding.ingress_effect().clone(),
                                agent
                                    .declaration
                                    .grants
                                    .iter()
                                    .map(|grant| grant.source.clone())
                                    .collect(),
                            ),
                        )
                    })
                    .collect(),
            )
    }
}

fn canonicalize_manager(manager: &mut crate::config::ExecutionManagerConfig) {
    manager.agents.sort();
    manager.subjects.sort_by(|a, b| a.id().cmp(b.id()));
    manager.routes.sort();
    if let Some(workforce) = &mut manager.workforce {
        workforce.teams.sort();
        workforce.job_classes.sort();
    }
}

fn canonicalize_declaration(declaration: &mut ExecutionScopeConfig) {
    declaration.subjects.sort_by(|a, b| a.id().cmp(b.id()));
    declaration.routes.sort();
    declaration.chains.sort_by(|a, b| a.name.cmp(&b.name));
    for chain in &mut declaration.chains {
        chain.subjects.sort_by(|a, b| a.id().cmp(b.id()));
    }
    declaration
        .managers
        .sort_by(|a, b| a.principal.id().cmp(b.principal.id()));
    declaration
        .managers
        .iter_mut()
        .for_each(canonicalize_manager);
    declaration
        .agent_services
        .sort_by(|a, b| a.card.agent_id.cmp(&b.card.agent_id));
    for service in &mut declaration.agent_services {
        service.recipient_permits.sort_by(|a, b| a.id.cmp(&b.id));
        service.grants.sort_by(|a, b| a.id.cmp(&b.id));
        for grant in &mut service.grants {
            grant.source_permits.sort_by(|a, b| a.id.cmp(&b.id));
        }
    }
    declaration.permits.sort_by(|a, b| a.id.cmp(&b.id));
    for permit in &mut declaration.permits {
        permit.routes.sort();
        permit.chains.sort();
        permit.agents.sort();
    }
    for effect in &mut declaration.historical_effects {
        effect.resources.sort();
    }
    declaration.historical_effects.sort_by(|a, b| {
        a.operation
            .cmp(&b.operation)
            .then(a.resources.cmp(&b.resources))
    });
}
