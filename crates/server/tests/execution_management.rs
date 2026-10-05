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
        "valid_from_ms":0, "credential_limits":{"max_units":5,"max_concurrent":2,"deadline_ms":4102444800000_i64},
        "root_max_units":5, "root_max_concurrent":1, "root_lifetime_ms":60000,
        "managers":[{"principal":{"id":"operator","kind":"human"},"subjects":[{"id":"agent/maya","kind":"agent"}],
            "routes":[{"provider":"incident","action_type":"execute"}],"valid_from_ms":0,
            "limits":{"max_units":5,"max_concurrent":2,"deadline_ms":4102444800000_i64},
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
    let config: ProviderConfig =
        toml::from_str("name='incident'\ntype='webhook'\nurl='https://example.com/incident'\n")
            .unwrap();
    let factory = StaticWebhook::build(&config, None).unwrap();
    let mut registry = ExecutionProviderRegistry::default();
    registry
        .register(factory.provider(), Some(factory))
        .unwrap();
    registry
}
async fn runtime(
    registry: &ExecutionProviderRegistry,
    config: &ExecutionAuthorityConfig,
    state: Arc<dyn StateStore>,
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
            clock: Arc::new(acteon_time::SystemClock::default()),
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
