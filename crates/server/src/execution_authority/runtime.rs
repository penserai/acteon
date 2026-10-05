//! Shared server installation against the configured backend, without wire proof.
mod management;
pub use management::ManagementError;
use std::{collections::BTreeMap, sync::Arc};

use acteon_core::{Action, ActionOutcome};
use acteon_executor::{
    ExecutorConfig, GovernedProviderMediator, ProviderExecutionAdmission,
    ProviderExecutionAuthority, ProviderExecutionMediator, ProviderInvocation,
    governed::GovernedProviderExecutor,
};
use acteon_governance::{
    AuthorityCoordinator, CoordinatorLimits,
    context::{ContextSigningKey, ExecutionContextHandle, TrustedContextStore},
    permit::{ExecutionPermit, PermitReference},
};
use acteon_state::StateStore;
use acteon_time::Clock;
use sha2::{Digest, Sha256};

use super::{
    AuthenticatedProviderAdmission, ExecutionProviderRegistry, PreparedExecutionScope,
    ProviderAdmissionRequest,
};
use crate::auth::projection::{AuthenticatedExecutionConfiguration, CredentialPolicyProjector};

pub struct ExecutionRuntimeDependencies {
    pub state: Arc<dyn StateStore>,
    pub executor: ExecutorConfig,
    pub clock: Arc<dyn Clock>,
    pub encryptor: Option<Arc<acteon_crypto::PayloadEncryptor>>,
    pub signing_key: zeroize::Zeroizing<Vec<u8>>,
}
struct InstalledScope {
    prepared: PreparedExecutionScope,
    coordinator: AuthorityCoordinator,
    contexts: Arc<TrustedContextStore>,
}
pub struct ExecutionAuthorityRuntime {
    state: Arc<dyn StateStore>,
    scopes: BTreeMap<(String, String), InstalledScope>,
    projectors: Vec<Arc<CredentialPolicyProjector>>,
    mediator: Arc<dyn ProviderExecutionMediator>,
    clock: Arc<dyn Clock>,
}
impl ExecutionAuthorityRuntime {
    /// Prepare must already have validated the entire deployment. Qualification,
    /// context keys and executor settings are checked before backend mutation.
    pub async fn install(
        registry: &ExecutionProviderRegistry,
        prepared: Vec<PreparedExecutionScope>,
        dependencies: ExecutionRuntimeDependencies,
    ) -> Result<Self, String> {
        Self::preflight(registry, &prepared, &dependencies)?;
        let mut scopes = BTreeMap::new();
        let mut projectors = Vec::new();
        let mut executors = Vec::new();
        for prepared in prepared {
            let declaration = prepared.declaration();
            let coordinator = if declaration.bootstrap {
                AuthorityCoordinator::initialize(
                    dependencies.state.clone(),
                    &declaration.namespace,
                    &declaration.tenant,
                    CoordinatorLimits::default(),
                )
                .await
            } else {
                AuthorityCoordinator::connect(
                    dependencies.state.clone(),
                    &declaration.namespace,
                    &declaration.tenant,
                )
                .await
            }
            .map_err(|_| "execution scope unavailable or requires reviewed cutover")?;
            projectors.push(Arc::new(prepared.projector(coordinator.clone()).await?));
            let contexts = Arc::new(
                TrustedContextStore::new(
                    dependencies.state.clone(),
                    coordinator.clone(),
                    "acteon.server.execution.v1".into(),
                    "deployment-v1".into(),
                    vec![
                        ContextSigningKey::new(
                            "deployment-v1".into(),
                            dependencies.signing_key.to_vec(),
                        )
                        .map_err(|_| "invalid execution signing key")?,
                    ],
                )
                .map_err(|_| "invalid execution context store")?,
            );
            for route in &declaration.routes {
                let actual = &registry.entries[&route.provider].actual;
                let probe = Action::new(
                    declaration.namespace.as_str(),
                    declaration.tenant.as_str(),
                    route.provider.as_str(),
                    &route.action_type,
                    serde_json::Value::Null,
                );
                let binding = prepared
                    .catalog()
                    .resolve(&probe, actual)
                    .map_err(|_| "unqualified runtime provider")?;
                // Each driver retains shared concurrency across requests.
                executors.push(
                    GovernedProviderExecutor::new(
                        dependencies.state.clone(),
                        coordinator.clone(),
                        contexts.clone(),
                        binding.clone(),
                        dependencies.executor.clone(),
                        dependencies.clock.clone(),
                        dependencies.encryptor.clone(),
                    )
                    .map_err(|_| "invalid execution driver")?,
                );
            }
            scopes.insert(
                (declaration.namespace.clone(), declaration.tenant.clone()),
                InstalledScope {
                    prepared,
                    coordinator,
                    contexts,
                },
            );
        }
        Ok(Self {
            state: dependencies.state,
            scopes,
            projectors,
            mediator: Arc::new(GovernedProviderMediator::new(executors)?),
            clock: dependencies.clock,
        })
    }
    fn preflight(
        registry: &ExecutionProviderRegistry,
        prepared: &[PreparedExecutionScope],
        dependencies: &ExecutionRuntimeDependencies,
    ) -> Result<(), String> {
        if prepared.is_empty()
            || prepared.len() > 128
            || dependencies.executor.max_retries >= 32
            || dependencies.executor.max_concurrent == 0
            || dependencies.executor.execution_timeout.is_zero()
            || prepared
                .iter()
                .map(|scope| scope.declaration().routes.len())
                .sum::<usize>()
                > 4096
        {
            return Err("invalid execution runtime".into());
        }
        ContextSigningKey::new("deployment-v1".into(), dependencies.signing_key.to_vec())
            .map_err(|_| "invalid execution signing key")?;
        let mut identities = std::collections::BTreeSet::new();
        for scope in prepared {
            let declaration = scope.declaration();
            if !identities.insert((&declaration.namespace, &declaration.tenant)) {
                return Err("duplicate execution runtime scope".into());
            }
            for route in &declaration.routes {
                let actual = &registry
                    .entries
                    .get(&route.provider)
                    .ok_or("missing actual provider")?
                    .actual;
                let probe = Action::new(
                    declaration.namespace.as_str(),
                    declaration.tenant.as_str(),
                    route.provider.as_str(),
                    &route.action_type,
                    serde_json::Value::Null,
                );
                scope
                    .catalog()
                    .resolve(&probe, actual)
                    .map_err(|_| "runtime registration differs from preparation")?;
            }
        }
        Ok(())
    }
    /// Publish independent permits only after authentication configuration and
    /// every scope projection have succeeded, before exposing the listener.
    pub async fn publish_deployment_permits(&self) -> Result<(), String> {
        for scope in self.scopes.values() {
            Self::publish_permits(&scope.prepared, &scope.coordinator, self.clock.as_ref()).await?;
        }
        Ok(())
    }

