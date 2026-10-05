use std::sync::{Arc, Mutex};

use axum::{Extension, Router, body::Body, http::Request, routing::get};
use tower::ServiceExt;

use acteon_core::{Action, PrincipalIdentity, PrincipalKind};
use acteon_executor::{
    GovernedProviderMediator, ProviderExecutionMediator, ProviderInvocation,
    ProviderInvocationOrigin,
};
use acteon_gateway::{CircuitBreakerConfig, GatewayBuilder};
use acteon_governance::{AuthorityCoordinator, CoordinatorLimits};
use acteon_provider::{DynProvider, LogProvider};
use acteon_rules::ir::{
    expr::Expr,
    rule::{Rule, RuleAction},
};
use acteon_server::{
    auth::{
        AuthProvider,
        api_key::hash_api_key,
        authority::AuthAuthority,
        config::{ApiKeyConfig, AuthFileConfig, AuthSettings, Grant},
        crypto::SecretString,
        middleware::AuthLayer,
        projection::{AuthenticatedExecutionConfiguration, ScopedCredentialBinding},
    },
    config::{AuthAuthorityConfig, ExecutionAuthorityConfig, ProviderConfig},
    execution_authority::{ExecutionProviderRegistry, RootExecutionRequest},
    provider_factory::StaticWebhook,
};
use acteon_state::StateStore;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use serde_json::json;

async fn authenticated_binding(provider: Arc<AuthProvider>) -> ScopedCredentialBinding {
    let saved = Arc::new(Mutex::new(None));
    let capture = saved.clone();
    let app = Router::new()
        .route(
            "/proof",
            get(
                move |Extension(proof): Extension<AuthenticatedExecutionConfiguration>| {
                    let capture = capture.clone();
                    async move {
                        *capture.lock().unwrap() = Some(proof.scope("prod", "acme").unwrap());
                    }
                },
            ),
        )
        .layer(AuthLayer::new(Some(provider)));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/proof")
                .header("authorization", "Bearer maya-secret")
                .header("x-execution-policy", "forged")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    saved.lock().unwrap().take().unwrap()
}

fn configuration() -> ExecutionAuthorityConfig {
    serde_json::from_value(json!({"scopes": [{
        "namespace": "prod", "tenant": "acme",
        "publisher": {"id": "scope-publisher", "kind": "system"},
        "subjects": [{"id": "agent/maya", "kind": "agent"}],
        "routes": [{"provider": "incident", "action_type": "execute"}],
        "valid_from_ms": 0,
        "credential_limits": {"max_units": 5, "max_concurrent": 2, "deadline_ms": 4_102_444_800_000_i64},
        "root_max_units": 5, "root_max_concurrent": 1, "root_lifetime_ms": 60000
    }]})).unwrap()
}

fn webhook() -> StaticWebhook {
    let config: ProviderConfig = toml::from_str(
        "name = 'incident'\ntype = 'webhook'\nurl = 'https://example.com/incident'\n",
    )
    .unwrap();
    StaticWebhook::build(&config, None).unwrap()
}

fn registry() -> (ExecutionProviderRegistry, Arc<dyn DynProvider>) {
    let factory = webhook();
    let actual = factory.provider();
    let mut registry = ExecutionProviderRegistry::default();
    registry.register(actual.clone(), Some(factory)).unwrap();
    (registry, actual)
}

#[test]
fn preparation_requires_the_actual_qualified_registration() {
    let (registry, actual) = registry();
    let config = configuration();
    let prepared = registry
        .prepare(&config, ("auth-control", "deployment"), &[8; 32])
        .unwrap();
    let action = Action::new(
        "prod",
        "acme",
        "requested-before-fallback",
        "execute",
        json!({}),
    );
    assert!(prepared[0].catalog().resolve(&action, &actual).is_ok());
    assert!(
        prepared[0]
            .catalog()
            .resolve(&action, &webhook().provider())
            .is_err()
    );
    let mut mismatched = ExecutionProviderRegistry::default();
    assert!(
        mismatched
            .register(actual.clone(), Some(webhook()))
            .is_err()
    );
    mismatched.register(actual, None).unwrap();
    assert!(
        mismatched
            .prepare(&config, ("auth-control", "deployment"), &[8; 32])
            .is_err()
    );
    let mut missing = config.clone();
    missing.scopes[0].routes[0].provider = "missing".into();
    assert!(
        registry
            .prepare(&missing, ("auth-control", "deployment"), &[8; 32])
            .is_err()
    );
    let mut duplicate = ExecutionProviderRegistry::default();
    let one: Arc<dyn DynProvider> = Arc::new(LogProvider::new("incident"));
    duplicate.register(one.clone(), None).unwrap();
    assert!(duplicate.register(one, None).is_err());
}

