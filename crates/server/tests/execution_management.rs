//! Original middleware proof, current scope policy, and management CAS together.
use acteon_governance::{AuthorityCoordinator, CoordinatorLimits};
use acteon_server::{
    auth::{
        AuthProvider, authority::AuthAuthority, config::AuthFileConfig, crypto::SecretString,
        middleware::AuthLayer, projection::AuthenticatedExecutionConfiguration,
    },
    config::{AuthAuthorityConfig, ExecutionAuthorityConfig, ProviderConfig},
    execution_authority::{
        ExecutionAuthorityRuntime, ExecutionProviderRegistry, ExecutionRuntimeDependencies,
        ManagementError,
    },
    provider_factory::StaticWebhook,
};
use acteon_state::StateStore;
use acteon_state_memory::MemoryStateStore;
use axum::{Extension, Router, body::Body, http::Request, routing::get};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

fn config() -> ExecutionAuthorityConfig {
    serde_json::from_value(json!({"scopes":[{
        "namespace":"prod", "tenant":"acme", "bootstrap":true,
        "publisher":{"id":"publisher","kind":"system"},
        "subjects":[{"id":"operator","kind":"human"},{"id":"agent/maya","kind":"agent"}],
        "routes":[{"provider":"incident","action_type":"execute"}],
        "valid_from_ms":0, "credential_limits":{"max_units":5,"max_concurrent":2,"deadline_ms":4_102_444_800_000_i64},
        "root_max_units":5, "root_max_concurrent":1, "root_lifetime_ms":60000,
        "managers":[{"principal":{"id":"operator","kind":"human"},"subjects":[{"id":"agent/maya","kind":"agent"}],
            "routes":[{"provider":"incident","action_type":"execute"}],"valid_from_ms":0,
            "limits":{"max_units":5,"max_concurrent":2,"deadline_ms":4_102_444_800_000_i64},
            "can_issue_permits":true,"can_intervene":true}]
    }]})).unwrap()
}
fn auth() -> AuthFileConfig {
    toml::from_str(&format!(
        r#"
authority_revision = 1
[settings]
jwt_secret = {:?}
[[api_keys]]
name = "operator"
authority_id = "credential/operator"
principal = {{id="operator",kind="human"}}
key_hash = {:?}
role = "operator"
[[api_keys.grants]]
namespaces = ["prod"]
tenants = ["acme"]
providers = ["audit"]
actions = ["read"]
"#,
        "t".repeat(32),
        acteon_server::auth::api_key::hash_api_key("operator-secret")
    ))
    .unwrap()
}
fn registry() -> ExecutionProviderRegistry {
    registry_and_provider().0
}
fn registry_and_provider() -> (
    ExecutionProviderRegistry,
    Arc<dyn acteon_provider::DynProvider>,
) {
    let config: ProviderConfig =
        toml::from_str("name='incident'\ntype='webhook'\nurl='https://example.com/incident'\n")
            .unwrap();
    let factory = StaticWebhook::build(&config, None).unwrap();
    let mut registry = ExecutionProviderRegistry::default();
    let actual = factory.provider();
    registry.register(actual.clone(), Some(factory)).unwrap();
    (registry, actual)
}
async fn runtime(
    registry: &ExecutionProviderRegistry,
    config: &ExecutionAuthorityConfig,
    state: Arc<dyn StateStore>,
) -> ExecutionAuthorityRuntime {
    runtime_with_clock(
        registry,
        config,
        state,
        Arc::new(acteon_time::SystemClock::default()),
    )
    .await
}
async fn runtime_with_clock(
    registry: &ExecutionProviderRegistry,
    config: &ExecutionAuthorityConfig,
    state: Arc<dyn StateStore>,
    clock: Arc<dyn acteon_time::Clock>,
) -> ExecutionAuthorityRuntime {
    let scopes = registry
        .prepare(config, ("auth-control", "deployment"), &[8; 32])
        .unwrap();
    ExecutionAuthorityRuntime::install(
        registry,
        scopes,
        ExecutionRuntimeDependencies {
            state,
            executor: acteon_executor::ExecutorConfig::default(),
            clock,
            encryptor: None,
            signing_key: vec![9; 32].into(),
        },
    )
    .await
    .unwrap()
}
async fn authority(state: Arc<dyn StateStore>) -> Arc<AuthAuthority> {
    let coordinator = AuthorityCoordinator::initialize(
        state,
        "auth-control",
        "deployment",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap();
    Arc::new(
        AuthAuthority::new(
            coordinator,
            &AuthAuthorityConfig {
                namespace: "auth-control".into(),
                tenant: "deployment".into(),
                source_id: "auth".into(),
                bootstrap: true,
            },
            SecretString::new("shared-auth-key-at-least-32-bytes".into()),
        )
        .unwrap(),
    )
}
async fn proof(auth: Arc<AuthProvider>) -> AuthenticatedExecutionConfiguration {
    let saved = Arc::new(Mutex::new(None));
    let captured = saved.clone();
    let router = Router::new()
        .route(
            "/proof",
            get(
                move |Extension(proof): Extension<AuthenticatedExecutionConfiguration>| {
                    let captured = captured.clone();
                    async move {
                        *captured.lock().unwrap() = Some(proof);
                    }
                },
            ),
        )
        .layer(AuthLayer::new(Some(auth)));
    let response = router
        .oneshot(
            Request::builder()
                .uri("/proof")
                .header("authorization", "Bearer operator-secret")
                .header("x-acteon-principal", "forged-admin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    saved.lock().unwrap().take().unwrap()
}
fn revoke() -> acteon_core::GovernanceInterventionRequest {
    serde_json::from_value(
        json!({"namespace":"prod","tenant":"acme","change_id":"stale-request","reason":"reviewed",
        "change":{"kind":"revoke_subject","subject":{"id":"agent/maya","kind":"agent"}}}),
    )
    .unwrap()
}

#[tokio::test]
async fn captured_operator_proof_cannot_outlive_role_revocation() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let registry = registry();
    let runtime = runtime(&registry, &config(), state.clone()).await;
    let mut tables = auth();
    let provider = Arc::new(
        AuthProvider::new_with_scope_projection(
            &tables,
            state.clone(),
            authority(state.clone()).await,
            runtime.projectors(),
        )
        .await
        .unwrap(),
    );
    let original = proof(provider.clone()).await;
    runtime
        .inspect_governance("prod", "acme", &original)
        .await
        .unwrap();
    tables.authority_revision = Some(2);
    tables.api_keys[0].role = "viewer".into();
    provider.reload(&tables).await.unwrap();
    assert!(matches!(
        runtime.intervene_governance(revoke(), &original).await,
        Err(ManagementError::Forbidden)
    ));
    let current = proof(provider).await;
    assert!(matches!(
        runtime.intervene_governance(revoke(), &current).await,
        Err(ManagementError::Forbidden)
    ));
    let state = AuthorityCoordinator::connect(state, "prod", "acme")
        .await
        .unwrap()
        .snapshot()
        .await
        .unwrap();
    assert!(!state.changes.contains_key("stale-request"));
    assert!(state.revoked_subjects.is_empty());
}

#[tokio::test]
async fn old_replica_cannot_apply_newly_narrowed_management_policy() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let registry = registry();
    let original_config = config();
    let original_runtime = runtime(&registry, &original_config, state.clone()).await;
    let authority = authority(state.clone()).await;
    let mut tables = auth();
    let provider = Arc::new(
        AuthProvider::new_with_scope_projection(
            &tables,
            state.clone(),
            authority.clone(),
            original_runtime.projectors(),
        )
        .await
        .unwrap(),
    );
    let old_proof = proof(provider).await;
    let mut narrowed = original_config.clone();
    narrowed.scopes[0].bootstrap = false;
    narrowed.scopes[0].managers[0].can_intervene = false;
    let current_runtime = runtime(&registry, &narrowed, state.clone()).await;
    tables.authority_revision = Some(2);
    let provider = Arc::new(
        AuthProvider::new_with_scope_projection(
            &tables,
            state.clone(),
            authority,
            current_runtime.projectors(),
        )
        .await
        .unwrap(),
    );
    let current_proof = proof(provider).await;
    assert!(matches!(
        original_runtime
            .intervene_governance(revoke(), &old_proof)
            .await,
        Err(ManagementError::Forbidden)
    ));
    assert!(matches!(
        original_runtime
            .intervene_governance(revoke(), &current_proof)
            .await,
        Err(ManagementError::Forbidden)
    ));
    assert!(matches!(
        current_runtime
            .intervene_governance(revoke(), &current_proof)
            .await,
        Err(ManagementError::Forbidden)
    ));
    let state = AuthorityCoordinator::connect(state, "prod", "acme")
        .await
        .unwrap()
        .snapshot()
        .await
        .unwrap();
    assert!(!state.changes.contains_key("stale-request"));
    assert!(state.revoked_subjects.is_empty());
}

fn workforce_config() -> ExecutionAuthorityConfig {
    let mut declaration = config();
    declaration.scopes[0].managers[0].workforce =
        Some(acteon_server::config::WorkforceManagerConfig {
            teams: vec![acteon_core::TeamRef::new("prod", "acme", "reliability").unwrap()],
            job_classes: vec!["execute".into()],
            can_manage_roster: true,
            can_issue_mandates: true,
        });
    declaration
}
fn team_change(id: &str) -> acteon_core::workforce::WorkforceChangeRequest {
    serde_json::from_value(json!({"namespace":"prod","tenant":"acme","change_id":id,"reason":"reviewed",
        "change":{"kind":"put_team","team":{"team":{"domain":"prod","tenant":"acme","id":"reliability"},"revision":1,"name":"Reliability"}}})).unwrap()
}

#[tokio::test]
async fn ordinary_governance_management_does_not_grant_workforce_management() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let registry = registry();
    let runtime = runtime(&registry, &config(), state.clone()).await;
    let provider = Arc::new(
        AuthProvider::new_with_scope_projection(
            &auth(),
            state.clone(),
            authority(state.clone()).await,
            runtime.projectors(),
        )
        .await
        .unwrap(),
    );
    let original = proof(provider).await;
    runtime
        .inspect_governance("prod", "acme", &original)
        .await
        .unwrap();
    assert!(matches!(
        runtime.inspect_workforce("prod", "acme", &original).await,
        Err(ManagementError::Forbidden)
    ));
    assert!(matches!(
        runtime
            .change_workforce(team_change("not-authorized"), &original)
            .await,
        Err(ManagementError::Forbidden)
    ));
    assert!(
        AuthorityCoordinator::connect(state, "prod", "acme")
            .await
            .unwrap()
            .snapshot()
            .await
            .unwrap()
            .workforce
            .teams
            .is_empty()
    );
}