    async fn publish_permits(
        prepared: &PreparedExecutionScope,
        coordinator: &AuthorityCoordinator,
        clock: &dyn Clock,
    ) -> Result<(), String> {
        let declaration = prepared.declaration();
        for permit in &declaration.permits {
            let definitions = prepared
                .catalog()
                .definitions(&declaration.namespace, &declaration.tenant);
            let effects = permit
                .routes
                .iter()
                .map(|route| {
                    definitions
                        .iter()
                        .find(|definition| {
                            definition.provider == route.provider
                                && definition.action_type == route.action_type
                        })
                        .map(|definition| definition.effect.clone())
                        .ok_or("permit route is not qualified")
                })
                .collect::<Result<Vec<_>, _>>()?;
            let change = format!(
                "deployment-permit/{:x}",
                Sha256::digest(
                    serde_json::to_vec(&(&permit.id, permit.revision))
                        .map_err(|_| "invalid permit identity")?
                )
            );
            coordinator
                .publish_permit(
                    &change,
                    ExecutionPermit {
                        id: permit.id.clone(),
                        revision: permit.revision,
                        subject: permit.subject.clone(),
                        effects,
                        valid_from_ms: permit.valid_from_ms,
                        limits: permit.limits.clone(),
                    },
                    permit.revision - 1,
                    &prepared.issuance,
                    &coordinator
                        .snapshot()
                        .await
                        .map_err(|_| "execution scope unavailable")?
                        .stamp(),
                    "explicit deployment permit",
                    clock.now().timestamp_millis(),
                )
                .await
                .map_err(|_| "deployment permit publication refused")?;
        }
        Ok(())
    }
    #[must_use]
    pub fn mediator(&self) -> Arc<dyn ProviderExecutionMediator> {
        self.mediator.clone()
    }
    #[must_use]
    pub fn projectors(&self) -> Vec<Arc<CredentialPolicyProjector>> {
        self.projectors.clone()
    }

    pub async fn verify_request(
        &self,
        action: &Action,
        authentication: &AuthenticatedExecutionConfiguration,
    ) -> Result<(), String> {
        let scope = self
            .scopes
            .get(&(
                action.namespace.as_str().into(),
                action.tenant.as_str().into(),
            ))
            .ok_or("execution scope is not declared")?;
        let binding = authentication
            .scope(action.namespace.as_str(), action.tenant.as_str())
            .map_err(|_| "private scope authentication unavailable")?;
        scope
            .prepared
            .verify_authenticated_scope(
                &binding,
                &scope.coordinator,
                self.clock.now().timestamp_millis(),
            )
            .await?;
        Ok(())
    }