#[test]
fn deployment_policy_is_canonical_and_binds_root_admission_limits() {
    let (registry, _) = registry();
    let mut config = configuration();
    config.scopes[0]
        .subjects
        .push(PrincipalIdentity::new("agent/rowan", PrincipalKind::Agent).unwrap());
    let original = registry
        .prepare(&config, ("auth-control", "deployment"), &[8; 32])
        .unwrap();
    let mut reordered = config.clone();
    reordered.scopes[0].subjects.reverse();
    reordered.scopes[0].bootstrap = true;
    let equivalent = registry
        .prepare(&reordered, ("auth-control", "deployment"), &[8; 32])
        .unwrap();
    assert_eq!(
        original[0].policy_fingerprint(),
        equivalent[0].policy_fingerprint()
    );
    for variation in 0..3 {
        let mut changed = config.clone();
        match variation {
            0 => changed.scopes[0].root_max_units = 4,
            1 => changed.scopes[0].root_max_concurrent = 2,
            _ => changed.scopes[0].root_lifetime_ms = 30000,
        }
        let changed = registry
            .prepare(&changed, ("auth-control", "deployment"), &[8; 32])
            .unwrap();
        assert_ne!(
            original[0].policy_fingerprint(),
            changed[0].policy_fingerprint()
        );
        assert_eq!(
            original[0].catalog().fingerprint(),
            changed[0].catalog().fingerprint()
        );
    }
}

#[tokio::test]
async fn root_policy_changes_require_a_new_security_revision() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    root_policy_contract(state.clone(), state).await;
}