#[tokio::test]
async fn workforce_proof_does_not_survive_current_role_offboarding() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let registry = registry();
    let runtime = runtime(&registry, &workforce_config(), state.clone()).await;
    let mut tables = auth();
    let provider = Arc::new(
        AuthProvider::new_with_scope_projection(
            &tables,
            state.clone(),
            authority(state.clone()).await,
            runtime.projectors(),
        )
        .await
        .unwrap(),
    );
    let original = proof(provider.clone()).await;
    runtime
        .inspect_workforce("prod", "acme", &original)
        .await
        .unwrap();
    tables.authority_revision = Some(2);
    tables.api_keys[0].role = "viewer".into();
    provider.reload(&tables).await.unwrap();
    assert!(matches!(
        runtime
            .change_workforce(team_change("offboarded"), &original)
            .await,
        Err(ManagementError::Forbidden)
    ));
    assert!(
        AuthorityCoordinator::connect(state, "prod", "acme")
            .await
            .unwrap()
            .snapshot()
            .await
            .unwrap()
            .workforce
            .teams
            .is_empty()
    );
}

#[tokio::test]
async fn old_replica_cannot_manage_a_team_after_deployment_bounds_narrow() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let registry = registry();
    let original_config = workforce_config();
    let old = runtime(&registry, &original_config, state.clone()).await;
    let authority = authority(state.clone()).await;
    let mut tables = auth();
    let provider = Arc::new(
        AuthProvider::new_with_scope_projection(
            &tables,
            state.clone(),
            authority.clone(),
            old.projectors(),
        )
        .await
        .unwrap(),
    );
    let original = proof(provider).await;
    let mut narrowed = original_config.clone();
    narrowed.scopes[0].bootstrap = false;
    narrowed.scopes[0].managers[0]
        .workforce
        .as_mut()
        .unwrap()
        .teams = vec![acteon_core::TeamRef::new("prod", "acme", "release").unwrap()];
    let current = runtime(&registry, &narrowed, state.clone()).await;
    tables.authority_revision = Some(2);
    let provider = Arc::new(
        AuthProvider::new_with_scope_projection(
            &tables,
            state.clone(),
            authority,
            current.projectors(),
        )
        .await
        .unwrap(),
    );
    let now = proof(provider).await;
    for (runtime, authentication) in [(&old, &original), (&old, &now), (&current, &now)] {
        assert!(matches!(
            runtime
                .change_workforce(team_change("narrowed"), authentication)
                .await,
            Err(ManagementError::Forbidden)
        ));
    }
    assert_eq!(
        current
            .inspect_workforce("prod", "acme", &now)
            .await
            .unwrap()
            .management
            .teams[0]
            .id(),
        "release"
    );
    assert!(
        AuthorityCoordinator::connect(state, "prod", "acme")
            .await
            .unwrap()
            .snapshot()
            .await
            .unwrap()
            .workforce
            .teams
            .is_empty()
    );
}

#[tokio::test]
async fn history_read_requires_explicit_permission_and_current_private_authority() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let registry = registry();
    let denied_config = config();
    assert!(!denied_config.scopes[0].managers[0].can_read_history);
    // Omission preserves the original deployment-policy fingerprint.
    assert!(
        serde_json::to_value(&denied_config).unwrap()["scopes"][0]["managers"][0]
            .get("can_read_history")
            .is_none()
    );
    let denied = runtime(&registry, &denied_config, state.clone()).await;
    let authority = authority(state.clone()).await;
    let mut tables = auth();
    let provider =
        projected_auth_provider(&tables, state.clone(), authority.clone(), &denied).await;
    let original = proof(provider).await;
    let id = uuid::Uuid::new_v4();
    assert!(matches!(
        denied
            .inspect_provider_history("prod", "acme", id, &original)
            .await,
        Err(ManagementError::Forbidden)
    ));

    let mut allowed_config = config();
    allowed_config.scopes[0].bootstrap = false;
    let manager = &mut allowed_config.scopes[0].managers[0];
    manager.can_read_history = true;
    manager.can_issue_permits = false;
    manager.can_intervene = false;
    manager.routes.clear();
    let allowed = runtime(&registry, &allowed_config, state.clone()).await;
    tables.authority_revision = Some(2);
    let provider = projected_auth_provider(&tables, state.clone(), authority, &allowed).await;
    let read_proof = proof(provider.clone()).await;
    assert!(
        allowed
            .inspect_governance("prod", "acme", &read_proof)
            .await
            .unwrap()
            .management
            .can_read_history
    );
    let coordinator = AuthorityCoordinator::connect(state.clone(), "prod", "acme")
        .await
        .unwrap();
    let before = serde_json::to_value(coordinator.snapshot().await.unwrap()).unwrap();
    assert!(matches!(
        allowed
            .inspect_provider_history("prod", "acme", id, &read_proof)
            .await,
        Err(ManagementError::NotFound)
    ));
    assert!(matches!(
        allowed.intervene_governance(revoke(), &read_proof).await,
        Err(ManagementError::Forbidden)
    ));
    assert!(matches!(
        allowed
            .inspect_provider_history("prod", "another-tenant", id, &read_proof)
            .await,
        Err(ManagementError::Forbidden)
    ));
    assert!(matches!(
        allowed
            .inspect_provider_history("prod", "acme", id, &original)
            .await,
        Err(ManagementError::Forbidden)
    ));
    assert_eq!(
        before,
        serde_json::to_value(coordinator.snapshot().await.unwrap()).unwrap()
    );
    tables.authority_revision = Some(3);
    tables.api_keys[0].role = "viewer".into();
    provider.reload(&tables).await.unwrap();
    assert!(matches!(
        allowed
            .inspect_provider_history("prod", "acme", id, &read_proof)
            .await,
        Err(ManagementError::Forbidden)
    ));
    assert!(
        !AuthorityCoordinator::connect(state, "prod", "acme")
            .await
            .unwrap()
            .snapshot()
            .await
            .unwrap()
            .changes
            .contains_key("stale-request")
    );
}

fn history_router(
    state: Arc<dyn StateStore>,
    runtime: Arc<ExecutionAuthorityRuntime>,
    auth: Arc<AuthProvider>,
) -> Router {
    let gateway = acteon_gateway::GatewayBuilder::new()
        .state(state)
        .lock(Arc::new(acteon_state_memory::MemoryDistributedLock::new()))
        .build()
        .unwrap();
    let metrics = gateway.metrics_arc();
    acteon_server::api::router(acteon_server::api::AppState {
        gateway: Arc::new(tokio::sync::RwLock::new(gateway)),
        metrics,
        audit: None,
        analytics: None,
        auth: Some(auth),
        execution_authority: Some(runtime),
        rate_limiter: None,
        embedding: None,
        embedding_metrics: None,
        connection_registry: None,
        a2a_discovery_cache: Arc::new(
            acteon_server::api::a2a_discovery_cache::DiscoveryCache::new(),
        ),
        dispatch_semaphore: Arc::new(tokio::sync::Semaphore::new(1000)),
        config: acteon_server::config::ConfigSnapshot::default(),
        static_quotas: None,
        static_templates: None,
        ui_path: None,
        ui_enabled: false,
        cors_allowed_origins: Vec::new(),
        signature_verifier: None,
        replay_protection: None,
        #[cfg(feature = "swarm")]
        swarm_registry: None,
        #[cfg(feature = "bus")]
        bus_backend: None,
        #[cfg(feature = "bus")]
        bus_schema_validator: acteon_bus::SchemaValidator::new(),
        #[cfg(feature = "bus")]
        bus_sessions: Arc::new(acteon_server::bus_sessions::BusSessionRegistry::default()),
    })
}

