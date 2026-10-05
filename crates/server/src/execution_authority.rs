//! Production preparation from validated declarations and actual registrations.
//! Preparation is read-only; publication is a later, explicitly ordered stage.
mod runtime;
pub use runtime::{ExecutionAuthorityRuntime, ExecutionRuntimeDependencies, ManagementError};
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
                declaration.subjects.sort_by(|a, b| a.id().cmp(b.id()));
                declaration.routes.sort();
                declaration
                    .managers
                    .sort_by(|a, b| a.principal.id().cmp(b.principal.id()));
                for manager in &mut declaration.managers {
                    manager.subjects.sort_by(|a, b| a.id().cmp(b.id()));
                    manager.routes.sort();
                }
                declaration.permits.sort_by(|a, b| a.id.cmp(&b.id));
                for permit in &mut declaration.permits {
                    permit.routes.sort();
                }
                for effect in &mut declaration.historical_effects {
                    effect.resources.sort();
                }
                declaration.historical_effects.sort_by(|a, b| {
                    a.operation
                        .cmp(&b.operation)
                        .then(a.resources.cmp(&b.resources))
                });
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
                let catalog = QualifiedProviderCatalog::new_trusted(bindings)
                    .map_err(|_| "invalid execution scope catalog")?;
                let mut effects: Vec<_> = catalog
                    .definitions(&declaration.namespace, &declaration.tenant)
                    .into_iter()
                    .map(|d| d.effect)
                    .collect();
                effects.extend(declaration.historical_effects.clone());
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
                if !declaration.managers.is_empty() {
                    policy["managers"] = serde_json::json!(declaration.managers);
                }
                let bytes = serde_json::to_vec(&policy)
                    .map_err(|_| "invalid execution deployment policy")?;
                Ok(PreparedExecutionScope {
                    declaration,
                    catalog,
                    issuance,
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
}

/// Private construction keeps preparation evidence separate from wire metadata.
pub struct PreparedExecutionScope {
    declaration: ExecutionScopeConfig,
    catalog: QualifiedProviderCatalog,
    issuance: PermitIssuanceCeiling,
    policy_fingerprint: String,
}

impl PreparedExecutionScope {
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
        contexts
            .capture_idempotent_credentialed_root(
                request.admission_key,
                RootContextAdmission {
                    handle: request.handle,
                    binding: ContextBinding {
                        execution_id: request.execution_id,
                        principal: source.principal().clone(),
                        request_digest,
                    },
                    credential_id: request.authentication.credential_reference().id.clone(),
                    auth_method: source.auth_method().into(),
                    accepted_ceiling_revision: String::new(), // Derived from exact permits by the store.
                    accepted_effects: vec![definition.effect],
                    deadline_ms,
                    evaluated_authority: stamp,
                },
                request.permits,
                request.authentication.credential_reference().clone(),
                acteon_governance::RootBudgetLimits {
                    max_units: self.declaration.root_max_units,
                    max_concurrent: self.declaration.root_max_concurrent,
                    deadline_ms,
                },
                clock,
            )
            .await
            .map_err(|e| format!("root admission denied: {e}"))
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
        Ok(CredentialPolicyProjector::new_trusted(
            coordinator,
            self.catalog.clone(),
            PermitIssuanceCeiling {
                issuer: self.issuance.issuer.clone(),
                subjects: self.issuance.subjects.clone(),
                effects: self.issuance.effects.clone(),
                valid_from_ms: self.issuance.valid_from_ms,
                limits: self.issuance.limits.clone(),
            },
            self.declaration.valid_from_ms,
            self.declaration.credential_limits.clone(),
        )
        .await?
        .with_deployment_policy_fingerprint(self.policy_fingerprint.clone()))
    }
}