#[allow(clippy::too_many_lines)] // One authenticated admission and policy-transition contract.
async fn root_policy_contract(state: Arc<dyn StateStore>, peer: Arc<dyn StateStore>) {
    let (registry, actual) = registry();
    let mut config = configuration();
    let original = registry
        .prepare(&config, ("auth-control", "deployment"), &[8; 32])
        .unwrap();
    let control = AuthorityCoordinator::initialize(
        state.clone(),
        "auth-control",
        "deployment",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap();
    let scope = AuthorityCoordinator::initialize(
        state.clone(),
        "prod",
        "acme",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap();
    let authority = AuthAuthority::new(
        control,
        &AuthAuthorityConfig {
            namespace: "auth-control".into(),
            tenant: "deployment".into(),
            source_id: "deployment-auth".into(),
            bootstrap: false,
        },
        SecretString::new("shared-security-fingerprint-key-32-bytes".into()),
    )
    .unwrap();
    let mut auth = AuthFileConfig {
        authority_revision: Some(1),
        settings: AuthSettings {
            jwt_secret: SecretString::new("jwt-signing-key-at-least-32-bytes".into()),
            jwt_expiry_seconds: 3600,
        },
        users: Vec::new(),
        api_keys: vec![ApiKeyConfig {
            authority_id: Some("credential/maya".into()),
            name: "maya".into(),
            principal: Some(config.scopes[0].subjects[0].clone()),
            key_hash: SecretString::new(hash_api_key("maya-secret").into()),
            role: "executor".into(),
            grants: vec![Grant {
                namespaces: vec!["prod".into()],
                tenants: vec!["acme".into()],
                providers: vec!["incident".into()],
                actions: vec!["execute".into()],
                agent_id: None,
            }],
        }],
    };
    let authority = Arc::new(authority);
    let projector = Arc::new(original[0].projector(scope.clone()).await.unwrap());
    let provider = Arc::new(
        AuthProvider::new_with_scope_projection(
            &auth,
            state.clone(),
            authority.clone(),
            vec![projector],
        )
        .await
        .unwrap(),
    );
    let old_binding = authenticated_binding(provider).await;
    let now = chrono::Utc::now().timestamp_millis();
    original[0]
        .verify_authenticated_scope(&old_binding, &scope, now)
        .await
        .unwrap();
    let contexts = acteon_governance::context::TrustedContextStore::new(
        state.clone(),
        scope.clone(),
        "test-execution".into(),
        "key-v1".into(),
        vec![
            acteon_governance::context::ContextSigningKey::new("key-v1".into(), vec![9; 32])
                .unwrap(),
        ],
    )
    .unwrap();
    let clock = acteon_time::SystemClock::default();
    let action = Action::new(
        "prod",
        "acme",
        "incident",
        "execute",
        json!({"incident": 42}),
    );
    let request = |authentication, permits| RootExecutionRequest {
        admission_key: "incident-42",
        handle: acteon_governance::context::ExecutionContextHandle::new(),
        execution_id: uuid::Uuid::new_v4(),
        action: &action,
        selected: &actual,
        authentication,
        permits,
    };
    assert!(
        original[0]
            .capture_root(request(&old_binding, &[]), &scope, &contexts, &clock)
            .await
            .is_err()
    );
    assert!(scope.snapshot().await.unwrap().roots.is_empty());
    let effect = original[0].catalog().definitions("prod", "acme")[0]
        .effect
        .clone();
    let limits = config.scopes[0].credential_limits.clone();
    let ceiling = acteon_governance::permit::PermitIssuanceCeiling {
        issuer: config.scopes[0].publisher.clone(),
        subjects: config.scopes[0].subjects.clone(),
        effects: vec![effect.clone()],
        valid_from_ms: 0,
        limits: limits.clone(),
    };
    scope
        .publish_permit(
            "issue-root-permit",
            acteon_governance::permit::ExecutionPermit {
                id: "maya-incident".into(),
                revision: 1,
                subject: config.scopes[0].subjects[0].clone(),
                effects: vec![effect],
                valid_from_ms: 0,
                limits,
            },
            0,
            &ceiling,
            &scope.snapshot().await.unwrap().stamp(),
            "explicit test issuance",
            now,
        )
        .await
        .unwrap();
    let permits = [acteon_governance::permit::PermitReference {
        id: "maya-incident".into(),
        accepted_revision: 1,
    }];
    let admitted = original[0]
        .capture_root(request(&old_binding, &permits), &scope, &contexts, &clock)
        .await
        .unwrap();
    assert_eq!(admitted.principal(), &config.scopes[0].subjects[0]);
    assert_eq!(
        admitted.credential_authority(),
        Some(old_binding.credential_reference())
    );
    assert_eq!(scope.snapshot().await.unwrap().roots.len(), 1);
    let peer_scope = AuthorityCoordinator::connect(peer.clone(), "prod", "acme")
        .await
        .unwrap();
    let peer_contexts = acteon_governance::context::TrustedContextStore::new(
        peer,
        peer_scope.clone(),
        "test-execution".into(),
        "key-v1".into(),
        vec![
            acteon_governance::context::ContextSigningKey::new("key-v1".into(), vec![9; 32])
                .unwrap(),
        ],
    )
    .unwrap();
    let replayed = original[0]
        .capture_root(
            request(&old_binding, &permits),
            &peer_scope,
            &peer_contexts,
            &clock,
        )
        .await
        .unwrap();
    assert_eq!(admitted.reference().unwrap(), replayed.reference().unwrap());
    assert_eq!(admitted.deadline_ms(), replayed.deadline_ms());
    let invocation_authority = original[0]
        .capture_provider_authority(request(&old_binding, &permits), &scope, &contexts, &clock)
        .await
        .unwrap();

    let before = serde_json::to_value(scope.snapshot().await.unwrap()).unwrap();
    config.scopes[0].root_max_units = 4;
    let changed = registry
        .prepare(&config, ("auth-control", "deployment"), &[8; 32])
        .unwrap();
    let changed_projector = changed[0].projector(scope.clone()).await.unwrap();
    assert!(
        changed_projector
            .publish(&authority, &auth, 2)
            .await
            .is_err()
    );
    assert_eq!(
        serde_json::to_value(scope.snapshot().await.unwrap()).unwrap(),
        before
    );
    auth.authority_revision = Some(2);
    let provider = Arc::new(
        AuthProvider::new_with_scope_projection(
            &auth,
            state.clone(),
            authority,
            vec![Arc::new(changed_projector)],
        )
        .await
        .unwrap(),
    );
    let new_binding = authenticated_binding(provider).await;
    // Original references cannot be refreshed after a source transition.
    assert!(
        original[0]
            .verify_authenticated_scope(&old_binding, &scope, now)
            .await
            .is_err()
    );
    assert!(
        original[0]
            .capture_root(request(&new_binding, &permits), &scope, &contexts, &clock)
            .await
            .is_err()
    );
    assert_eq!(scope.snapshot().await.unwrap().roots.len(), 1);
    // Fresh authentication does not authorize an old runtime's larger root.
    assert!(
        original[0]
            .verify_authenticated_scope(&new_binding, &scope, now)
            .await
            .is_err()
    );
    changed[0]
        .verify_authenticated_scope(&new_binding, &scope, now)
        .await
        .unwrap();
    assert!(
        changed[0]
            .capture_root(request(&old_binding, &permits), &scope, &contexts, &clock)
            .await
            .is_err()
    );
    let mut new_request = request(&new_binding, &permits);
    new_request.admission_key = "incident-42-next-operation";
    let new_root = changed[0]
        .capture_root(new_request, &scope, &contexts, &clock)
        .await
        .unwrap();
    let snapshot = scope.snapshot().await.unwrap();
    assert_eq!(snapshot.roots.len(), 2);
    assert_eq!(
        snapshot.roots[&new_root.execution_id().to_string()]
            .limits
            .max_units,
        4
    );
    // Admission from the original private authentication proof cannot bypass
    // subsequent credential revocation at the actual provider gate.
    let credential_revision = scope.snapshot().await.unwrap().credentials
        [&old_binding.credential_reference().id]
        .authority
        .ceiling
        .revision;
    scope
        .change(
            "stop-original-credential",
            acteon_governance::AuthorityChange::RevokeCredential {
                credential_id: old_binding.credential_reference().id.clone(),
                expected_revision: credential_revision,
            },
            "operator",
            "stop execution",
        )
        .await
        .unwrap();
    let driver = acteon_executor::governed::GovernedProviderExecutor::new(
        state,
        scope.clone(),
        Arc::new(contexts),
        original[0]
            .catalog()
            .resolve(&action, &actual)
            .unwrap()
            .clone(),
        acteon_executor::ExecutorConfig::default(),
        Arc::new(clock),
        None,
    )
    .unwrap();
    let mediator = GovernedProviderMediator::new(vec![driver]).unwrap();
    let outcome = mediator
        .execute(ProviderInvocation {
            action: &action,
            selected: &actual,
            context: None,
            origin: ProviderInvocationOrigin::Dispatch,
            authority: Some(&invocation_authority),
        })
        .await;
    let acteon_core::ActionOutcome::Failed(error) = outcome else {
        panic!("revoked credential executed a provider");
    };
    assert_eq!(error.code, "GOVERNED_EXECUTION_REFUSED");
    let snapshot = scope.snapshot().await.unwrap();
    assert!(snapshot.starts.is_empty());
    assert_eq!(
        snapshot.roots[&admitted.execution_id().to_string()].spent_units,
        0
    );
}

#[cfg(feature = "redis")]
#[tokio::test]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; independent Redis clients"]
async fn independent_redis_root_admission_passes_the_contract() {
    use acteon_state_redis::{RedisConfig, RedisStateStore};
    let config = RedisConfig {
        url: std::env::var("ACTEON_GOVERNANCE_REDIS_URL").unwrap(),
        prefix: format!("root_admission_{}", uuid::Uuid::new_v4().simple()),
        ..Default::default()
    };
    scope_upgrade_cli_contract(
        Arc::new(RedisStateStore::new(&config).unwrap()),
        &format!(
            "backend = 'redis'\nurl = {:?}\nprefix = {:?}",
            config.url, config.prefix
        ),
    )
    .await;
    root_policy_contract(
        Arc::new(RedisStateStore::new(&config).unwrap()),
        Arc::new(RedisStateStore::new(&config).unwrap()),
    )
    .await;
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires DATABASE_URL; independent PostgreSQL clients"]
async fn independent_postgres_root_admission_passes_the_contract() {
    use acteon_state_postgres::{PostgresConfig, PostgresStateStore};
    let config = PostgresConfig {
        url: std::env::var("DATABASE_URL").unwrap(),
        table_prefix: format!("root_admission_{}_", uuid::Uuid::new_v4().simple()),
        ..Default::default()
    };
    scope_upgrade_cli_contract(
        Arc::new(PostgresStateStore::new(config.clone()).await.unwrap()),
        &format!(
            "backend = 'postgres'\nurl = {:?}\nprefix = {:?}",
            config.url, config.table_prefix
        ),
    )
    .await;
    root_policy_contract(
        Arc::new(PostgresStateStore::new(config.clone()).await.unwrap()),
        Arc::new(PostgresStateStore::new(config.clone()).await.unwrap()),
    )
    .await;
    let pool = sqlx::PgPool::connect(&config.url).await.unwrap();
    // Only tables allocated by this UUID-scoped fixture.
    for suffix in ["state", "locks", "timeout_index", "chain_ready_index"] {
        sqlx::query(&format!(
            "DROP TABLE public.{}{suffix}",
            config.table_prefix
        ))
        .execute(&pool)
        .await
        .unwrap();
    }
    pool.close().await;
}

#[cfg(feature = "dynamodb")]
#[tokio::test]
#[ignore = "requires DYNAMODB_ENDPOINT; independent DynamoDB Local clients"]
async fn independent_dynamodb_root_admission_passes_the_contract() {
    use acteon_state_dynamodb::{DynamoConfig, DynamoStateStore, build_client, create_table};
    let config = DynamoConfig {
        endpoint_url: Some(std::env::var("DYNAMODB_ENDPOINT").unwrap()),
        table_name: format!("root_admission_{}", uuid::Uuid::new_v4().simple()),
        key_prefix: format!("root_admission_{}", uuid::Uuid::new_v4().simple()),
        ..Default::default()
    };
    let client = build_client(&config).await;
    create_table(&client, &config.table_name).await.unwrap();
    scope_upgrade_cli_contract(
        Arc::new(DynamoStateStore::new(&config).await.unwrap()),
        &format!(
            "backend = 'dynamodb'\nurl = {:?}\nprefix = {:?}\ntable_name = {:?}\nregion = {:?}",
            config.endpoint_url.as_ref().unwrap(),
            config.key_prefix,
            config.table_name,
            config.region
        ),
    )
    .await;
    root_policy_contract(
        Arc::new(DynamoStateStore::new(&config).await.unwrap()),
        Arc::new(DynamoStateStore::new(&config).await.unwrap()),
    )
    .await;
    client
        .delete_table()
        .table_name(&config.table_name)
        .send()
        .await
        .unwrap();
}

#[tokio::test]
async fn preparation_cannot_adopt_another_deployments_authentication_control_scope() {
    let (registry, _) = registry();
    let prepared = registry
        .prepare(&configuration(), ("auth-control", "deployment"), &[8; 32])
        .unwrap();
    let scope = AuthorityCoordinator::initialize(
        Arc::new(MemoryStateStore::new()),
        "prod",
        "acme",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap();
    scope
        .reserve_scope(acteon_governance::ScopePurpose::AuthenticationControl {
            source_id: "another-deployment".into(),
        })
        .await
        .unwrap();
    let before = serde_json::to_value(scope.snapshot().await.unwrap()).unwrap();
    assert!(prepared[0].projector(scope.clone()).await.is_err());
    assert_eq!(
        serde_json::to_value(scope.snapshot().await.unwrap()).unwrap(),
        before
    );
}

#[cfg(any(feature = "redis", feature = "postgres", feature = "dynamodb"))]
async fn scope_upgrade_cli_contract(state: Arc<dyn StateStore>, backend: &str) {
    use acteon_governance::ScopePurpose;
    let (key, incarnation) = legacy_control_fixture(&state).await;
    let before = state.get_versioned(&key).await.unwrap();
    let path = std::env::temp_dir().join(format!("acteon-cutover-{}.toml", uuid::Uuid::new_v4()));
    std::fs::write(&path, format!("[state]\n{backend}\n")).unwrap();
    let invoke = |digest: Option<&str>| {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_acteon-server"));
        command.arg("--config").arg(&path).args([
            "scope-upgrade",
            "--namespace",
            "legacy-control",
            "--tenant",
            "cutover",
            "--purpose",
            "authentication-control",
            "--source-id",
            "legacy-auth",
            "--actor",
            "operator",
            "--reason",
            "reviewed cutover",
        ]);
        if let Some(digest) = digest {
            command.args(["--review-digest", digest]);
        }
        command.output().unwrap()
    };
    let preview = invoke(None);
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(report["status"], "preview");
    assert_eq!(report["plan"]["from_protocol"], 7);
    assert_eq!(state.get_versioned(&key).await.unwrap(), before);
    let refused = invoke(Some("unreviewed"));
    assert!(!refused.status.success());
    assert_eq!(state.get_versioned(&key).await.unwrap(), before);
    let applied = invoke(Some(report["plan"]["review_digest"].as_str().unwrap()));
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&applied.stdout).unwrap();
    assert_eq!(report["status"], "applied");
    let migrated = AuthorityCoordinator::connect(state, "legacy-control", "cutover")
        .await
        .unwrap()
        .snapshot()
        .await
        .unwrap();
    assert_eq!(
        migrated.purpose,
        ScopePurpose::AuthenticationControl {
            source_id: "legacy-auth".into()
        }
    );
    assert_eq!(migrated.incarnation, incarnation);
    assert_eq!(migrated.credential_configurations.len(), 1);
    assert_eq!(migrated.changes.len(), 2);
    std::fs::remove_file(path).unwrap();
}

#[cfg(any(feature = "redis", feature = "postgres", feature = "dynamodb"))]
async fn legacy_control_fixture(state: &Arc<dyn StateStore>) -> (acteon_state::StateKey, String) {
    use acteon_core::{ResourceKind, ResourceRef};
    use acteon_governance::{
        RootBudgetLimits, configuration::CredentialConfiguration, context::AcceptedEffect,
        permit::PermitIssuanceCeiling,
    };
    use acteon_state::{KeyKind, StateKey};
    let coordinator = AuthorityCoordinator::initialize(
        state.clone(),
        "legacy-control",
        "cutover",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap();
    let ceiling = PermitIssuanceCeiling {
        issuer: PrincipalIdentity::new("legacy-publisher", PrincipalKind::System).unwrap(),
        subjects: vec![PrincipalIdentity::new("operator", PrincipalKind::Human).unwrap()],
        effects: vec![AcceptedEffect {
            operation: "auth.configuration.publish".into(),
            resources: vec![
                ResourceRef::new(
                    ResourceKind::ExternalService,
                    "legacy-control",
                    "cutover",
                    "auth-source",
                )
                .unwrap(),
            ],
        }],
        valid_from_ms: 0,
        limits: RootBudgetLimits {
            max_units: 1,
            max_concurrent: 1,
            deadline_ms: i64::MAX,
        },
    };
    coordinator
        .publish_credential_configuration(
            "legacy-publication",
            &CredentialConfiguration {
                source_id: "legacy-auth".into(),
                revision: 1,
                configuration_fingerprint: "a".repeat(64),
                credentials: vec![],
            },
            0,
            &ceiling,
            &coordinator.snapshot().await.unwrap().stamp(),
            "original publication",
            1,
        )
        .await
        .unwrap();
    let mut legacy = serde_json::to_value(coordinator.snapshot().await.unwrap()).unwrap();
    legacy["schema_version"] = 7.into();
    legacy.as_object_mut().unwrap().remove("purpose");
    let key = StateKey::new(
        "legacy-control",
        "cutover",
        KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
        "authority",
    );
    state.set(&key, &legacy.to_string(), None).await.unwrap();
    (key, legacy["incarnation"].as_str().unwrap().into())
}

struct DispatchFixture {
    prepared: acteon_server::execution_authority::PreparedExecutionScope,
    coordinator: AuthorityCoordinator,
    contexts: Arc<acteon_governance::context::TrustedContextStore>,
    binding: ScopedCredentialBinding,
    permits: Vec<acteon_governance::permit::PermitReference>,
    gateway: acteon_gateway::Gateway,
    clock: Arc<acteon_time::SystemClock>,
}
#[allow(clippy::too_many_lines)] // One complete private-authentication/permit/runtime fixture.
async fn dispatch_fixture(url: &str, rule: Option<RuleAction>, fallback: bool) -> DispatchFixture {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let factory = StaticWebhook::build(
        &toml::from_str(&format!(
            "name = 'incident'\ntype = 'webhook'\nurl = '{url}'\ninternal_hosts = ['127.0.0.1']"
        ))
        .unwrap(),
        None,
    )
    .unwrap();
    let actual = factory.provider();
    let mut registry = ExecutionProviderRegistry::default();
    registry.register(actual.clone(), Some(factory)).unwrap();
    let prepared = registry
        .prepare(&configuration(), ("auth-control", "deployment"), &[8; 32])
        .unwrap()
        .pop()
        .unwrap();
    let coordinator = AuthorityCoordinator::initialize(
        state.clone(),
        "prod",
        "acme",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap();
    let control = AuthorityCoordinator::initialize(
        state.clone(),
        "auth-control",
        "deployment",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap();
    let authority = Arc::new(
        AuthAuthority::new(
            control,
            &AuthAuthorityConfig {
                namespace: "auth-control".into(),
                tenant: "deployment".into(),
                source_id: "deployment-auth".into(),
                bootstrap: false,
            },
            SecretString::new("shared-security-fingerprint-key-32-bytes".into()),
        )
        .unwrap(),
    );
    let projector = Arc::new(prepared.projector(coordinator.clone()).await.unwrap());
    let auth = AuthFileConfig {
        authority_revision: Some(1),
        settings: AuthSettings {
            jwt_secret: SecretString::new("jwt-signing-key-at-least-32-bytes".into()),
            jwt_expiry_seconds: 3600,
        },
        users: Vec::new(),
        api_keys: vec![ApiKeyConfig {
            authority_id: Some("credential/maya".into()),
            name: "maya".into(),
            principal: Some(prepared.declaration().subjects[0].clone()),
            key_hash: SecretString::new(hash_api_key("maya-secret").into()),
            role: "executor".into(),
            grants: vec![Grant {
                namespaces: vec!["prod".into()],
                tenants: vec!["acme".into()],
                providers: vec!["incident".into()],
                actions: vec!["execute".into()],
                agent_id: None,
            }],
        }],
    };
    let binding = authenticated_binding(Arc::new(
        AuthProvider::new_with_scope_projection(&auth, state.clone(), authority, vec![projector])
            .await
            .unwrap(),
    ))
    .await;
    let effect = prepared.catalog().definitions("prod", "acme")[0]
        .effect
        .clone();
    let issuance = acteon_governance::permit::PermitIssuanceCeiling {
        issuer: prepared.declaration().publisher.clone(),
        subjects: prepared.declaration().subjects.clone(),
        effects: vec![effect.clone()],
        valid_from_ms: 0,
        limits: prepared.declaration().credential_limits.clone(),
    };
    let clock = Arc::new(acteon_time::SystemClock::default());
    coordinator
        .publish_permit(
            "issue-dispatch",
            acteon_governance::permit::ExecutionPermit {
                id: "dispatch-permit".into(),
                revision: 1,
                subject: prepared.declaration().subjects[0].clone(),
                effects: vec![effect],
                valid_from_ms: 0,
                limits: issuance.limits.clone(),
            },
            0,
            &issuance,
            &coordinator.snapshot().await.unwrap().stamp(),
            "test admission",
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .unwrap();
    let contexts = Arc::new(
        acteon_governance::context::TrustedContextStore::new(
            state.clone(),
            coordinator.clone(),
            "dispatch-test".into(),
            "k1".into(),
            vec![
                acteon_governance::context::ContextSigningKey::new("k1".into(), vec![9; 32])
                    .unwrap(),
            ],
        )
        .unwrap(),
    );
    let action = Action::new("prod", "acme", "incident", "execute", json!({}));
    let driver = acteon_executor::governed::GovernedProviderExecutor::new(
        state.clone(),
        coordinator.clone(),
        contexts.clone(),
        prepared
            .catalog()
            .resolve(&action, &actual)
            .unwrap()
            .clone(),
        acteon_executor::ExecutorConfig::default(),
        clock.clone(),
        None,
    )
    .unwrap();
    let mut builder = GatewayBuilder::new()
        .state(state)
        .lock(Arc::new(MemoryDistributedLock::new()))
        .provider(actual)
        .provider(Arc::new(LogProvider::new("log")))
        .provider_execution_mediator(Arc::new(
            GovernedProviderMediator::new(vec![driver]).unwrap(),
        ))
        .external_url("https://city.example.com");
    if let Some(rule) = rule {
        builder = builder.rules(vec![Rule::new("dispatch-policy", Expr::Bool(true), rule)]);
    }
    if fallback {
        builder = builder.circuit_breaker_provider(
            "log",
            CircuitBreakerConfig {
                failure_threshold: 1,
                fallback_provider: Some("incident".into()),
                ..CircuitBreakerConfig::default()
            },
        );
    }
    let gateway = builder.build().unwrap();
    if fallback {
        gateway
            .circuit_breakers()
            .unwrap()
            .get("log")
            .unwrap()
            .record_failure()
            .await;
    }
    DispatchFixture {
        prepared,
        coordinator,
        contexts,
        binding,
        clock,
        gateway,
        permits: vec![acteon_governance::permit::PermitReference {
            id: "dispatch-permit".into(),
            accepted_revision: 1,
        }],
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Shared real transport covers six paths and authority inheritance.
async fn private_admission_governs_real_gateway_calls_after_modification_and_routing() {
    use std::future::IntoFuture;
    let received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let capture = received.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/incident", listener.local_addr().unwrap());
    let router = Router::new().route(
        "/incident",
        axum::routing::post(move |axum::Json(payload): axum::Json<serde_json::Value>| {
            let capture = capture.clone();
            async move {
                capture.lock().unwrap().push(payload);
                axum::Json(json!({"accepted": true}))
            }
        }),
    );
    let server = tokio::spawn(axum::serve(listener, router).into_future());
    for (rule, fallback) in [
        (None, false),
        (
            Some(RuleAction::Modify {
                changes: json!({"value": 2}),
            }),
            false,
        ),
        (
            Some(RuleAction::Deduplicate {
                ttl_seconds: Some(300),
            }),
            false,
        ),
        (
            Some(RuleAction::Throttle {
                max_count: 1,
                window_seconds: 60,
            }),
            false,
        ),
        (
            Some(RuleAction::Reroute {
                target_provider: "incident".into(),
            }),
            false,
        ),
        (None, true),
    ] {
        let direct = rule.is_none() && !fallback;
        let modified = matches!(&rule, Some(RuleAction::Modify { .. }));
        let rerouted = matches!(&rule, Some(RuleAction::Reroute { .. })) || fallback;
        let f = dispatch_fixture(&url, rule, fallback).await;
        let before = received.lock().unwrap().len();
        let mut action = Action::new(
            "prod",
            "acme",
            if rerouted { "log" } else { "incident" },
            "execute",
            json!({"value": 1}),
        );
        action
            .metadata
            .labels
            .insert("principal".into(), "agent/imposter".into());
        action
            .metadata
            .labels
            .insert("credential_id".into(), "credential/maya".into());
        let unauthenticated = f.gateway.dispatch(action.clone(), None).await.unwrap();
        let acteon_core::ActionOutcome::Failed(error) = unauthenticated else {
            panic!("wire labels bypassed private admission");
        };
        assert_eq!(error.code, "EXECUTION_AUTHORITY_REQUIRED");
        assert_eq!(received.lock().unwrap().len(), before);
        assert!(f.coordinator.snapshot().await.unwrap().roots.is_empty());
        let missing_permits = f.prepared.provider_admission(
            acteon_server::execution_authority::ProviderAdmissionRequest {
                admission_key: "original-operation",
                handle: acteon_governance::context::ExecutionContextHandle::new(),
                execution_id: uuid::Uuid::new_v4(),
                authentication: &f.binding,
                permits: &[],
            },
            &f.coordinator,
            &f.contexts,
            f.clock.as_ref(),
        );
        let acteon_core::ActionOutcome::Failed(error) = f
            .gateway
            .dispatch_with_execution_admission(action.clone(), None, &missing_permits)
            .await
            .unwrap()
        else {
            panic!("missing permits consumed dispatch state or executed");
        };
        assert_eq!(error.code, "EXECUTION_ADMISSION_REFUSED");
        assert_eq!(received.lock().unwrap().len(), before);
        assert!(f.coordinator.snapshot().await.unwrap().roots.is_empty());
        let admission = f.prepared.provider_admission(
            acteon_server::execution_authority::ProviderAdmissionRequest {
                admission_key: "original-operation",
                handle: acteon_governance::context::ExecutionContextHandle::new(),
                execution_id: uuid::Uuid::new_v4(),
                authentication: &f.binding,
                permits: &f.permits,
            },
            &f.coordinator,
            &f.contexts,
            f.clock.as_ref(),
        );
        let outcome = f
            .gateway
            .dispatch_with_execution_admission(action.clone(), None, &admission)
            .await
            .unwrap();
        assert!(
            matches!(
                outcome,
                acteon_core::ActionOutcome::Executed(_)
                    | acteon_core::ActionOutcome::Rerouted { .. }
            ),
            "{outcome:?}"
        );
        assert_eq!(received.lock().unwrap().len(), before + 1);
        assert_eq!(
            received.lock().unwrap().last().unwrap()["payload"]["value"],
            if modified { 2 } else { 1 }
        );
        let replay = f
            .gateway
            .dispatch_with_execution_admission(action.clone(), None, &admission)
            .await
            .unwrap();
        assert!(matches!(
            replay,
            acteon_core::ActionOutcome::Executed(_)
                | acteon_core::ActionOutcome::Rerouted { .. }
                | acteon_core::ActionOutcome::Deduplicated
                | acteon_core::ActionOutcome::Throttled { .. }
        ));
        assert_eq!(received.lock().unwrap().len(), before + 1);
        let snapshot = f.coordinator.snapshot().await.unwrap();
        assert_eq!(snapshot.roots.len(), 1);
        assert_eq!(snapshot.starts.len(), 1);
        assert_eq!(snapshot.roots.values().next().unwrap().spent_units, 1);
        assert_eq!(snapshot.roots.values().next().unwrap().active_attempts, 0);
        if direct {
            let mut changed = action.clone();
            changed.payload = json!({"value": 99});
            let acteon_core::ActionOutcome::Failed(error) = f
                .gateway
                .dispatch_with_execution_admission(changed, None, &admission)
                .await
                .unwrap()
            else {
                panic!("operation key admitted changed work");
            };
            assert_eq!(error.code, "EXECUTION_ADMISSION_REFUSED");
            f.coordinator
                .change(
                    "stop-maya",
                    acteon_governance::AuthorityChange::RevokeCredential {
                        credential_id: "credential/maya".into(),
                        expected_revision: 1,
                    },
                    "operator",
                    "stop work",
                )
                .await
                .unwrap();
            let acteon_core::ActionOutcome::Failed(error) = f
                .gateway
                .dispatch_with_execution_admission(action, None, &admission)
                .await
                .unwrap()
            else {
                panic!("revoked authentication resumed work");
            };
            assert_eq!(error.code, "EXECUTION_ADMISSION_REFUSED");
            assert_eq!(received.lock().unwrap().len(), before + 1);
            assert_eq!(f.coordinator.snapshot().await.unwrap().roots.len(), 1);
        }
    }
    let f = dispatch_fixture(
        &url,
        Some(RuleAction::RequestApproval {
            notify_provider: "incident".into(),
            timeout_seconds: 3600,
            message: None,
        }),
        false,
    )
    .await;
    let before = received.lock().unwrap().len();
    let admission = f.prepared.provider_admission(
        acteon_server::execution_authority::ProviderAdmissionRequest {
            admission_key: "approval-operation",
            handle: acteon_governance::context::ExecutionContextHandle::new(),
            execution_id: uuid::Uuid::new_v4(),
            authentication: &f.binding,
            permits: &f.permits,
        },
        &f.coordinator,
        &f.contexts,
        f.clock.as_ref(),
    );
    let acteon_core::ActionOutcome::PendingApproval {
        approval_id,
        notification_sent,
        ..
    } = f
        .gateway
        .dispatch_with_execution_admission(
            Action::new("prod", "acme", "incident", "execute", json!({})),
            None,
            &admission,
        )
        .await
        .unwrap()
    else {
        panic!("expected approval");
    };
    assert!(!notification_sent);
    assert!(
        !f.gateway
            .retry_approval_notification("prod", "acme", &approval_id)
            .await
            .unwrap()
    );
    assert_eq!(received.lock().unwrap().len(), before);
    assert!(f.coordinator.snapshot().await.unwrap().roots.is_empty());
    server.abort();
}

#[test]
fn manager_intervention_footprints_must_fit_before_authority_publication() {
    let (registry, _) = registry();
    let mut config = configuration();
    let scope = &mut config.scopes[0];
    scope.routes = (0..128)
        .map(|i| acteon_server::config::ExecutionRouteConfig {
            provider: "incident".into(),
            action_type: format!("execute-{i}"),
        })
        .collect();
    scope
        .managers
        .push(acteon_server::config::ExecutionManagerConfig {
            principal: scope.subjects[0].clone(),
            subjects: scope.subjects.clone(),
            routes: scope.routes.clone(),
            valid_from_ms: scope.valid_from_ms,
            limits: scope.credential_limits.clone(),
            can_issue_permits: true,
            can_intervene: false,
        });
    registry
        .prepare(&config, ("auth-control", "deployment"), &[8; 32])
        .unwrap();
    config.scopes[0].managers[0].can_intervene = true;
    assert_eq!(
        registry
            .prepare(&config, ("auth-control", "deployment"), &[8; 32])
            .err()
            .as_deref(),
        Some("execution manager exceeds control footprint capacity"),
    );
    config.scopes[0].managers[0].routes.truncate(1);
    registry
        .prepare(&config, ("auth-control", "deployment"), &[8; 32])
        .unwrap();
}