// Keep the ordered deployment/retirement contract together for review.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn retained_history_remains_authenticated_after_every_live_provider_is_removed() {
    use acteon_core::{Action, ProviderHistoryStatus};
    use acteon_executor::governed::GovernedProviderExecutor;
    use acteon_governance::context::{
        ContextSigningKey, ExecutionContextHandle, TrustedContextStore,
    };
    use acteon_governance::permit::PermitReference;
    use acteon_state::testing::faults::{FaultStore, FaultTiming, WriteOperation};
    let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
    let state: Arc<dyn StateStore> = faults.clone();
    let (registry, actual) = registry_and_provider();
    let mut live_config = config();
    live_config.scopes[0].managers[0].can_read_history = true;
    let operator = live_config.scopes[0].managers[0].principal.clone();
    live_config.scopes[0].permits = vec![
        serde_json::from_value(json!({
            "id":"operator-incident", "revision":1, "subject":operator,
            "routes":[{"provider":"incident","action_type":"execute"}], "valid_from_ms":0,
            "limits":{"max_units":5,"max_concurrent":1,"deadline_ms":4_102_444_800_000_i64}
        }))
        .unwrap(),
    ];
    let prepared = registry
        .prepare(&live_config, ("auth-control", "deployment"), &[8; 32])
        .unwrap();
    let live = runtime(&registry, &live_config, state.clone()).await;
    let authority = authority(state.clone()).await;
    let mut tables = auth();
    tables.api_keys[0].grants[0].providers = vec!["incident".into()];
    tables.api_keys[0].grants[0].actions = vec!["execute".into()];
    let provider = Arc::new(
        AuthProvider::new_with_scope_projection(
            &tables,
            state.clone(),
            authority.clone(),
            live.projectors(),
        )
        .await
        .unwrap(),
    );
    live.publish_deployment_permits(None).await.unwrap();
    let live_proof = proof(provider).await;
    let binding = live_proof.scope("prod", "acme").unwrap();
    let coordinator = AuthorityCoordinator::connect(state.clone(), "prod", "acme")
        .await
        .unwrap();
    let contexts = Arc::new(
        TrustedContextStore::new(
            state.clone(),
            coordinator.clone(),
            "acteon.server.execution.v1".into(),
            "deployment-v1".into(),
            vec![ContextSigningKey::new("deployment-v1".into(), vec![9; 32]).unwrap()],
        )
        .unwrap(),
    );
    let clock = acteon_time::SystemClock::default();
    let action = Action::new(
        "prod",
        "acme",
        "incident",
        "execute",
        json!({"incident":42}),
    );
    let execution_id = uuid::Uuid::new_v4();
    let permits = vec![PermitReference {
        id: "operator-incident".into(),
        accepted_revision: 1,
    }];
    let context = prepared[0]
        .capture_root(
            acteon_server::execution_authority::RootExecutionRequest {
                admission_key: "retired-history",
                handle: ExecutionContextHandle::new(),
                execution_id,
                action: &action,
                selected: &actual,
                authentication: &binding,
                permits: &permits,
            },
            &coordinator,
            &contexts,
            &clock,
        )
        .await
        .unwrap();
    let reference = context.reference().unwrap();
    let driver = GovernedProviderExecutor::new(
        state.clone(),
        coordinator.clone(),
        contexts,
        prepared[0]
            .catalog()
            .resolve(&action, &actual)
            .unwrap()
            .clone(),
        acteon_executor::ExecutorConfig::default(),
        Arc::new(clock),
        None,
    )
    .unwrap()
    .require_credential_authority();
    // Persist the authentic operation but refuse admission before any HTTP send.
    faults
        .fail_next(
            acteon_state::KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    assert!(
        driver
            .execute(&reference, &permits, &action, &operator)
            .await
            .is_err()
    );
    assert_eq!(faults.consumed(), 1);
    assert!(matches!(
        live.inspect_provider_history("prod", "acme", execution_id, &live_proof)
            .await,
        Err(ManagementError::NotFound)
    )); // operator's work is outside this reader's allowlist
    let effects = prepared[0]
        .catalog()
        .definitions("prod", "acme")
        .into_iter()
        .map(|d| d.effect)
        .collect();
    drop(live);
    drop(prepared);
    drop(registry);

    let mut archived_config = live_config;
    let declaration = &mut archived_config.scopes[0];
    declaration.bootstrap = false;
    declaration.history_only = true;
    declaration.routes.clear();
    declaration.permits.clear();
    declaration.historical_effects = effects;
    let manager = &mut declaration.managers[0];
    manager.subjects = vec![operator.clone()];
    manager.routes.clear();
    manager.can_issue_permits = false;
    manager.can_intervene = false;
    let empty_registry = ExecutionProviderRegistry::default();
    let archived = Arc::new(runtime(&empty_registry, &archived_config, state.clone()).await);
    tables.authority_revision = Some(2);
    let provider = Arc::new(
        AuthProvider::new_with_scope_projection(
            &tables,
            state.clone(),
            authority,
            archived.projectors(),
        )
        .await
        .unwrap(),
    );
    let current_proof = proof(provider.clone()).await;
    let snapshot = coordinator.snapshot().await.unwrap();
    let credential = &snapshot.credentials["credential/operator"].authority;
    assert!(!credential.execution_enabled);
    assert_eq!(credential.ceiling.effects, Vec::new());
    let before = serde_json::to_value(snapshot).unwrap();
    assert!(matches!(
        archived
            .inspect_provider_history("prod", "acme", execution_id, &live_proof)
            .await,
        Err(ManagementError::Forbidden)
    ));
    let history = archived
        .inspect_provider_history("prod", "acme", execution_id, &current_proof)
        .await
        .unwrap();
    assert_eq!(history.subject, operator);
    assert_eq!(history.receipt.execution_id, execution_id.to_string());
    assert!(matches!(
        history.receipt.status,
        ProviderHistoryStatus::Prepared
    ));
    assert_eq!(history.receipt.attempts, 0);
    assert_eq!(
        coordinator.snapshot().await.unwrap().roots[&execution_id.to_string()].spent_units,
        0
    );
    let app = history_router(state.clone(), archived.clone(), provider);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v1/governance/executions/{execution_id}?namespace=prod&tenant=acme"
                ))
                .header("authorization", "Bearer operator-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let wire: acteon_core::ProviderExecutionHistory = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(wire.subject, operator);
    assert_eq!(wire.receipt.execution_id, execution_id.to_string());
    assert!(matches!(
        wire.receipt.status,
        ProviderHistoryStatus::Prepared
    ));
    // A worker retaining the old adapter must also observe current credential disablement.
    assert!(matches!(
        driver
            .execute(&reference, &permits, &action, &operator)
            .await,
        Err(acteon_executor::governed::GovernedProviderError::Admission(
            _
        ))
    ));
    let retained_authority =
        acteon_executor::ProviderExecutionAuthority::new_trusted(reference, operator, permits);
    let outcome = archived
        .mediator()
        .execute(acteon_executor::ProviderInvocation {
            action: &action,
            selected: &actual,
            context: None,
            origin: acteon_executor::ProviderInvocationOrigin::Dispatch,
            authority: Some(&retained_authority),
        })
        .await;
    assert!(
        matches!(outcome, acteon_core::ActionOutcome::Failed(error) if error.code == "PROVIDER_UNQUALIFIED")
    );
    assert_eq!(
        before,
        serde_json::to_value(coordinator.snapshot().await.unwrap()).unwrap()
    );
}

