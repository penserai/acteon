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

#[cfg(feature = "redis")]
mod server_process {
    use super::*;
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
            redis: &RedisConfig,
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
backend = "redis"
url = {url:?}
prefix = {prefix:?}
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
"#,
                url = redis.url,
                prefix = redis.prefix
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
    #[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; real production startup and watcher"]
    async fn real_server_startup_and_watcher_refuse_stale_authentication() {
        let cfg = RedisConfig {
            url: std::env::var("ACTEON_GOVERNANCE_REDIS_URL").unwrap(),
            prefix: format!("server-auth-main-{}", uuid::Uuid::new_v4()),
            ..Default::default()
        };
        let state = RedisStateStore::new(&cfg).unwrap();
        let client = reqwest::Client::new();
        // Normal startup cannot implicitly recreate a missing authority record.
        let mut missing = Server::start(&cfg, false, 1, "operator", "key-original");
        missing.wait_refused().await;
        assert!(
            missing.log().contains("refusing recreation"),
            "{}",
            missing.log()
        );
        drop(missing);
        let mut a = Server::start(&cfg, true, 1, "operator", "key-original");
        a.wait_role(&client, "key-original", "operator").await;
        let mut b = Server::start(&cfg, false, 1, "operator", "key-original");
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
        let mut obsolete = Server::start(&cfg, false, 1, "operator", "key-original");
        obsolete.wait_refused().await;
        assert!(
            obsolete.log().contains("publication refused"),
            "{}",
            obsolete.log()
        );
        drop(obsolete);
        let mut recovered = Server::start(&cfg, false, 2, "executor", "key-rotated");
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