    /// A replay marker binds stable authenticated ownership and original input.
    /// The current permit gate remains mandatory even when a marker matches.
    pub fn request_marker(
        &self,
        action: &Action,
        authentication: &AuthenticatedExecutionConfiguration,
        permits: &[PermitReference],
    ) -> Result<String, String> {
        let binding = authentication
            .scope(action.namespace.as_str(), action.tenant.as_str())
            .map_err(|_| "scope authentication unavailable")?;
        let input = acteon_executor::governed::governed_provider_input_digest(action)
            .map_err(|_| "invalid original work")?;
        let permit_tag = acteon_governance::permit::permit_revision_tag(permits)
            .map_err(|_| "invalid permit references")?;
        let bytes = serde_json::to_vec(&(
            "acteon.server.execution.replay.v1",
            binding.authentication_source().principal(),
            &binding.credential_reference().id,
            input,
            permit_tag,
        ))
        .map_err(|_| "invalid replay binding")?;
        Ok(format!("governed:{:x}", Sha256::digest(bytes)))
    }

    pub async fn dispatch(
        &self,
        gateway: &acteon_gateway::Gateway,
        action: Action,
        caller: &acteon_core::Caller,
        authentication: &AuthenticatedExecutionConfiguration,
        permits: &[PermitReference],
        replay_ttl: Option<u64>,
    ) -> Result<ActionOutcome, acteon_gateway::GatewayError> {
        let scope = self
            .scopes
            .get(&(
                action.namespace.as_str().into(),
                action.tenant.as_str().into(),
            ))
            .ok_or_else(|| {
                acteon_gateway::GatewayError::Configuration(
                    "execution scope is not declared".into(),
                )
            })?;
        let binding = authentication
            .scope(action.namespace.as_str(), action.tenant.as_str())
            .map_err(|_| {
                acteon_gateway::GatewayError::Configuration(
                    "private scope authentication unavailable".into(),
                )
            })?;
        let key = action.id.to_string();
        let admission = scope.prepared.provider_admission(
            ProviderAdmissionRequest {
                admission_key: &key,
                handle: ExecutionContextHandle::new(),
                execution_id: uuid::Uuid::new_v4(),
                authentication: &binding,
                permits,
            },
            &scope.coordinator,
            &scope.contexts,
            self.clock.as_ref(),
        );
        let replay = replay_ttl
            .map(|ttl| {
                self.request_marker(&action, authentication, permits)
                    .map(|marker| {
                        (
                            acteon_state::StateKey::new(
                                action.namespace.clone(),
                                action.tenant.clone(),
                                acteon_state::KeyKind::Custom("action_replay".into()),
                                action.id.to_string(),
                            ),
                            marker,
                            std::time::Duration::from_secs(ttl),
                        )
                    })
            })
            .transpose()
            .map_err(|_| {
                acteon_gateway::GatewayError::Configuration(
                    "execution replay binding unavailable".into(),
                )
            })?;
        let admission = ReplayBoundAdmission {
            inner: admission,
            state: self.state.as_ref(),
            replay,
        };
        gateway
            .dispatch_with_execution_admission(action, Some(caller), &admission)
            .await
    }
}

/// Reserve replay ownership only after current permits and durable root admission
/// succeed. A refused request must never poison a later authorized retry.
struct ReplayBoundAdmission<'a> {
    inner: AuthenticatedProviderAdmission<'a>,
    state: &'a dyn StateStore,
    replay: Option<(acteon_state::StateKey, String, std::time::Duration)>,
}
#[async_trait::async_trait]
impl ProviderExecutionAdmission for ReplayBoundAdmission<'_> {
    async fn admit(
        &self,
        invocation: ProviderInvocation<'_>,
    ) -> Result<ProviderExecutionAuthority, acteon_core::ActionError> {
        let authority = self.inner.admit(invocation).await?;
        if let Some((key, marker, ttl)) = &self.replay {
            let refused = || acteon_core::ActionError {
                code: "EXECUTION_REPLAY_BINDING_REFUSED".into(),
                message: "Execution replay binding unavailable or conflicting".into(),
                retryable: false,
                attempts: 0,
            };
            let claimed = self
                .state
                .check_and_set(key, marker, Some(*ttl))
                .await
                .map_err(|_| refused())?;
            if !claimed
                && self.state.get(key).await.map_err(|_| refused())?.as_deref()
                    != Some(marker.as_str())
            {
                return Err(refused());
            }
        }
        Ok(authority)
    }
}