#[test]
fn history_only_declarations_reject_every_live_or_write_grant() {
    let archive = history_archive_config();
    archive.validate(("auth-control", "deployment")).unwrap();
    ExecutionProviderRegistry::default()
        .prepare(&archive, ("auth-control", "deployment"), &[8; 32])
        .unwrap();
    let baseline = serde_json::to_value(&archive).unwrap();
    for (pointer, replacement) in [
        ("/scopes/0/history_only", json!(false)),
        ("/scopes/0/bootstrap", json!(true)),
        (
            "/scopes/0/routes",
            json!([{"provider":"incident","action_type":"execute"}]),
        ),
        (
            "/scopes/0/chains",
            json!([{"name":"hidden-chain","subjects":[{"id":"agent/maya","kind":"agent"}]}]),
        ),
        ("/scopes/0/historical_effects", json!([])),
        ("/scopes/0/managers", json!([])),
        ("/scopes/0/managers/0/can_read_history", json!(false)),
        ("/scopes/0/managers/0/can_issue_permits", json!(true)),
        ("/scopes/0/managers/0/can_intervene", json!(true)),
    ] {
        let mut wire = baseline.clone();
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        wire.pointer_mut(parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(key.into(), replacement);
        let changed: ExecutionAuthorityConfig = serde_json::from_value(wire).unwrap();
        assert!(
            changed.validate(("auth-control", "deployment")).is_err(),
            "accepted {pointer}"
        );
    }
    let mut workforce = archive.clone();
    workforce.scopes[0].managers[0].workforce = Some(
        serde_json::from_value(json!({
            "teams":[], "job_classes":[], "can_manage_roster":true
        }))
        .unwrap(),
    );
    assert!(workforce.validate(("auth-control", "deployment")).is_err());
}

async fn projected_auth_provider(
    tables: &AuthFileConfig,
    state: Arc<dyn StateStore>,
    authority: Arc<AuthAuthority>,
    runtime: &ExecutionAuthorityRuntime,
) -> Arc<AuthProvider> {
    Arc::new(
        AuthProvider::new_with_scope_projection(tables, state, authority, runtime.projectors())
            .await
            .unwrap(),
    )
}

fn history_archive_config() -> ExecutionAuthorityConfig {
    let registry = registry();
    let mut archive = config();
    let prepared = registry
        .prepare(&archive, ("auth-control", "deployment"), &[8; 32])
        .unwrap();
    let scope = &mut archive.scopes[0];
    scope.historical_effects = prepared[0]
        .catalog()
        .definitions("prod", "acme")
        .into_iter()
        .map(|d| d.effect)
        .collect();
    scope.bootstrap = false;
    scope.history_only = true;
    scope.routes.clear();
    let manager = &mut scope.managers[0];
    manager.routes.clear();
    manager.can_issue_permits = false;
    manager.can_intervene = false;
    manager.can_read_history = true;
    archive
}

#[tokio::test]
async fn history_only_requires_existing_state_and_does_not_bootstrap_a_new_scope() {
    let registry = ExecutionProviderRegistry::default();
    let prepared = registry
        .prepare(
            &history_archive_config(),
            ("auth-control", "deployment"),
            &[8; 32],
        )
        .unwrap();
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let result = ExecutionAuthorityRuntime::install(
        &registry,
        prepared,
        ExecutionRuntimeDependencies {
            state: state.clone(),
            executor: acteon_executor::ExecutorConfig::default(),
            clock: Arc::new(acteon_time::SystemClock::default()),
            encryptor: None,
            signing_key: vec![9; 32].into(),
        },
    )
    .await;
    assert!(
        matches!(result, Err(error) if error == "execution scope unavailable or requires reviewed cutover")
    );
    assert_eq!(
        state
            .scan_keys_by_kind(acteon_state::KeyKind::Custom(
                acteon_governance::COORDINATOR_KIND.into()
            ))
            .await
            .unwrap(),
        Vec::new()
    );
}

struct HistoryReadFixture {
    faults: Arc<acteon_state::testing::faults::FaultStore>,
    state: Arc<dyn StateStore>,
    runtime: Arc<ExecutionAuthorityRuntime>,
    provider: Arc<AuthProvider>,
    authority: Arc<AuthAuthority>,
    tables: AuthFileConfig,
    clock: Arc<acteon_time::ManualClock>,
    execution_id: uuid::Uuid,
    proof: AuthenticatedExecutionConfiguration,
}

// Explicit production authentication, capture, and persistence setup for ordered races.
async fn history_read_fixture() -> HistoryReadFixture {
    history_read_fixture_with_reconciliation(false).await
}

async fn history_read_fixture_with_reconciliation(reconciliation: bool) -> HistoryReadFixture {
    history_read_fixture_with_verifier(reconciliation, false).await
}

#[allow(clippy::too_many_lines)]
async fn history_read_fixture_with_verifier(
    reconciliation: bool,
    install_verifier: bool,
) -> HistoryReadFixture {
    use acteon_executor::governed::GovernedProviderExecutor;
    use acteon_governance::{
        context::{ContextSigningKey, ExecutionContextHandle, TrustedContextStore},
        permit::PermitReference,
    };
    use acteon_state::testing::faults::{FaultStore, FaultTiming, WriteOperation};
    let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
    let state: Arc<dyn StateStore> = faults.clone();
    let (registry, actual) = registry_and_provider();
    let clock = Arc::new(acteon_time::ManualClock::new(chrono::Utc::now()));
    let mut configuration = config();
    let scope = &mut configuration.scopes[0];
    let operator = scope.managers[0].principal.clone();
    scope.managers[0].subjects = vec![operator.clone()];
    scope.managers[0].can_read_history = true;
    scope.managers[0].limits.deadline_ms =
        acteon_time::Clock::now(clock.as_ref()).timestamp_millis() + 60_000;
    scope.permits = vec![
        serde_json::from_value(json!({
            "id":"operator-incident", "revision":1, "subject":operator,
            "routes":[{"provider":"incident","action_type":"execute"}], "valid_from_ms":0,
            "limits":{"max_units":5,"max_concurrent":1,"deadline_ms":4_102_444_800_000_i64}
        }))
        .unwrap(),
    ];
    let prepared = registry
        .prepare(&configuration, ("auth-control", "deployment"), &[8; 32])
        .unwrap();
    if reconciliation {
        let manager = &mut configuration.scopes[0].managers[0];
        manager.can_reconcile = true;
        manager.reconciliation_resources = prepared[0]
            .catalog()
            .definitions("prod", "acme")
            .into_iter()
            .flat_map(|d| d.effect.resources)
            .collect();
    }
    let prepared = registry
        .prepare(&configuration, ("auth-control", "deployment"), &[8; 32])
        .unwrap();
    let mut runtime =
        runtime_with_clock(&registry, &configuration, state.clone(), clock.clone()).await;
    if install_verifier {
        let action = acteon_core::Action::new("prod", "acme", "incident", "execute", json!({}));
        let digest = prepared[0]
            .catalog()
            .resolve(&action, &actual)
            .unwrap()
            .reconciliation_binding_digest()
            .unwrap();
        let source = serde_json::from_value(json!({
            "namespace":"prod", "tenant":"acme", "binding_digest":digest,
            "source_id":"test-finality-source", "qualification_ref":"test/external-finality-contract",
            "verifier_revision":"qualified-source-v1",
            "keys":[{"id":"dedicated-key", "secret_env":"ACTEON_FINALITY_TEST_KEY"}]
        })).unwrap();
        let installations = acteon_server::config::prepare_reconciliation_sources(
            &[source],
            Some(&configuration),
            |name| {
                (name == "ACTEON_FINALITY_TEST_KEY")
                    .then(|| zeroize::Zeroizing::new(hex::encode([47; 32])))
            },
        )
        .unwrap();
        runtime = runtime
            .with_trusted_reconciliation_verifiers(installations)
            .unwrap();
    }
    let runtime = Arc::new(runtime);
    let authority = authority(state.clone()).await;
    let mut tables = auth();
    tables.api_keys[0].grants[0].providers = vec!["incident".into()];
    tables.api_keys[0].grants[0].actions = vec!["execute".into()];
    let provider =
        projected_auth_provider(&tables, state.clone(), authority.clone(), &runtime).await;
    runtime.publish_deployment_permits(None).await.unwrap();
    let proof = proof(provider.clone()).await;
    let binding = proof.scope("prod", "acme").unwrap();
    let coordinator = AuthorityCoordinator::connect(state.clone(), "prod", "acme")
        .await
        .unwrap();
    let contexts = Arc::new(
        TrustedContextStore::new(
            state.clone(),
            coordinator.clone(),
            "acteon.server.execution.v1".into(),
            "deployment-v1".into(),
            vec![ContextSigningKey::new("deployment-v1".into(), vec![9; 32]).unwrap()],
        )
        .unwrap(),
    );
    let action = acteon_core::Action::new(
        "prod",
        "acme",
        "incident",
        "execute",
        json!({"incident":42}),
    );
    let execution_id = uuid::Uuid::new_v4();
    let permits = vec![PermitReference {
        id: "operator-incident".into(),
        accepted_revision: 1,
    }];
    let context = prepared[0]
        .capture_root(
            acteon_server::execution_authority::RootExecutionRequest {
                admission_key: "history-read-race",
                handle: ExecutionContextHandle::new(),
                execution_id,
                action: &action,
                selected: &actual,
                authentication: &binding,
                permits: &permits,
            },
            &coordinator,
            &contexts,
            clock.as_ref(),
        )
        .await
        .unwrap();
    let driver = GovernedProviderExecutor::new(
        state.clone(),
        coordinator.clone(),
        contexts,
        prepared[0]
            .catalog()
            .resolve(&action, &actual)
            .unwrap()
            .clone(),
        acteon_executor::ExecutorConfig::default(),
        clock.clone(),
        None,
    )
    .unwrap()
    .require_credential_authority();
    faults
        .fail_next(
            acteon_state::KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    assert!(
        driver
            .execute(&context.reference().unwrap(), &permits, &action, &operator)
            .await
            .is_err()
    );
    assert_eq!(faults.consumed(), 1);
    assert!(coordinator.snapshot().await.unwrap().starts.is_empty());
    assert!(
        runtime
            .inspect_provider_history("prod", "acme", execution_id, &proof)
            .await
            .is_ok()
    );
    HistoryReadFixture {
        faults,
        state,
        runtime,
        provider,
        authority,
        tables,
        clock,
        execution_id,
        proof,
    }
}

#[derive(Debug, Clone, Copy)]
enum HistoryReadRace {
    Unchanged,
    RoleOffboarding,
    CredentialRevocation,
    SubjectRevocation,
    AuthenticationSourceDisable,
    AuthenticationEpochRotation,
    AuthenticationSourceUnavailable,
    UnavailableSourceWithExpiry,
    ManagementExpiry,
    CorruptionWithExpiry,
    ResourceClosure,
}
impl HistoryReadFixture {
    async fn assert_read_response(
        &self,
        race: HistoryReadRace,
        status: axum::http::StatusCode,
        value: &serde_json::Value,
    ) {
        if matches!(race, HistoryReadRace::Unchanged) {
            assert_eq!(status, 200);
            assert_eq!(value["subject"]["id"], "operator");
            assert_eq!(
                value["receipt"]["execution_id"],
                self.execution_id.to_string()
            );
        } else {
            if matches!(
                race,
                HistoryReadRace::CorruptionWithExpiry
                    | HistoryReadRace::UnavailableSourceWithExpiry
            ) {
                assert_eq!(
                    status, 403,
                    "expired reader learned storage integrity: {value}"
                );
            }
            if matches!(race, HistoryReadRace::AuthenticationSourceUnavailable) {
                assert_eq!(
                    status, 503,
                    "missing source was treated as caller input: {value}"
                );
                let key = acteon_state::StateKey::new(
                    "auth-control",
                    "deployment",
                    acteon_state::KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
                    "authority",
                );
                assert!(
                    self.state.get(&key).await.unwrap().is_none(),
                    "read recreated missing authority"
                );
            } else {
                assert!(status == 403 || status == 409, "{race:?}: {status} {value}");
            }
            assert!(value.get("receipt").is_none(), "{race:?} exposed evidence");
            assert!(
                value.get("subject").is_none(),
                "{race:?} exposed participant"
            );
        }
    }
    async fn change_during_read(&mut self, race: HistoryReadRace) {
        let change = match race {
            HistoryReadRace::Unchanged => return,
            HistoryReadRace::RoleOffboarding => {
                self.tables.authority_revision = Some(2);
                self.tables.api_keys[0].role = "viewer".into();
                self.provider.reload(&self.tables).await.unwrap();
                return;
            }
            HistoryReadRace::AuthenticationEpochRotation => {
                self.tables.authority_revision = Some(2);
                self.tables.api_keys[0].role = "viewer".into();
                // Publish from an independent auth-only host. The target scope's
                // execution projection remains unchanged, so it cannot mask this check.
                AuthProvider::new_with_authority(
                    &self.tables,
                    self.state.clone(),
                    self.authority.clone(),
                )
                .await
                .unwrap();
                return;
            }
            HistoryReadRace::AuthenticationSourceDisable => {
                let source =
                    AuthorityCoordinator::connect(self.state.clone(), "auth-control", "deployment")
                        .await
                        .unwrap();
                source
                    .change(
                        "disable-reader-at-source",
                        acteon_governance::AuthorityChange::RevokeSubject {
                            subject: "operator".into(),
                        },
                        "trusted-auth-controller",
                        "ordered offboarding",
                    )
                    .await
                    .unwrap();
                return;
            }
            HistoryReadRace::AuthenticationSourceUnavailable
            | HistoryReadRace::UnavailableSourceWithExpiry => {
                let key = acteon_state::StateKey::new(
                    "auth-control",
                    "deployment",
                    acteon_state::KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
                    "authority",
                );
                assert!(self.state.delete(&key).await.unwrap());
                if matches!(race, HistoryReadRace::UnavailableSourceWithExpiry) {
                    self.clock
                        .advance_to(std::time::Duration::from_secs(60))
                        .unwrap();
                }
                return;
            }
            HistoryReadRace::ManagementExpiry => {
                self.clock
                    .advance_to(std::time::Duration::from_secs(60))
                    .unwrap();
                return;
            }
            HistoryReadRace::CorruptionWithExpiry => {
                let key = acteon_state::StateKey::new(
                    "prod",
                    "acme",
                    acteon_state::KeyKind::Custom(acteon_executor::governed::OPERATION_KIND.into()),
                    self.execution_id.to_string(),
                );
                self.state
                    .set(&key, "corrupted receipt", None)
                    .await
                    .unwrap();
                self.clock
                    .advance_to(std::time::Duration::from_secs(60))
                    .unwrap();
                return;
            }
            HistoryReadRace::CredentialRevocation => json!({"kind":"revoke_credential",
                "credential_id":"credential/operator", "expected_revision":1}),
            HistoryReadRace::SubjectRevocation => json!({"kind":"revoke_subject",
                "subject":{"id":"operator", "kind":"human"}}),
            HistoryReadRace::ResourceClosure => {
                let scope = self
                    .runtime
                    .inspect_governance("prod", "acme", &self.proof)
                    .await
                    .unwrap();
                json!({"kind":"close_resource", "resource":scope.routes[0].effect.resources[0]})
            }
        };
        let request = serde_json::from_value(json!({"namespace":"prod", "tenant":"acme",
            "change_id":"history-read-race", "reason":"ordered authorization test", "change":change})).unwrap();
        self.runtime
            .intervene_governance(request, &self.proof)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn history_http_revalidates_authority_after_a_paused_receipt_read() {
    use acteon_state::testing::faults::{FaultTiming, ReadOperation};
    for race in [
        HistoryReadRace::Unchanged,
        HistoryReadRace::RoleOffboarding,
        HistoryReadRace::CredentialRevocation,
        HistoryReadRace::SubjectRevocation,
        HistoryReadRace::AuthenticationSourceDisable,
        HistoryReadRace::AuthenticationEpochRotation,
        HistoryReadRace::AuthenticationSourceUnavailable,
        HistoryReadRace::UnavailableSourceWithExpiry,
        HistoryReadRace::ManagementExpiry,
        HistoryReadRace::CorruptionWithExpiry,
        HistoryReadRace::ResourceClosure,
    ] {
        let mut fixture = history_read_fixture().await;
        let router = history_router(
            fixture.state.clone(),
            fixture.runtime.clone(),
            fixture.provider.clone(),
        );
        let (reached, resume) = fixture
            .faults
            .pause_next_read(
                acteon_state::KeyKind::Custom(acteon_executor::governed::OPERATION_KIND.into()),
                ReadOperation::Get,
                FaultTiming::After,
            )
            .unwrap();
        let request = Request::builder()
            .uri(format!(
                "/v1/governance/executions/{}?namespace=prod&tenant=acme",
                fixture.execution_id
            ))
            .header("authorization", "Bearer operator-secret")
            .body(Body::empty())
            .unwrap();
        let reading = tokio::spawn(router.clone().oneshot(request));
        tokio::time::timeout(std::time::Duration::from_secs(5), reached)
            .await
            .unwrap()
            .unwrap();
        fixture.change_during_read(race).await;
        let coordinator = AuthorityCoordinator::connect(fixture.state.clone(), "prod", "acme")
            .await
            .unwrap();
        let before_resume = serde_json::to_value(coordinator.snapshot().await.unwrap()).unwrap();
        let source_key = acteon_state::StateKey::new(
            "auth-control",
            "deployment",
            acteon_state::KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
            "authority",
        );
        let source_before_resume = fixture.state.get(&source_key).await.unwrap();
        resume.send(()).unwrap();
        let response = tokio::time::timeout(std::time::Duration::from_secs(5), reading)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        fixture.assert_read_response(race, status, &value).await;
        assert_eq!(fixture.faults.consumed(), 2);
        assert_eq!(
            before_resume,
            serde_json::to_value(coordinator.snapshot().await.unwrap()).unwrap(),
            "{race:?}: read changed authority or accounting"
        );
        assert_eq!(
            source_before_resume,
            fixture.state.get(&source_key).await.unwrap(),
            "{race:?}: read changed authentication authority"
        );
        if matches!(race, HistoryReadRace::ResourceClosure) {
            let fresh = fixture
                .runtime
                .inspect_provider_history("prod", "acme", fixture.execution_id, &fixture.proof)
                .await
                .unwrap();
            assert_eq!(fresh.subject.id(), "operator");
        }
    }
}

#[test]
fn reconciliation_configuration_requires_an_independent_exact_resource_ceiling() {
    use acteon_core::{ResourceKind, ResourceRef};
    let mut configuration = config();
    assert!(!configuration.scopes[0].managers[0].can_reconcile);
    configuration.scopes[0].managers[0].can_reconcile = true;
    assert!(
        configuration
            .validate(("auth-control", "deployment"))
            .is_err()
    );
    let resource = ResourceRef::new(ResourceKind::Endpoint, "prod", "acme", "incident").unwrap();
    configuration.scopes[0].managers[0].reconciliation_resources = vec![resource.clone()];
    configuration
        .validate(("auth-control", "deployment"))
        .unwrap();
    configuration.scopes[0].managers[0]
        .reconciliation_resources
        .push(resource);
    assert!(
        configuration
            .validate(("auth-control", "deployment"))
            .is_err()
    );
    configuration.scopes[0].managers[0].reconciliation_resources =
        vec![ResourceRef::new(ResourceKind::Endpoint, "prod", "foreign", "incident").unwrap()];
    assert!(
        configuration
            .validate(("auth-control", "deployment"))
            .is_err()
    );
    configuration.scopes[0].managers[0].can_reconcile = false;
    assert!(
        configuration
            .validate(("auth-control", "deployment"))
            .is_err()
    );
}

fn installed_finality_verifier()
-> Arc<dyn acteon_executor::governed::reconciliation::ProviderReconciliationVerifier> {
    Arc::new(
        acteon_executor::governed::reconciliation::HmacFinalityVerifier::new_trusted(
            "qualified-source-v1",
            std::collections::BTreeMap::from([("dedicated-key".into(), vec![47; 32])]),
        )
        .unwrap(),
    )
}
#[tokio::test]
async fn trusted_reconciliation_installation_is_exact_bounded_and_does_not_mutate_state() {
    use acteon_server::execution_authority::TrustedReconciliationInstallation;
    for case in 0..5 {
        let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let runtime = runtime(&registry(), &config(), state.clone()).await;
        let coordinator = AuthorityCoordinator::connect(state, "prod", "acme")
            .await
            .unwrap();
        let original = serde_json::to_value(coordinator.snapshot().await.unwrap()).unwrap();
        let installation = || TrustedReconciliationInstallation {
            namespace: if case == 1 { "foreign" } else { "prod" }.into(),
            tenant: "acme".into(),
            bindings: std::collections::BTreeMap::from([(
                if case == 2 {
                    "bad-digest".into()
                } else {
                    "a".repeat(64)
                },
                installed_finality_verifier(),
            )]),
        };
        let installations = match case {
            3 => vec![installation(), installation()],
            4 => vec![],
            _ => vec![installation()],
        };
        let result = runtime.with_trusted_reconciliation_verifiers(installations);
        if case == 0 {
            let runtime = result.unwrap();
            assert!(runtime.reconciliation_store("prod", "acme").is_some());
            assert!(runtime.reconciliation_store("prod", "foreign").is_none());
            assert!(
                runtime
                    .with_trusted_reconciliation_verifiers(vec![installation()])
                    .is_err()
            );
        } else {
            assert!(result.is_err());
        }
        assert_eq!(
            serde_json::to_value(coordinator.snapshot().await.unwrap()).unwrap(),
            original
        );
    }
}

#[tokio::test]
async fn reconciliation_management_requires_its_own_grant_and_host_installation() {
    use acteon_executor::governed::OPERATION_KIND;
    for enabled in [false, true] {
        let f = history_read_fixture_with_reconciliation(enabled).await;
        let key = acteon_state::StateKey::new(
            "prod",
            "acme",
            acteon_state::KeyKind::Custom(OPERATION_KIND.into()),
            f.execution_id.to_string(),
        );
        let raw = f.state.get(&key).await.unwrap().unwrap();
        let op: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let context: acteon_core::ExecutionContextReference =
            serde_json::from_value(op["context"].clone()).unwrap();
        let result = f
            .runtime
            .provider_reconciliation_attempt(&context, 0, &f.proof)
            .await;
        assert!(matches!(
            (enabled, result),
            (false, Err(ManagementError::Forbidden)) | (true, Err(ManagementError::Unavailable))
        ));
        let result = f
            .runtime
            .reconcile_provider_attempt(&context, 0, b"untrusted", &f.proof)
            .await;
        assert!(matches!(
            (enabled, result),
            (false, Err(ManagementError::Forbidden)) | (true, Err(ManagementError::Unavailable))
        ));
        let coordinator = AuthorityCoordinator::connect(f.state.clone(), "prod", "acme")
            .await
            .unwrap();
        assert!(coordinator.snapshot().await.unwrap().starts.is_empty());
    }
}

async fn register_abandoned_reconciliation_attempt(f: &HistoryReadFixture) {
    use acteon_governance::{
        AttemptEvidenceReference,
        context::{ContextSigningKey, TrustedContextStore},
        permit::{PermitReference, PermittedAttempt},
    };
    use sha2::Digest;
    let coordinator = AuthorityCoordinator::connect(f.state.clone(), "prod", "acme")
        .await
        .unwrap();
    let key = acteon_state::StateKey::new(
        "prod",
        "acme",
        acteon_state::KeyKind::Custom(acteon_executor::governed::OPERATION_KIND.into()),
        f.execution_id.to_string(),
    );
    let raw = f.state.get(&key).await.unwrap().unwrap();
    let op: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let reference: acteon_core::ExecutionContextReference =
        serde_json::from_value(op["context"].clone()).unwrap();
    let contexts = TrustedContextStore::new(
        f.state.clone(),
        coordinator.clone(),
        "acteon.server.execution.v1".into(),
        "deployment-v1".into(),
        vec![ContextSigningKey::new("deployment-v1".into(), vec![9; 32]).unwrap()],
    )
    .unwrap();
    let context = contexts
        .recover_reference_for_observation(&reference)
        .await
        .unwrap();
    let permits: Vec<PermitReference> = serde_json::from_value(op["permits"].clone()).unwrap();
    let effect = serde_json::from_value(op["binding"]["effect"].clone()).unwrap();
    let id = uuid::Uuid::new_v5(&f.execution_id, &0_u32.to_be_bytes()).to_string();
    coordinator
        .register_permitted_attempt_with_operation(
            PermittedAttempt {
                id: &id,
                context: &context,
                permits: &permits,
                effect: &effect,
                request_digest: reference.request_digest(),
                units: 1,
                clock: f.clock.as_ref(),
            },
            &AttemptEvidenceReference {
                id: f.execution_id.to_string(),
                digest: format!("{:x}", sha2::Sha256::digest(raw.as_bytes())),
            },
        )
        .await
        .unwrap();
}
fn reconciliation_url(execution_id: uuid::Uuid, tail: &str) -> String {
    format!("/v1/governance/executions/{execution_id}/attempts/0/{tail}?namespace=prod&tenant=acme")
}
async fn reconciliation_http(
    app: Router,
    execution_id: uuid::Uuid,
    evidence: Option<&[u8]>,
) -> axum::response::Response {
    use base64::Engine;
    let mut request = Request::builder()
        .uri(reconciliation_url(
            execution_id,
            if evidence.is_some() {
                "reconciliation"
            } else {
                "correlation"
            },
        ))
        .header("authorization", "Bearer operator-secret");
    let body = if let Some(proof) = evidence {
        request = request
            .method("POST")
            .header("content-type", "application/json");
        Body::from(
            serde_json::to_vec(&acteon_core::ProviderReconciliationRequest {
                proof_base64: base64::engine::general_purpose::STANDARD.encode(proof),
            })
            .unwrap(),
        )
    } else {
        Body::empty()
    };
    app.oneshot(request.body(body).unwrap()).await.unwrap()
}
async fn response_json(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 100_000)
            .await
            .unwrap(),
    )
    .unwrap()
}
#[tokio::test]
async fn reconciliation_http_correlates_accepts_and_replays_without_a_provider_invocation() {
    let f = history_read_fixture_with_verifier(true, true).await;
    register_abandoned_reconciliation_attempt(&f).await;
    let app = history_router(f.state.clone(), f.runtime.clone(), f.provider.clone());
    let response = reconciliation_http(app.clone(), f.execution_id, None).await;
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let correlation: acteon_core::ProviderReconciliationCorrelation =
        serde_json::from_value(response_json(response).await).unwrap();
    assert_eq!(correlation.context.execution_id(), f.execution_id);
    let descriptor = serde_json::from_value(serde_json::to_value(correlation).unwrap()).unwrap();
    let proof = acteon_executor::governed::reconciliation::sign_finality_receipt(
        descriptor,
        acteon_executor::governed::reconciliation::ProviderFinality::NoEffect {
            reason: "source sealed an abandoned attempt before any send".into(),
        },
        "qualified-source-v1",
        "dedicated-key",
        &[47; 32],
    )
    .unwrap();
    let accepted = reconciliation_http(app.clone(), f.execution_id, Some(&proof)).await;
    assert_eq!(accepted.status(), axum::http::StatusCode::OK);
    let receipt: acteon_core::ProviderHistoryReceipt =
        serde_json::from_value(response_json(accepted).await).unwrap();
    assert!(matches!(
        receipt.status,
        acteon_core::ProviderHistoryStatus::Completed { .. }
    ));
    let coordinator = AuthorityCoordinator::connect(f.state.clone(), "prod", "acme")
        .await
        .unwrap();
    let original = coordinator.snapshot().await.unwrap();
    let start = original.starts.values().next().unwrap();
    assert!(start.evidence.is_none());
    assert_eq!(
        start
            .reconciliation_acceptance
            .as_ref()
            .unwrap()
            .operator
            .id(),
        "operator"
    );
    assert_eq!(original.roots[&f.execution_id.to_string()].spent_units, 1);
    assert_eq!(
        original.roots[&f.execution_id.to_string()].active_attempts,
        0
    );
    let replay = reconciliation_http(app.clone(), f.execution_id, Some(&proof)).await;
    assert_eq!(replay.status(), axum::http::StatusCode::OK);
    assert_eq!(
        serde_json::to_value(coordinator.snapshot().await.unwrap()).unwrap(),
        serde_json::to_value(original).unwrap()
    );
    let history = f
        .runtime
        .inspect_provider_history("prod", "acme", f.execution_id, &f.proof)
        .await
        .unwrap();
    assert!(
        history.attempts[0]
            .reconciliation
            .as_ref()
            .unwrap()
            .acceptance
            .is_some()
    );
    assert_eq!(
        reconciliation_http(app, f.execution_id, None)
            .await
            .status(),
        axum::http::StatusCode::CONFLICT
    );
    assert_eq!(
        f.state
            .scan_keys_by_kind(acteon_state::KeyKind::Custom(
                acteon_executor::governed::RESULT_KIND.into()
            ))
            .await
            .unwrap(),
        Vec::new()
    );
}

#[tokio::test]
async fn reconciliation_http_preserves_permission_qualification_and_ownership_refusals() {
    for (granted, qualified, expected) in
        [(false, true, 403), (true, false, 503), (true, true, 404)]
    {
        let f = history_read_fixture_with_verifier(granted, qualified).await;
        let app = history_router(f.state.clone(), f.runtime.clone(), f.provider.clone());
        for proof in [None, Some(b"untrusted".as_slice())] {
            let response = reconciliation_http(app.clone(), uuid::Uuid::new_v4(), proof).await;
            assert_eq!(response.status().as_u16(), expected);
        }
        let unauthenticated = app
            .oneshot(
                Request::builder()
                    .uri(reconciliation_url(f.execution_id, "correlation"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            unauthenticated.status(),
            axum::http::StatusCode::UNAUTHORIZED
        );
    }
}

#[tokio::test]
async fn reconciliation_http_rejects_untrusted_oversized_and_unknown_field_proofs_without_settlement()
 {
    let f = history_read_fixture_with_verifier(true, true).await;
    register_abandoned_reconciliation_attempt(&f).await;
    let app = history_router(f.state.clone(), f.runtime.clone(), f.provider.clone());
    let coordinator = AuthorityCoordinator::connect(f.state.clone(), "prod", "acme")
        .await
        .unwrap();
    let before = serde_json::to_value(coordinator.snapshot().await.unwrap()).unwrap();
    for body in [
        json!({"proof_base64":"%%%"}),
        json!({"proof_base64":"A".repeat(87_385)}),
        json!({"proof_base64":"e30=","verifier":"request-chosen"}),
        json!({"proof_base64":"e30="}),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(reconciliation_url(f.execution_id, "reconciliation"))
                    .header("authorization", "Bearer operator-secret")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.status().is_client_error());
        assert_eq!(
            serde_json::to_value(coordinator.snapshot().await.unwrap()).unwrap(),
            before
        );
    }
    assert_eq!(
        f.state
            .scan_keys_by_kind(acteon_state::KeyKind::Custom(
                acteon_executor::governed::reconciliation::RECONCILIATION_KIND.into()
            ))
            .await
            .unwrap(),
        Vec::new()
    );
}
#[tokio::test]
async fn reconciliation_http_source_offboarding_during_lookup_refuses_both_routes_without_writes() {
    use acteon_state::testing::faults::{FaultTiming, ReadOperation};
    for accepting in [false, true] {
        let mut f = history_read_fixture_with_verifier(true, true).await;
        register_abandoned_reconciliation_attempt(&f).await;
        let app = history_router(f.state.clone(), f.runtime.clone(), f.provider.clone());
        let coordinator = AuthorityCoordinator::connect(f.state.clone(), "prod", "acme")
            .await
            .unwrap();
        let before = serde_json::to_value(coordinator.snapshot().await.unwrap()).unwrap();
        let (reached, resume) = f
            .faults
            .pause_next_read(
                acteon_state::KeyKind::Custom(acteon_executor::governed::OPERATION_KIND.into()),
                ReadOperation::Get,
                FaultTiming::After,
            )
            .unwrap();
        let execution_id = f.execution_id;
        let pending = tokio::spawn(async move {
            reconciliation_http(
                app,
                execution_id,
                accepting.then_some(b"untrusted".as_slice()),
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), reached)
            .await
            .unwrap()
            .unwrap();
        f.change_during_read(HistoryReadRace::AuthenticationSourceDisable)
            .await;
        resume.send(()).unwrap();
        let response = tokio::time::timeout(std::time::Duration::from_secs(5), pending)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
        let value = response_json(response).await;
        assert!(value.get("context").is_none());
        assert!(value.get("token").is_none());
        assert_eq!(
            serde_json::to_value(coordinator.snapshot().await.unwrap()).unwrap(),
            before
        );
        assert_eq!(
            f.state
                .scan_keys_by_kind(acteon_state::KeyKind::Custom(
                    acteon_executor::governed::reconciliation::RECONCILIATION_KIND.into()
                ))
                .await
                .unwrap(),
            Vec::new()
        );
    }
}

fn reconciliation_archive_config() -> ExecutionAuthorityConfig {
    let mut archive = history_archive_config();
    let scope = &mut archive.scopes[0];
    scope.history_only = false;
    scope.reconciliation_only = true;
    let manager = &mut scope.managers[0];
    manager.can_reconcile = true;
    manager.reconciliation_resources = scope
        .historical_effects
        .iter()
        .flat_map(|effect| effect.resources.iter().cloned())
        .collect();
    archive
}

#[test]
fn reconciliation_only_rejects_bootstrap_live_work_and_unrelated_write_grants() {
    let registry = ExecutionProviderRegistry::default();
    let archive = reconciliation_archive_config();
    assert!(
        registry
            .prepare(&archive, ("auth-control", "deployment"), &[8; 32])
            .is_ok()
    );
    for (pointer, value) in [
        ("/scopes/0/bootstrap", json!(true)),
        ("/scopes/0/history_only", json!(true)),
        ("/scopes/0/reconciliation_only", json!(false)),
        ("/scopes/0/historical_effects", json!([])),
        (
            "/scopes/0/routes",
            json!([{"provider":"incident","action_type":"execute"}]),
        ),
        (
            "/scopes/0/chains",
            json!([{"name":"incident","subjects":[{"id":"agent/maya","kind":"agent"}]}]),
        ),
        (
            "/scopes/0/permits",
            json!([{"id":"new", "revision":1,"subject":{"id":"agent/maya","kind":"agent"},
            "routes":[],"valid_from_ms":0,"limits":{"max_units":1,"max_concurrent":1,"deadline_ms":4_102_444_800_000_i64}}]),
        ),
        ("/scopes/0/managers/0/can_issue_permits", json!(true)),
        ("/scopes/0/managers/0/can_intervene", json!(true)),
        (
            "/scopes/0/managers/0/workforce",
            json!({"teams":[],"job_classes":[],"can_manage_roster":true}),
        ),
    ] {
        let mut value_config = serde_json::to_value(&archive).unwrap();
        let (parent, field) = pointer.rsplit_once('/').unwrap();
        value_config
            .pointer_mut(parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(field.into(), value);
        let candidate: Result<ExecutionAuthorityConfig, _> = serde_json::from_value(value_config);
        assert!(
            candidate.is_err()
                || registry
                    .prepare(
                        &candidate.unwrap(),
                        ("auth-control", "deployment"),
                        &[8; 32]
                    )
                    .is_err(),
            "{pointer}"
        );
    }
}

async fn deploy_reconciliation_archive(
    f: &mut HistoryReadFixture,
    digest: String,
    can_read_history: bool,
) {
    let mut archive = reconciliation_archive_config();
    let manager = &mut archive.scopes[0].managers[0];
    manager.subjects = vec![manager.principal.clone()];
    manager.can_read_history = can_read_history;
    let empty = ExecutionProviderRegistry::default();
    let retained = runtime_with_clock(&empty, &archive, f.state.clone(), f.clock.clone()).await;
    let source = serde_json::from_value(json!({
        "namespace":"prod","tenant":"acme","binding_digest":digest,
        "source_id":"retired-incident-journal","qualification_ref":"test/irrevocable-finality",
        "verifier_revision":"qualified-source-v1",
        "keys":[{"id":"dedicated-key","secret_env":"ACTEON_FINALITY_TEST_KEY"}]
    }))
    .unwrap();
    let installations =
        acteon_server::config::prepare_reconciliation_sources(&[source], Some(&archive), |name| {
            (name == "ACTEON_FINALITY_TEST_KEY")
                .then(|| zeroize::Zeroizing::new(hex::encode([47; 32])))
        })
        .unwrap();
    f.runtime = Arc::new(
        retained
            .with_trusted_reconciliation_verifiers(installations)
            .unwrap(),
    );
    f.tables.authority_revision = Some(2);
    f.provider =
        projected_auth_provider(&f.tables, f.state.clone(), f.authority.clone(), &f.runtime).await;
    f.proof = proof(f.provider.clone()).await;
}

#[tokio::test]
async fn reconciliation_only_settles_retained_attempts_without_live_provider_authority() {
    let mut f = history_read_fixture_with_verifier(true, true).await;
    register_abandoned_reconciliation_attempt(&f).await;
    let correlation = f
        .runtime
        .provider_reconciliation_context("prod", "acme", f.execution_id, &f.proof)
        .await
        .unwrap();
    let attempt = f
        .runtime
        .provider_reconciliation_attempt(&correlation, 0, &f.proof)
        .await
        .unwrap();
    let signed = acteon_executor::governed::reconciliation::sign_finality_receipt(
        attempt.clone(),
        acteon_executor::governed::reconciliation::ProviderFinality::NoEffect {
            reason: "external source irrevocably fenced the retained attempt".into(),
        },
        "qualified-source-v1",
        "dedicated-key",
        &[47; 32],
    )
    .unwrap();
    let old_proof = f.proof.clone();
    deploy_reconciliation_archive(&mut f, attempt.binding_digest, true).await;
    let coordinator = AuthorityCoordinator::connect(f.state.clone(), "prod", "acme")
        .await
        .unwrap();
    let snapshot = coordinator.snapshot().await.unwrap();
    assert_eq!(
        snapshot.roots[&f.execution_id.to_string()].active_attempts,
        1
    );
    assert!(matches!(
        f.runtime
            .inspect_provider_history("prod", "acme", f.execution_id, &old_proof)
            .await,
        Err(ManagementError::Forbidden)
    ));
    assert!(matches!(
        f.runtime
            .reconcile_provider_attempt(&correlation, 0, &signed, &old_proof)
            .await,
        Err(ManagementError::Forbidden)
    ));
    let app = history_router(f.state.clone(), f.runtime.clone(), f.provider.clone());
    assert_eq!(
        reconciliation_http(app.clone(), f.execution_id, None)
            .await
            .status(),
        200
    );
    let response = reconciliation_http(app.clone(), f.execution_id, Some(&signed)).await;
    assert_eq!(response.status(), 200);
    let receipt: acteon_core::ProviderHistoryReceipt =
        serde_json::from_value(response_json(response).await).unwrap();
    assert!(matches!(
        receipt.status,
        acteon_core::ProviderHistoryStatus::Completed {
            outcome: acteon_core::ActionOutcome::Failed(_)
        }
    ));
    let settled = serde_json::to_value(coordinator.snapshot().await.unwrap()).unwrap();
    assert_eq!(
        reconciliation_http(app, f.execution_id, Some(&signed))
            .await
            .status(),
        200
    );
    assert_eq!(
        serde_json::to_value(coordinator.snapshot().await.unwrap()).unwrap(),
        settled
    );
    assert_eq!(
        settled["roots"][f.execution_id.to_string()]["active_attempts"],
        0
    );
    let history = f
        .runtime
        .inspect_provider_history("prod", "acme", f.execution_id, &f.proof)
        .await
        .unwrap();
    assert!(
        history.attempts[0]
            .reconciliation
            .as_ref()
            .unwrap()
            .acceptance
            .is_some()
    );
    assert_eq!(
        f.state
            .scan_keys_by_kind(acteon_state::KeyKind::Custom(
                acteon_executor::governed::RESULT_KIND.into()
            ))
            .await
            .unwrap(),
        Vec::new()
    );
    assert_archive_denies_live_management(&f).await;
}

#[tokio::test]
async fn reconciliation_only_requires_existing_state_without_bootstrap() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let registry = ExecutionProviderRegistry::default();
    let prepared = registry
        .prepare(
            &reconciliation_archive_config(),
            ("auth-control", "deployment"),
            &[8; 32],
        )
        .unwrap();
    let result = ExecutionAuthorityRuntime::install(
        &registry,
        prepared,
        ExecutionRuntimeDependencies {
            state: state.clone(),
            executor: acteon_executor::ExecutorConfig::default(),
            clock: Arc::new(acteon_time::SystemClock::default()),
            encryptor: None,
            signing_key: vec![9; 32].into(),
        },
    )
    .await;
    assert!(result.is_err());
    assert_eq!(
        state
            .scan_keys_by_kind(acteon_state::KeyKind::Custom(
                acteon_governance::COORDINATOR_KIND.into()
            ))
            .await
            .unwrap(),
        Vec::new()
    );
}

async fn assert_archive_denies_live_management(f: &HistoryReadFixture) {
    let coordinator = AuthorityCoordinator::connect(f.state.clone(), "prod", "acme")
        .await
        .unwrap();
    let snapshot = coordinator.snapshot().await.unwrap();
    let credential = &snapshot.credentials["credential/operator"].authority;
    assert!(!credential.execution_enabled);
    assert_eq!(credential.ceiling.effects, Vec::new());
    let action = acteon_core::Action::new("prod", "acme", "incident", "execute", json!({}));
    assert!(f.runtime.verify_request(&action, &f.proof).await.is_err());
    let publication = serde_json::from_value(json!({
        "namespace":"prod","tenant":"acme","change_id":"archive-cannot-issue",
        "expected_revision":0,"reason":"attempt to issue live authority",
        "permit":{"id":"forbidden","revision":1,"subject":credential.ceiling.subject,
            "routes":[{"provider":"incident","action_type":"execute"}],"valid_from_ms":0,
            "limits":{"max_units":1,"max_concurrent":1,"deadline_ms":4_102_444_800_000_i64}}
    }))
    .unwrap();
    assert!(matches!(
        f.runtime
            .publish_governance_permit(publication, &f.proof)
            .await,
        Err(ManagementError::Forbidden)
    ));
    let intervention = serde_json::from_value(json!({
        "namespace":"prod","tenant":"acme","change_id":"archive-cannot-revoke",
        "reason":"attempt unrelated management mutation",
        "change":{"kind":"revoke_subject","subject":credential.ceiling.subject}
    }))
    .unwrap();
    assert!(matches!(
        f.runtime.intervene_governance(intervention, &f.proof).await,
        Err(ManagementError::Forbidden)
    ));
    assert_eq!(
        serde_json::to_value(coordinator.snapshot().await.unwrap()).unwrap(),
        serde_json::to_value(snapshot).unwrap()
    );
}

#[tokio::test]
async fn reconciliation_only_does_not_require_or_imply_history_access() {
    let mut f = history_read_fixture_with_verifier(true, true).await;
    register_abandoned_reconciliation_attempt(&f).await;
    let context = f
        .runtime
        .provider_reconciliation_context("prod", "acme", f.execution_id, &f.proof)
        .await
        .unwrap();
    let attempt = f
        .runtime
        .provider_reconciliation_attempt(&context, 0, &f.proof)
        .await
        .unwrap();
    let signed = acteon_executor::governed::reconciliation::sign_finality_receipt(
        attempt.clone(),
        acteon_executor::governed::reconciliation::ProviderFinality::NoEffect {
            reason: "external source irrevocably fenced the retained attempt".into(),
        },
        "qualified-source-v1",
        "dedicated-key",
        &[47; 32],
    )
    .unwrap();
    deploy_reconciliation_archive(&mut f, attempt.binding_digest, false).await;
    assert!(matches!(
        f.runtime
            .inspect_provider_history("prod", "acme", f.execution_id, &f.proof)
            .await,
        Err(ManagementError::Forbidden)
    ));
    let app = history_router(f.state.clone(), f.runtime.clone(), f.provider.clone());
    assert_eq!(
        reconciliation_http(app.clone(), f.execution_id, None)
            .await
            .status(),
        200
    );
    assert_eq!(
        reconciliation_http(app, f.execution_id, Some(&signed))
            .await
            .status(),
        200
    );
    assert!(matches!(
        f.runtime
            .inspect_provider_history("prod", "acme", f.execution_id, &f.proof)
            .await,
        Err(ManagementError::Forbidden)
    ));
    assert_archive_denies_live_management(&f).await;
}
