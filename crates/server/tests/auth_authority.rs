use std::sync::Arc;

use acteon_core::{PrincipalIdentity, PrincipalKind};
use acteon_governance::{
    AuthorityChange, AuthorityCoordinator, COORDINATOR_KIND, CoordinatorLimits,
};
use acteon_server::auth::AuthProvider;
use acteon_server::auth::api_key::hash_api_key;
use acteon_server::auth::authority::{AuthAuthority, AuthenticatedConfiguration};
use acteon_server::auth::config::{ApiKeyConfig, AuthFileConfig, AuthSettings, Grant, UserConfig};
use acteon_server::auth::crypto::SecretString;
use acteon_server::auth::identity::CallerIdentity;
use acteon_server::auth::middleware::AuthLayer;
use acteon_server::config::AuthAuthorityConfig;
use acteon_state::testing::faults::{FaultStore, FaultTiming, WriteOperation};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;
use axum::{Extension, Json, Router, routing::get};
use serde_json::{Value, json};

fn settings() -> AuthAuthorityConfig {
    AuthAuthorityConfig {
        namespace: "auth-control".into(),
        tenant: "deployment".into(),
        source_id: "workforce-auth".into(),
        bootstrap: false,
    }
}
fn configuration(revision: u64) -> AuthFileConfig {
    AuthFileConfig {
        authority_revision: Some(revision),
        settings: AuthSettings {
            jwt_secret: SecretString::new("jwt-test-secret-at-least-32-bytes".into()),
            jwt_expiry_seconds: 3600,
        },
        users: Vec::new(),
        api_keys: vec![ApiKeyConfig {
            authority_id: None,
            name: "maya-assistant".into(),
            principal: Some(
                PrincipalIdentity::new("agent/maya-assistant", PrincipalKind::Agent).unwrap(),
            ),
            key_hash: SecretString::new(hash_api_key("key-original").into()),
            role: "operator".into(),
            grants: vec![Grant {
                tenants: vec!["acme".into()],
                namespaces: vec!["prod".into()],
                providers: vec!["read".into(), "write".into()],
                actions: vec!["execute".into()],
                agent_id: Some("maya-assistant".into()),
            }],
        }],
    }
}
async fn coordinator(state: Arc<dyn StateStore>) -> AuthorityCoordinator {
    AuthorityCoordinator::initialize(
        state,
        "auth-control",
        "deployment",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap()
}
fn authority(c: AuthorityCoordinator) -> Arc<AuthAuthority> {
    authority_with_key(c, "shared-fingerprint-test-key-32-bytes")
}
fn authority_with_key(c: AuthorityCoordinator, key: &str) -> Arc<AuthAuthority> {
    Arc::new(AuthAuthority::new(c, &settings(), SecretString::new(key.to_owned().into())).unwrap())
}
async fn provider(
    state: Arc<dyn StateStore>,
    c: AuthorityCoordinator,
    config: &AuthFileConfig,
) -> Arc<AuthProvider> {
    Arc::new(
        AuthProvider::new_with_authority(config, state, authority(c))
            .await
            .unwrap(),
    )
}
fn router(provider: Arc<AuthProvider>) -> Router {
    Router::new()
        .route(
            "/who",
            get(
                |Extension(identity): Extension<CallerIdentity>,
                 Extension(binding): Extension<AuthenticatedConfiguration>| async move {
                    assert_eq!(identity.principal.as_ref().unwrap(), binding.principal());
                    assert_eq!(identity.id, binding.caller_id());
                    assert_eq!(identity.auth_method, binding.auth_method());
                    Json(json!({"role": identity.role, "revision": binding.reference().revision}))
                },
            ),
        )
        .layer(AuthLayer::new(Some(provider)))
}
async fn serve(provider: Arc<AuthProvider>) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}/who", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router(provider)).await.unwrap();
    });
    (address, task)
}

/// Actual sockets and middleware, with independent providers and storage clients.
#[allow(clippy::too_many_lines)] // One lifecycle contract reused by memory and independent Redis clients.
async fn replica_contract(first: Arc<dyn StateStore>, second: Arc<dyn StateStore>) {
    let c = coordinator(first.clone()).await;
    let peer = AuthorityCoordinator::connect(second.clone(), "auth-control", "deployment")
        .await
        .unwrap();
    let old = configuration(1);
    let a = provider(first.clone(), c.clone(), &old).await;
    let b = provider(second, peer, &old).await;
    let (url_a, task_a) = serve(a.clone()).await;
    let (url_b, task_b) = serve(b.clone()).await;
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .get(&url_a)
            .bearer_auth("key-original")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let mut current = configuration(2);
    current.api_keys[0].role = "executor".into();
    current.api_keys[0].grants[0].providers = vec!["read".into()];
    current.api_keys[0].key_hash = SecretString::new(hash_api_key("key-rotated").into());
    b.reload(&current).await.unwrap();
    assert_eq!(
        client
            .get(&url_a)
            .bearer_auth("key-original")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client
            .get(&url_b)
            .bearer_auth("key-original")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let response = client
        .get(&url_b)
        .bearer_auth("key-rotated")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"role":"executor", "revision":2})
    );
    assert!(a.reload(&old).await.is_err());
    assert!(
        AuthProvider::new_with_authority(&old, first.clone(), authority(c.clone()))
            .await
            .is_err()
    );
    a.reload(&current).await.unwrap();
    let caller = a.authenticate_api_key("key-rotated").await.unwrap();
    assert!(!caller.is_authorized("acme", "prod", "write", "execute"));
    assert_eq!(
        client
            .get(&url_a)
            .header("x-api-key", "key-rotated")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    c.change(
        "disable-agent",
        AuthorityChange::RevokeSubject {
            subject: "agent/maya-assistant".into(),
        },
        "operator",
        "offboard",
    )
    .await
    .unwrap();
    assert_eq!(
        client
            .get(&url_a)
            .bearer_auth("key-rotated")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client
            .get(&url_b)
            .bearer_auth("key-rotated")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    task_a.abort();
    task_b.abort();
    first
        .delete(&StateKey::new(
            "auth-control",
            "deployment",
            KeyKind::Custom(COORDINATOR_KIND.into()),
            "authority",
        ))
        .await
        .unwrap();
}
#[tokio::test]
async fn real_http_replicas_refuse_obsolete_authentication_and_disabled_principals() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    replica_contract(state.clone(), state).await;
}
#[cfg(feature = "redis")]
#[tokio::test]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; explicitly run against isolated Redis prefix"]
async fn independent_redis_authentication_replicas_pass_the_contract() {
    use acteon_state_redis::{RedisConfig, RedisStateStore};
    let cfg = RedisConfig {
        url: std::env::var("ACTEON_GOVERNANCE_REDIS_URL").unwrap(),
        prefix: format!("server-auth-authority-{}", uuid::Uuid::new_v4()),
        ..Default::default()
    };
    let a: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&cfg).unwrap());
    let b: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&cfg).unwrap());
    replica_contract(a, b).await;
}
#[tokio::test]
async fn canonical_configuration_is_stable_but_security_changes_conflict() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let c = coordinator(state.clone()).await;
    let mut cfg = configuration(1);
    provider(state.clone(), c.clone(), &cfg).await;
    let generation = c.snapshot().await.unwrap().generation;
    cfg.api_keys[0].grants[0].providers.reverse();
    let duplicate = cfg.api_keys[0].grants[0].clone();
    cfg.api_keys[0].grants.push(duplicate);
    cfg.api_keys[0].role = "OPERATOR".into();
    provider(state.clone(), c.clone(), &cfg).await;
    assert_eq!(c.snapshot().await.unwrap().generation, generation);
    cfg.api_keys[0].role = "executor".into();
    assert!(
        AuthProvider::new_with_authority(&cfg, state.clone(), authority(c.clone()))
            .await
            .is_err()
    );
    cfg.api_keys[0].role = "operator".into();
    cfg.api_keys[0].grants[0].agent_id = Some("different-agent".into());
    assert!(
        AuthProvider::new_with_authority(&cfg, state.clone(), authority(c.clone()))
            .await
            .is_err()
    );
    let raw = state
        .get(&StateKey::new(
            "auth-control",
            "deployment",
            KeyKind::Custom(COORDINATOR_KIND.into()),
            "authority",
        ))
        .await
        .unwrap()
        .unwrap();
    for secret in [
        hash_api_key("key-original"),
        "jwt-test-secret-at-least-32-bytes".into(),
        "maya-assistant".into(),
    ] {
        assert!(!raw.contains(&secret));
    }
}
#[tokio::test]
async fn fingerprint_key_and_security_settings_are_bound() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let c = coordinator(state.clone()).await;
    let mut cfg = configuration(1);
    let a = provider(state.clone(), c.clone(), &cfg).await;
    assert!(
        AuthProvider::new_with_authority(
            &cfg,
            state,
            authority_with_key(c, "different-shared-fingerprint-key-32-bytes")
        )
        .await
        .is_err()
    );
    cfg.authority_revision = Some(2);
    cfg.settings.jwt_expiry_seconds = 7200;
    assert!(a.reload(&cfg).await.is_err());
    assert!(a.authenticate_api_key("key-original").await.is_some());
}
#[tokio::test]
async fn lost_reload_acknowledgment_preserves_safe_tables_and_reconciles() {
    for timing in [FaultTiming::Before, FaultTiming::After] {
        let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
        let state: Arc<dyn StateStore> = faults.clone();
        let c = coordinator(state.clone()).await;
        let a = provider(state, c.clone(), &configuration(1)).await;
        let mut cfg = configuration(2);
        cfg.api_keys[0].role = "executor".into();
        faults
            .fail_next(
                KeyKind::Custom(COORDINATOR_KIND.into()),
                WriteOperation::CompareAndSwap,
                timing,
            )
            .unwrap();
        assert!(a.reload(&cfg).await.is_err());
        assert_eq!(
            a.authenticate_api_key("key-original").await.is_some(),
            timing == FaultTiming::Before
        );
        a.reload(&cfg).await.unwrap();
        assert_eq!(
            a.authenticate_api_key("key-original")
                .await
                .unwrap()
                .role
                .to_string(),
            "executor"
        );
        assert_eq!(c.snapshot().await.unwrap().changes.len(), 2);
    }
}
#[tokio::test]
async fn missing_or_recreated_authority_cannot_refresh_old_tables() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let c = coordinator(state.clone()).await;
    let cfg = configuration(1);
    let a = provider(state.clone(), c, &cfg).await;
    let key = StateKey::new(
        "auth-control",
        "deployment",
        KeyKind::Custom(COORDINATOR_KIND.into()),
        "authority",
    );
    state.delete(&key).await.unwrap();
    assert!(a.authenticate_api_key("key-original").await.is_none());
    assert!(
        AuthorityCoordinator::connect(state.clone(), "auth-control", "deployment")
            .await
            .is_err()
    );
    let recreated = coordinator(state.clone()).await;
    let b = provider(state, recreated, &cfg).await;
    assert!(b.authenticate_api_key("key-original").await.is_some());
    assert!(a.authenticate_api_key("key-original").await.is_none());
}
#[tokio::test]
async fn invalid_or_unbound_configuration_cannot_become_authoritative() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let c = coordinator(state.clone()).await;
    let mut cfg = configuration(1);
    cfg.api_keys[0].principal = None;
    assert!(
        AuthProvider::new_with_authority(&cfg, state.clone(), authority(c.clone()))
            .await
            .is_err()
    );
    cfg = configuration(1);
    cfg.authority_revision = None;
    assert!(
        AuthProvider::new_with_authority(&cfg, state.clone(), authority(c.clone()))
            .await
            .is_err()
    );
    assert!(
        c.snapshot()
            .await
            .unwrap()
            .credential_configurations
            .is_empty()
    );
    let other = AuthorityCoordinator::initialize(
        state.clone(),
        "other-control",
        "deployment",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap();
    assert!(
        AuthProvider::new_with_authority(&configuration(1), state, authority(other))
            .await
            .is_err()
    );
}
#[tokio::test]
async fn jwt_sessions_refresh_only_from_current_tables_and_stale_replica_login_is_denied() {
    use argon2::{Argon2, PasswordHasher, password_hash::SaltString};
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let c = coordinator(state.clone()).await;
    let mut cfg = configuration(1);
    let salt = SaltString::encode_b64(b"fixed-test-salt").unwrap();
    let password_hash = Argon2::default()
        .hash_password(b"password", &salt)
        .unwrap()
        .to_string();
    cfg.users.push(UserConfig {
        authority_id: None,
        username: "maya".into(),
        principal: Some(PrincipalIdentity::new("human/maya", PrincipalKind::Human).unwrap()),
        password_hash: SecretString::new(password_hash.into()),
        role: "operator".into(),
        grants: cfg.api_keys[0].grants.clone(),
    });
    let a = provider(state.clone(), c.clone(), &cfg).await;
    let b = provider(state, c, &cfg).await;
    let (token, _) = a.login("maya", "password").await.unwrap();
    cfg.authority_revision = Some(2);
    cfg.users[0].role = "viewer".into();
    b.reload(&cfg).await.unwrap();
    assert!(a.validate_jwt(&token).await.is_err());
    assert!(a.login("maya", "password").await.is_err());
    assert_eq!(
        b.validate_jwt(&token).await.unwrap().role.to_string(),
        "viewer"
    );
    a.reload(&cfg).await.unwrap();
    assert_eq!(
        a.validate_jwt(&token).await.unwrap().role.to_string(),
        "viewer"
    );
}

#[tokio::test]
async fn credential_enrollment_is_resolved_from_the_actual_key() {
    use acteon_server::auth::enrollment::AuthenticatedCredential;
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let mut cfg = configuration(1);
    cfg.api_keys[0].authority_id = Some("credential/inspect".into());
    let mut second = configuration(1).api_keys.remove(0);
    second.authority_id = Some("credential/repair".into());
    second.name = cfg.api_keys[0].name.clone(); // Display names cannot select authority.
    second.key_hash = SecretString::new(hash_api_key("key-repair").into());
    second.role = "viewer".into();
    cfg.api_keys.push(second);
    let auth = Arc::new(AuthProvider::new(&cfg, state).unwrap());
    let app = Router::new()
        .route(
            "/binding",
            get(
                |Extension(identity): Extension<CallerIdentity>,
                 Extension(binding): Extension<AuthenticatedCredential>| async move {
                    assert_eq!(identity.principal.as_ref().unwrap(), binding.principal());
                    assert_eq!(identity.auth_method, binding.auth_method());
                    Json(json!({"authority_id": binding.id(), "role": identity.role}))
                },
            ),
        )
        .layer(AuthLayer::new(Some(auth)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/binding", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::new();
    for (key, id, role) in [
        ("key-original", "credential/inspect", "operator"),
        ("key-repair", "credential/repair", "viewer"),
    ] {
        let response = client
            .get(&url)
            .bearer_auth(key)
            .header("x-authority-id", "credential/forged")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({"authority_id": id, "role": role})
        );
    }
    assert_eq!(
        client
            .get(&url)
            .bearer_auth("credential/inspect")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    task.abort();
}

#[tokio::test]
async fn rotating_enrollment_requires_one_complete_policy_and_reload_is_atomic() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let mut cfg = configuration(1);
    cfg.api_keys[0].authority_id = Some("credential/diagnostics".into());
    let mut rotated = configuration(1).api_keys.remove(0);
    rotated.authority_id = cfg.api_keys[0].authority_id.clone();
    rotated.name = "rotated".into();
    rotated.key_hash = SecretString::new(hash_api_key("key-rotated").into());
    rotated.grants[0].providers.reverse();
    cfg.api_keys.push(rotated);
    let auth = AuthProvider::new(&cfg, state).unwrap();
    assert!(auth.authenticate_api_key("key-original").await.is_some());
    assert!(auth.authenticate_api_key("key-rotated").await.is_some());
    cfg.api_keys[1].grants[0].agent_id = Some("other-bus-identity".into());
    assert!(auth.reload(&cfg).await.is_err());
    assert!(auth.authenticate_api_key("key-original").await.is_some());
    cfg.api_keys[1].grants[0].agent_id = cfg.api_keys[0].grants[0].agent_id.clone();
    cfg.api_keys[1].role = "viewer".into();
    assert!(auth.reload(&cfg).await.is_err());
    cfg.api_keys[1].role = "operator".into();
    cfg.api_keys.remove(0);
    auth.reload(&cfg).await.unwrap();
    assert!(auth.authenticate_api_key("key-original").await.is_none());
    assert!(auth.authenticate_api_key("key-rotated").await.is_some());
    cfg.api_keys[0].principal = None;
    assert!(auth.reload(&cfg).await.is_err());
    assert!(auth.authenticate_api_key("key-rotated").await.is_some());
}

#[tokio::test]
async fn jwt_enrollment_cannot_silently_switch_to_a_replacement() {
    use argon2::{Argon2, PasswordHasher, password_hash::SaltString};
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let c = coordinator(state.clone()).await;
    let mut cfg = configuration(1);
    let salt = SaltString::encode_b64(b"fixed-test-salt").unwrap();
    let hash = Argon2::default()
        .hash_password(b"password", &salt)
        .unwrap()
        .to_string();
    cfg.users.push(UserConfig {
        authority_id: None,
        username: "maya".into(),
        principal: Some(PrincipalIdentity::new("human/maya", PrincipalKind::Human).unwrap()),
        password_hash: SecretString::new(hash.into()),
        role: "operator".into(),
        grants: cfg.api_keys[0].grants.clone(),
    });
    let auth = provider(state, c, &cfg).await;
    let (legacy, _) = auth.login("maya", "password").await.unwrap();
    cfg.users[0].authority_id = Some("credential/maya-v1".into());
    assert!(auth.reload(&cfg).await.is_err()); // ID is part of the shared security epoch.
    assert!(auth.validate_jwt(&legacy).await.is_ok());
    cfg.authority_revision = Some(2);
    auth.reload(&cfg).await.unwrap();
    assert!(auth.validate_jwt(&legacy).await.is_err());
    let (enrolled, _) = auth.login("maya", "password").await.unwrap();
    cfg.authority_revision = Some(3);
    cfg.users[0].role = "viewer".into();
    auth.reload(&cfg).await.unwrap();
    assert_eq!(
        auth.validate_jwt(&enrolled).await.unwrap().role.to_string(),
        "viewer"
    );
    cfg.authority_revision = Some(4);
    cfg.users[0].authority_id = Some("credential/maya-v2".into());
    auth.reload(&cfg).await.unwrap();
    assert!(auth.validate_jwt(&enrolled).await.is_err());
    let (replacement, _) = auth.login("maya", "password").await.unwrap();
    assert!(auth.validate_jwt(&replacement).await.is_ok());
}

mod server_process {
    use super::*;
    #[cfg(feature = "redis")]
    use acteon_state_redis::{RedisConfig, RedisStateStore};
    use std::{
        fs,
        path::PathBuf,
        process::{Child, Command, Stdio},
        time::Duration,
    };

    struct Server {
        child: Child,
        url: String,
        directory: PathBuf,
    }
    impl Server {
        fn start(
            state_config: &str,
            bootstrap: bool,
            revision: u64,
            role: &str,
            raw_key: &str,
        ) -> Self {
            let directory =
                std::env::temp_dir().join(format!("acteon-auth-process-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&directory).unwrap();
            let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = socket.local_addr().unwrap().port();
            drop(socket);
            let config = format!(
                r#"
[server]
host = "127.0.0.1"
port = {port}
[state]
{state_config}
[ui]
enabled = false
[auth]
enabled = true
config_path = "auth.toml"
watch = true
[auth.authority]
namespace = "auth-control"
tenant = "deployment"
source_id = "workforce-auth"
bootstrap = {bootstrap}
"#
            );
            fs::write(directory.join("acteon.toml"), config).unwrap();
            Self::write_auth(&directory, revision, role, raw_key);
            let log = fs::File::create(directory.join("server.log")).unwrap();
            let child = Command::new(env!("CARGO_BIN_EXE_acteon-server"))
                .arg("-c")
                .arg(directory.join("acteon.toml"))
                .env("ACTEON_AUTH_KEY", "11".repeat(32))
                .env(
                    "ACTEON_AUTH_AUTHORITY_KEY",
                    "shared-fingerprint-test-key-32-bytes",
                )
                .env("RUST_LOG", "acteon_server=info")
                .stdout(Stdio::from(log.try_clone().unwrap()))
                .stderr(Stdio::from(log))
                .spawn()
                .unwrap();
            Self {
                child,
                directory,
                url: format!("http://127.0.0.1:{port}/v1/auth/identity"),
            }
        }
        fn write_auth(directory: &std::path::Path, revision: u64, role: &str, raw_key: &str) {
            let config = format!(
                r#"
authority_revision = {revision}
[settings]
jwt_secret = "jwt-test-secret-at-least-32-bytes"
[[api_keys]]
name = "maya-assistant"
key_hash = {key_hash:?}
role = {role:?}
principal = {{ id = "agent/maya-assistant", kind = "agent" }}
[[api_keys.grants]]
tenants = ["acme"]
namespaces = ["prod"]
providers = ["read"]
actions = ["execute"]
"#,
                key_hash = hash_api_key(raw_key)
            );
            // Atomic replacement exercises the production parent-directory watcher.
            fs::write(directory.join("auth.new"), config).unwrap();
            fs::rename(directory.join("auth.new"), directory.join("auth.toml")).unwrap();
        }
        async fn wait_role(&mut self, client: &reqwest::Client, key: &str, role: &str) {
            tokio::time::timeout(Duration::from_secs(15), async {
                loop {
                    assert!(self.child.try_wait().unwrap().is_none(), "{}", self.log());
                    if let Ok(response) = client.get(&self.url).bearer_auth(key).send().await
                        && response.status() == 200
                    {
                        let body: Value = response.json().await.unwrap();
                        if body["role"] == role {
                            break;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .unwrap_or_else(|_| panic!("server did not converge: {}", self.log()));
        }
        fn log(&self) -> String {
            fs::read_to_string(self.directory.join("server.log")).unwrap()
        }
        async fn wait_refused(&mut self) {
            let status = tokio::time::timeout(Duration::from_secs(15), async {
                loop {
                    if let Some(status) = self.child.try_wait().unwrap() {
                        break status;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .unwrap_or_else(|_| panic!("startup did not refuse: {}", self.log()));
            assert!(!status.success(), "{}", self.log());
        }
    }
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    #[tokio::test]
    async fn real_memory_server_uses_the_configured_state_backend() {
        let client = reqwest::Client::new();
        let mut missing =
            Server::start("backend = \"memory\"", false, 1, "operator", "key-original");
        missing.wait_refused().await;
        drop(missing);
        let mut server = Server::start("backend = \"memory\"", true, 1, "operator", "key-original");
        server.wait_role(&client, "key-original", "operator").await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while !server.log().contains("auth watcher started") {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        Server::write_auth(&server.directory, 2, "executor", "key-rotated");
        server.wait_role(&client, "key-rotated", "executor").await;
        assert_eq!(
            client
                .get(&server.url)
                .bearer_auth("key-original")
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
    }

    #[cfg(feature = "redis")]
    #[tokio::test]
    #[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; real production startup and watcher"]
    async fn real_server_startup_and_watcher_refuse_stale_authentication() {
        let cfg = RedisConfig {
            url: std::env::var("ACTEON_GOVERNANCE_REDIS_URL").unwrap(),
            prefix: format!("server-auth-main-{}", uuid::Uuid::new_v4()),
            ..Default::default()
        };
        let state = RedisStateStore::new(&cfg).unwrap();
        let state_config = format!(
            "backend = \"redis\"\nurl = {:?}\nprefix = {:?}",
            cfg.url, cfg.prefix
        );
        production_contract(&state_config, &state).await;
    }

    #[cfg(any(feature = "redis", feature = "postgres", feature = "dynamodb"))]
    pub(super) async fn production_contract(state_config: &str, state: &dyn StateStore) {
        let client = reqwest::Client::new();
        // Normal startup cannot implicitly recreate a missing authority record.
        let mut missing = Server::start(state_config, false, 1, "operator", "key-original");
        missing.wait_refused().await;
        assert!(
            missing.log().contains("refusing recreation"),
            "{}",
            missing.log()
        );
        drop(missing);
        let mut a = Server::start(state_config, true, 1, "operator", "key-original");
        a.wait_role(&client, "key-original", "operator").await;
        let mut b = Server::start(state_config, false, 1, "operator", "key-original");
        b.wait_role(&client, "key-original", "operator").await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while !b.log().contains("auth watcher started") {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        Server::write_auth(&b.directory, 2, "executor", "key-rotated");
        b.wait_role(&client, "key-rotated", "executor").await;
        assert_eq!(
            client
                .get(&a.url)
                .bearer_auth("key-original")
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        assert_eq!(
            client
                .get(&b.url)
                .bearer_auth("key-original")
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        drop(a);
        let mut obsolete = Server::start(state_config, false, 1, "operator", "key-original");
        obsolete.wait_refused().await;
        assert!(
            obsolete.log().contains("publication refused"),
            "{}",
            obsolete.log()
        );
        drop(obsolete);
        let mut recovered = Server::start(state_config, false, 2, "executor", "key-rotated");
        recovered
            .wait_role(&client, "key-rotated", "executor")
            .await;
        drop(recovered);
        drop(b);
        state
            .delete(&StateKey::new(
                "auth-control",
                "deployment",
                KeyKind::Custom(COORDINATOR_KIND.into()),
                "authority",
            ))
            .await
            .unwrap();
    }
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires DATABASE_URL; independent configured PostgreSQL clients"]
async fn independent_postgres_authentication_replicas_pass_the_contract() {
    use acteon_state_postgres::{PostgresConfig, PostgresStateStore};
    let config = PostgresConfig {
        url: std::env::var("DATABASE_URL").unwrap(),
        table_prefix: format!("auth_guard_{}_", uuid::Uuid::new_v4().simple()),
        ..Default::default()
    };
    let a: Arc<dyn StateStore> = Arc::new(PostgresStateStore::new(config.clone()).await.unwrap());
    let b: Arc<dyn StateStore> = Arc::new(PostgresStateStore::new(config.clone()).await.unwrap());
    replica_contract(a.clone(), b).await;
    let state_config = format!(
        "backend = \"postgres\"\nurl = {:?}\nprefix = {:?}",
        config.url, config.table_prefix
    );
    server_process::production_contract(&state_config, a.as_ref()).await;
    drop(a);
    // Table prefix was allocated by this fixture; no shared/live tables or rows.
    let pool = sqlx::PgPool::connect(&config.url).await.unwrap();
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
#[ignore = "requires DYNAMODB_ENDPOINT; independent configured DynamoDB Local clients"]
async fn independent_dynamodb_authentication_replicas_pass_the_contract() {
    use acteon_state_dynamodb::{DynamoConfig, DynamoStateStore, build_client, create_table};
    let config = DynamoConfig {
        endpoint_url: Some(std::env::var("DYNAMODB_ENDPOINT").unwrap()),
        table_name: format!("auth_guard_{}", uuid::Uuid::new_v4().simple()),
        key_prefix: format!("auth_guard_{}", uuid::Uuid::new_v4().simple()),
        ..Default::default()
    };
    let client = build_client(&config).await;
    create_table(&client, &config.table_name).await.unwrap();
    let a: Arc<dyn StateStore> = Arc::new(DynamoStateStore::new(&config).await.unwrap());
    let b: Arc<dyn StateStore> = Arc::new(DynamoStateStore::new(&config).await.unwrap());
    replica_contract(a.clone(), b).await;
    let state_config = format!(
        "backend = \"dynamodb\"\nurl = {:?}\ntable_name = {:?}\nprefix = {:?}\nregion = {:?}",
        config.endpoint_url.as_ref().unwrap(),
        config.table_name,
        config.key_prefix,
        config.region
    );
    server_process::production_contract(&state_config, a.as_ref()).await;
    drop(a);
    client
        .delete_table()
        .table_name(&config.table_name)
        .send()
        .await
        .unwrap();
}

#[tokio::test]
async fn invalid_and_cross_method_enrollment_ids_are_refused() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let mut cfg = configuration(1);
    for id in ["", "*", " credential/one", "credential/one\n"] {
        cfg.api_keys[0].authority_id = Some(id.into());
        assert!(AuthProvider::new(&cfg, state.clone()).is_err());
    }
    cfg.api_keys[0].authority_id = Some("credential/one".into());
    cfg.users.push(UserConfig {
        authority_id: Some("credential/one".into()),
        username: "same-actor".into(),
        principal: cfg.api_keys[0].principal.clone(),
        password_hash: SecretString::new("unused-hash".into()),
        role: cfg.api_keys[0].role.clone(),
        grants: cfg.api_keys[0].grants.clone(),
    });
    assert!(AuthProvider::new(&cfg, state.clone()).is_err());
    cfg.users[0].authority_id = Some("credential/user-one".into());
    assert!(AuthProvider::new(&cfg, state).is_ok());
}

async fn projection_fixture() -> (
    acteon_server::auth::projection::CredentialPolicyProjector,
    AuthorityCoordinator,
    Arc<AuthAuthority>,
    AuthFileConfig,
) {
    projection_fixture_on(Arc::new(MemoryStateStore::new())).await
}

async fn projection_fixture_on(
    state: Arc<dyn StateStore>,
) -> (
    acteon_server::auth::projection::CredentialPolicyProjector,
    AuthorityCoordinator,
    Arc<AuthAuthority>,
    AuthFileConfig,
) {
    projection_fixture_for(state, "prod").await
}

async fn projection_fixture_for(
    state: Arc<dyn StateStore>,
    namespace: &str,
) -> (
    acteon_server::auth::projection::CredentialPolicyProjector,
    AuthorityCoordinator,
    Arc<AuthAuthority>,
    AuthFileConfig,
) {
    use acteon_core::{ResourceKind, ResourceRef};
    use acteon_executor::{catalog::QualifiedProviderCatalog, governed::BoundProvider};
    use acteon_governance::{RootBudgetLimits, permit::PermitIssuanceCeiling};
    let auth = authority(coordinator(state.clone()).await);
    let scope =
        AuthorityCoordinator::initialize(state, namespace, "acme", CoordinatorLimits::default())
            .await
            .unwrap();
    let mut cfg = configuration(1);
    cfg.api_keys[0].authority_id = Some("credential/diagnostics".into());
    cfg.api_keys[0].grants[0].namespaces = vec!["prod".into(), "secondary".into()];
    let bindings = ["read", "write"]
        .into_iter()
        .map(|name| {
            let provider: Arc<dyn acteon_provider::DynProvider> =
                Arc::new(acteon_provider::LogProvider::new(name));
            BoundProvider::new_trusted(
                provider,
                &ResourceRef::new(ResourceKind::Endpoint, namespace, "acme", name).unwrap(),
                "execute",
                "opaque-version-v1",
                vec![],
            )
            .unwrap()
        })
        .collect();
    let catalog = QualifiedProviderCatalog::new_trusted(bindings).unwrap();
    let effects = catalog
        .definitions(namespace, "acme")
        .into_iter()
        .map(|d| d.effect)
        .collect();
    let limits = RootBudgetLimits {
        max_units: 5,
        max_concurrent: 2,
        deadline_ms: 4_102_444_800_000,
    };
    let issuance = PermitIssuanceCeiling {
        issuer: PrincipalIdentity::new("scope-publisher", PrincipalKind::System).unwrap(),
        subjects: vec![cfg.api_keys[0].principal.clone().unwrap()],
        effects,
        valid_from_ms: 0,
        limits: limits.clone(),
    };
    let projector = acteon_server::auth::projection::CredentialPolicyProjector::new_trusted(
        scope.clone(),
        catalog,
        issuance,
        0,
        limits,
    )
    .await
    .unwrap();
    (projector, scope, auth, cfg)
}

#[tokio::test]
async fn projection_isolates_unrelated_scope_actors_without_enlarging_issuance() {
    let (projector, scope, auth, mut cfg) = projection_fixture().await;
    let mut unrelated = configuration(1).api_keys.remove(0);
    unrelated.authority_id = Some("credential/other-team".into());
    unrelated.name = "other-team".into();
    unrelated.key_hash = SecretString::new(hash_api_key("other-team-key").into());
    unrelated.principal =
        Some(PrincipalIdentity::new("other-team-agent", PrincipalKind::Agent).unwrap());
    unrelated.grants[0].namespaces = vec!["secondary".into()];
    cfg.api_keys.push(unrelated);

    let projected = projector.project(&auth, &cfg).unwrap();
    assert_eq!(projected.credentials.len(), 1);
    projector.publish(&auth, &cfg, 1).await.unwrap();
    let before = serde_json::to_value(scope.snapshot().await.unwrap()).unwrap();
    assert!(before["credentials"].get("credential/other-team").is_none());

    // A matching grant cannot turn the auth file into an issuance whitelist.
    cfg.authority_revision = Some(2);
    cfg.api_keys[1].grants[0].namespaces = vec!["prod".into()];
    assert!(projector.project(&auth, &cfg).is_err());
    assert!(projector.publish(&auth, &cfg, 2).await.is_err());
    assert_eq!(
        serde_json::to_value(scope.snapshot().await.unwrap()).unwrap(),
        before
    );
}

#[tokio::test]
async fn qualified_projection_keeps_credentials_separate_and_retires_omissions() {
    let (projector, scope, auth, mut cfg) = projection_fixture().await;
    let mut read_only = configuration(1).api_keys.remove(0);
    read_only.authority_id = Some("credential/reader".into());
    read_only.key_hash = SecretString::new(hash_api_key("reader-key").into());
    read_only.grants[0].providers = vec!["read".into()];
    cfg.api_keys.push(read_only);
    let mut viewer = configuration(1).api_keys.remove(0);
    viewer.authority_id = Some("credential/viewer".into());
    viewer.key_hash = SecretString::new(hash_api_key("viewer-key").into());
    viewer.role = "viewer".into();
    cfg.api_keys.push(viewer);
    let reference = projector.publish(&auth, &cfg, 1).await.unwrap();
    scope
        .verify_credential_configuration(&reference)
        .await
        .unwrap();
    let snapshot = scope.snapshot().await.unwrap();
    assert_eq!(
        snapshot.credentials["credential/diagnostics"]
            .authority
            .ceiling
            .effects
            .len(),
        2
    );
    assert_eq!(
        snapshot.credentials["credential/reader"]
            .authority
            .ceiling
            .effects
            .len(),
        1
    );
    assert!(
        !snapshot.credentials["credential/viewer"]
            .authority
            .execution_enabled
    );
    assert!(
        snapshot.credentials["credential/viewer"]
            .authority
            .ceiling
            .effects
            .is_empty()
    );
    assert_eq!(projector.publish(&auth, &cfg, 2).await.unwrap(), reference);
    assert_eq!(
        scope.snapshot().await.unwrap().generation,
        snapshot.generation
    );
    cfg.api_keys[0].grants.clear();
    assert!(projector.publish(&auth, &cfg, 3).await.is_err());
    assert_eq!(
        scope.snapshot().await.unwrap().generation,
        snapshot.generation
    );
    cfg.api_keys.remove(0);
    cfg.authority_revision = Some(2);
    projector.publish(&auth, &cfg, 4).await.unwrap();
    assert!(
        scope
            .verify_credential_configuration(&reference)
            .await
            .is_err()
    );
    assert!(scope.snapshot().await.unwrap().credentials["credential/diagnostics"].revoked);
    let mut restored = configuration(3).api_keys.remove(0);
    restored.authority_id = Some("credential/diagnostics".into());
    cfg.api_keys.push(restored);
    cfg.authority_revision = Some(3);
    assert!(projector.publish(&auth, &cfg, 5).await.is_err());
    cfg.api_keys.last_mut().unwrap().authority_id = Some("credential/diagnostics-v2".into());
    projector.publish(&auth, &cfg, 6).await.unwrap();
    assert!(!scope.snapshot().await.unwrap().credentials["credential/diagnostics-v2"].revoked);
}

#[tokio::test]
async fn scope_projection_refuses_unenrolled_and_ambiguous_authentication_inputs() {
    let (projector, scope, auth, mut cfg) = projection_fixture().await;
    let initial = scope.snapshot().await.unwrap().generation;
    cfg.api_keys[0].authority_id = None;
    assert!(projector.publish(&auth, &cfg, 1).await.is_err());
    assert_eq!(scope.snapshot().await.unwrap().generation, initial);
    cfg.api_keys[0].authority_id = Some("credential/diagnostics".into());
    let mut duplicate = configuration(1).api_keys.remove(0);
    duplicate.authority_id = Some("credential/other".into());
    cfg.api_keys.push(duplicate); // The same hash cannot resolve two logical credentials.
    assert!(projector.publish(&auth, &cfg, 1).await.is_err());
    assert_eq!(scope.snapshot().await.unwrap().generation, initial);
}

#[tokio::test]
async fn real_http_scope_bindings_refuse_partial_publication_and_reconcile_without_refresh() {
    use acteon_server::auth::projection::AuthenticatedExecutionConfiguration;
    use std::sync::Mutex;
    let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
    let state: Arc<dyn StateStore> = faults.clone();
    let (projector, scope, authority, mut cfg) = projection_fixture_on(state.clone()).await;
    let projector = Arc::new(projector);
    let a = Arc::new(
        AuthProvider::new_with_scope_projection(
            &cfg,
            state.clone(),
            authority.clone(),
            vec![projector.clone()],
        )
        .await
        .unwrap(),
    );
    let b = Arc::new(
        AuthProvider::new_with_scope_projection(&cfg, state, authority.clone(), vec![projector])
            .await
            .unwrap(),
    );
    // A node omitting the declared projection cannot join the same auth epoch.
    assert!(
        AuthProvider::new_with_authority(&cfg, faults.clone(), authority)
            .await
            .is_err()
    );
    let saved = Arc::new(Mutex::new(None::<AuthenticatedExecutionConfiguration>));
    let mut servers = Vec::new();
    let mut urls = Vec::new();
    for provider in [a.clone(), b.clone()] {
        let scope = scope.clone();
        let saved = saved.clone();
        let app = Router::new().route("/scope", get(
            move |Extension(proof): Extension<AuthenticatedExecutionConfiguration>| {
                let scope = scope.clone();
                let saved = saved.clone();
                async move {
                    let binding = proof.scope("prod", "acme").unwrap();
                    *saved.lock().unwrap() = Some(proof);
                    if binding.verify_current(&scope, chrono::Utc::now().timestamp_millis()).await.is_err() {
                        return (axum::http::StatusCode::FORBIDDEN, Json(json!({"eligible": false})));
                    }
                    (axum::http::StatusCode::OK, Json(json!({
                        "scope_revision": binding.configuration_reference().revision,
                        "auth_revision": binding.authentication_source().reference().revision,
                        "credential": binding.credential_reference().id,
                    })))
                }
            }
        )).layer(AuthLayer::new(Some(provider)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        urls.push(format!("http://{}/scope", listener.local_addr().unwrap()));
        servers.push(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));
    }
    let client = reqwest::Client::new();
    let old = client
        .get(&urls[0])
        .bearer_auth("key-original")
        .header("x-scope-revision", "999")
        .send()
        .await
        .unwrap();
    assert_eq!(old.status(), 200);
    assert_eq!(
        old.json::<Value>().await.unwrap(),
        json!({
            "scope_revision": 1, "auth_revision": 1, "credential": "credential/diagnostics",
        })
    );
    let captured = saved
        .lock()
        .unwrap()
        .clone()
        .unwrap()
        .scope("prod", "acme")
        .unwrap();
    assert!(
        captured
            .verify_current(&scope, 4_102_444_800_000)
            .await
            .is_err()
    );
    cfg.authority_revision = Some(2);
    cfg.api_keys[0].grants[0].providers = vec!["read".into()];
    faults
        .fail_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .unwrap();
    assert!(a.reload(&cfg).await.is_err());
    // Scope commit succeeded but the auth epoch/table swap was not acknowledged.
    assert!(a.authenticate_api_key("key-original").await.is_some());
    assert_eq!(
        client
            .get(&urls[0])
            .bearer_auth("key-original")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert!(captured.verify_current(&scope, 1).await.is_err());
    a.reload(&cfg).await.unwrap();
    let current = client
        .get(&urls[0])
        .bearer_auth("key-original")
        .send()
        .await
        .unwrap();
    assert_eq!(current.status(), 200);
    assert_eq!(current.json::<Value>().await.unwrap()["scope_revision"], 2);
    assert_eq!(
        client
            .get(&urls[1])
            .bearer_auth("key-original")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let generation = scope.snapshot().await.unwrap().generation;
    b.reload(&cfg).await.unwrap();
    assert_eq!(scope.snapshot().await.unwrap().generation, generation);
    assert_eq!(
        client
            .get(&urls[1])
            .bearer_auth("key-original")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert!(captured.verify_current(&scope, 1).await.is_err());
    for task in servers {
        task.abort();
    }
}

#[tokio::test]
async fn partial_multi_scope_publication_is_restrictive_and_identical_retry_converges() {
    use acteon_server::auth::projection::AuthenticatedExecutionConfiguration;
    let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
    let state: Arc<dyn StateStore> = faults.clone();
    let (first, scope_a, authority, mut cfg) = projection_fixture_for(state.clone(), "prod").await;
    let (second, scope_b, _, _) = projection_fixture_for(state.clone(), "secondary").await;
    let provider = Arc::new(
        AuthProvider::new_with_scope_projection(
            &cfg,
            state,
            authority,
            vec![Arc::new(first), Arc::new(second)],
        )
        .await
        .unwrap(),
    );
    let app = Router::new().route("/scope-pair", get(
        move |Extension(proof): Extension<AuthenticatedExecutionConfiguration>| {
            let a = scope_a.clone(); let b = scope_b.clone();
            async move {
                let now = chrono::Utc::now().timestamp_millis();
                Json(json!({
                    "first": proof.scope("prod", "acme").unwrap().verify_current(&a, now).await.is_ok(),
                    "second": proof.scope("secondary", "acme").unwrap().verify_current(&b, now).await.is_ok(),
                }))
            }
        }
    )).layer(AuthLayer::new(Some(provider.clone())));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/scope-pair", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::new();
    let read = || client.get(&url).bearer_auth("key-original").send();
    assert_eq!(
        read().await.unwrap().json::<Value>().await.unwrap(),
        json!({"first":true,"second":true})
    );
    cfg.authority_revision = Some(2);
    cfg.api_keys[0].grants[0].providers = vec!["read".into()];
    faults
        .fail_after_matches(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
            1,
        )
        .unwrap();
    assert!(provider.reload(&cfg).await.is_err());
    assert_eq!(
        read().await.unwrap().json::<Value>().await.unwrap(),
        json!({"first":false,"second":true})
    );
    provider.reload(&cfg).await.unwrap();
    assert_eq!(
        read().await.unwrap().json::<Value>().await.unwrap(),
        json!({"first":true,"second":true})
    );
    cfg.authority_revision = Some(1);
    cfg.api_keys[0].grants[0].providers = vec!["read".into(), "write".into()];
    assert!(provider.reload(&cfg).await.is_err());
    assert_eq!(
        read().await.unwrap().json::<Value>().await.unwrap(),
        json!({"first":true,"second":true})
    );
    task.abort();
}

#[tokio::test]
async fn projection_cannot_poison_its_authentication_control_scope() {
    use acteon_core::{ResourceKind, ResourceRef};
    use acteon_executor::{catalog::QualifiedProviderCatalog, governed::BoundProvider};
    use acteon_governance::{RootBudgetLimits, permit::PermitIssuanceCeiling};
    use acteon_server::auth::projection::CredentialPolicyProjector;
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let control = coordinator(state.clone()).await;
    let authority = authority(control.clone());
    let mut cfg = configuration(1);
    cfg.api_keys[0].authority_id = Some("credential/diagnostics".into());
    let existing = AuthProvider::new_with_authority(&cfg, state.clone(), authority.clone())
        .await
        .unwrap();
    let before = serde_json::to_value(control.snapshot().await.unwrap()).unwrap();
    let target: Arc<dyn acteon_provider::DynProvider> =
        Arc::new(acteon_provider::LogProvider::new("read"));
    let bound = BoundProvider::new_trusted(
        target,
        &ResourceRef::new(
            ResourceKind::Endpoint,
            "auth-control",
            "deployment",
            "wrong-place",
        )
        .unwrap(),
        "execute",
        "opaque-v1",
        vec![],
    )
    .unwrap();
    let catalog = QualifiedProviderCatalog::new_trusted(vec![bound]).unwrap();
    let effects = catalog
        .definitions("auth-control", "deployment")
        .into_iter()
        .map(|d| d.effect)
        .collect();
    let limits = RootBudgetLimits {
        max_units: 5,
        max_concurrent: 2,
        deadline_ms: 4_102_444_800_000,
    };
    let issuance = PermitIssuanceCeiling {
        issuer: PrincipalIdentity::new("scope-publisher", PrincipalKind::System).unwrap(),
        subjects: vec![cfg.api_keys[0].principal.clone().unwrap()],
        effects,
        valid_from_ms: 0,
        limits: limits.clone(),
    };
    let projector = Arc::new(
        CredentialPolicyProjector::new_trusted(control.clone(), catalog, issuance, 0, limits)
            .await
            .unwrap(),
    );
    assert!(projector.publish(&authority, &cfg, 1).await.is_err());
    assert!(
        AuthProvider::new_with_scope_projection(&cfg, state, authority, vec![projector])
            .await
            .is_err()
    );
    assert_eq!(
        serde_json::to_value(control.snapshot().await.unwrap()).unwrap(),
        before
    );
    assert!(
        existing
            .authenticate_api_key("key-original")
            .await
            .is_some()
    );
}
