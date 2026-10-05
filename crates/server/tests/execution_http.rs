//! Real binary, middleware, configured memory backend and actual network send.
use acteon_core::Action;
use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use std::{
    fs,
    future::IntoFuture,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

struct Server {
    child: Child,
    directory: PathBuf,
    url: String,
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.directory);
    }
}
impl Server {
    fn start(webhook: &str, state: &str) -> Self {
        Self::start_with_role(webhook, state, "executor")
    }
    fn start_with_role(webhook: &str, state: &str, role: &str) -> Self {
        let directory =
            std::env::temp_dir().join(format!("acteon-execution-http-{}", uuid::Uuid::new_v4()));
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
{state}
[ui]
enabled = false
[auth]
enabled = true
config_path = "auth.toml"
watch = false
[auth.authority]
namespace = "auth-control"
tenant = "deployment"
source_id = "workforce-auth"
bootstrap = true
[signing]
enabled = true
reject_replay = true
replay_ttl_seconds = 3600
[[providers]]
name = "incident"
type = "webhook"
url = "{webhook}"
internal_hosts = ["127.0.0.1"]
[[execution_authority.scopes]]
namespace = "prod"
tenant = "acme"
bootstrap = true
publisher = {{id="operator",kind="human"}}
subjects = [{{id="agent/maya",kind="agent"}}]
routes = [{{provider="incident",action_type="execute"}}]
valid_from_ms = 0
credential_limits = {{max_units=5,max_concurrent=2,deadline_ms=4102444800000}}
root_max_units = 5
root_max_concurrent = 1
root_lifetime_ms = 60000
[[execution_authority.scopes.permits]]
id = "maya-incident"
revision = 1
subject = {{id="agent/maya",kind="agent"}}
routes = [{{provider="incident",action_type="execute"}}]
valid_from_ms = 0
limits = {{max_units=5,max_concurrent=2,deadline_ms=4102444800000}}
"#
        );
        fs::write(directory.join("acteon.toml"), config).unwrap();
        fs::write(
            directory.join("auth.toml"),
            format!(
                r#"
authority_revision = 1
[settings]
jwt_secret = "jwt-test-secret-at-least-32-bytes"
[[api_keys]]
name = "maya-agent"
authority_id = "credential/maya"
principal = {{id="agent/maya",kind="agent"}}
key_hash = {:?}
role = {role:?}
[[api_keys.grants]]
namespaces = ["prod"]
tenants = ["acme"]
providers = ["incident"]
actions = ["execute"]
"#,
                acteon_server::auth::api_key::hash_api_key("maya-secret")
            ),
        )
        .unwrap();
        let child = Self::launch(&directory);
        Self {
            child,
            directory,
            url: format!("http://127.0.0.1:{port}"),
        }
    }
    fn launch(directory: &std::path::Path) -> Child {
        let log = fs::File::create(directory.join("server.log")).unwrap();
        Command::new(env!("CARGO_BIN_EXE_acteon-server"))
            .arg("-c")
            .arg(directory.join("acteon.toml"))
            .env("ACTEON_AUTH_KEY", "11".repeat(32))
            .env(
                "ACTEON_AUTH_AUTHORITY_KEY",
                "shared-auth-fingerprint-test-key-32-bytes",
            )
            .env(
                "ACTEON_EXECUTION_AUTHORITY_KEY",
                "execution-signing-test-key-at-least-32-bytes",
            )
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap()
    }
    fn restart(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
        self.child = Self::launch(&self.directory);
    }
    async fn ready(&mut self, client: &reqwest::Client) {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                assert!(
                    self.child.try_wait().unwrap().is_none(),
                    "{}",
                    fs::read_to_string(self.directory.join("server.log")).unwrap()
                );
                if let Ok(response) = client.get(format!("{}/health", self.url)).send().await
                    && response.status().is_success()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .unwrap();
    }
}
#[tokio::test]
async fn real_server_requires_private_auth_and_permits_and_replays_without_resending() {
    execution_http_contract("backend = 'memory'", false).await;
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires DATABASE_URL; real binary and PostgreSQL persistence"]
async fn postgres_server_preserves_execution_replay_across_restart() {
    let url = std::env::var("DATABASE_URL").unwrap();
    let prefix = format!("exec_http_{}_", uuid::Uuid::new_v4().simple());
    execution_http_contract(
        &format!("backend = 'postgres'\nurl = {url:?}\nprefix = {prefix:?}"),
        true,
    )
    .await;
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    for suffix in ["state", "locks", "timeout_index", "chain_ready_index"] {
        sqlx::query(&format!("DROP TABLE public.{prefix}{suffix}"))
            .execute(&pool)
            .await
            .unwrap();
    }
    pool.close().await;
}

async fn execution_http_contract(state: &str, restart: bool) {
    let calls = Arc::new(AtomicUsize::new(0));
    let received = calls.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let webhook = format!("http://{}/incident", listener.local_addr().unwrap());
    let router = Router::new().route(
        "/incident",
        post(move |Json(_): Json<Value>| {
            let received = received.clone();
            async move {
                received.fetch_add(1, Ordering::SeqCst);
                Json(json!({"ok":true}))
            }
        }),
    );
    let receiver = tokio::spawn(axum::serve(listener, router).into_future());
    let mut server = Server::start(&webhook, state);
    let client = reqwest::Client::new();
    server.ready(&client).await;
    let action = Action::new(
        "prod",
        "acme",
        "incident",
        "execute",
        json!({"incident":42}),
    );
    let url = format!("{}/v1/dispatch", server.url);
    let permits = r#"[{"id":"maya-incident","accepted_revision":1}]"#;
    let response = client
        .post(&url)
        .json(&action)
        .header("x-acteon-execution-permits", permits)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    let response = client
        .post(&url)
        .bearer_auth("maya-secret")
        .json(&action)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    for _ in 0..2 {
        let response = client
            .post(&url)
            .bearer_auth("maya-secret")
            .json(&action)
            .header("x-acteon-execution-permits", permits)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: Value = response.json().await.unwrap();
        assert!(body.get("Executed").is_some(), "{body}");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    if restart {
        server.restart();
        server.ready(&client).await;
        let response = client
            .post(&url)
            .bearer_auth("maya-secret")
            .json(&action)
            .header("x-acteon-execution-permits", permits)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: Value = response.json().await.unwrap();
        assert!(body.get("Executed").is_some(), "{body}");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    let mut changed = action.clone();
    changed.payload = json!({"incident":99});
    let response = client
        .post(&url)
        .bearer_auth("maya-secret")
        .json(&changed)
        .header("x-acteon-execution-permits", permits)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    let denied = Action::new(
        "prod",
        "acme",
        "incident",
        "execute",
        json!({"incident":43}),
    );
    let response = client
        .post(&url)
        .bearer_auth("maya-secret")
        .json(&denied)
        .header(
            "x-acteon-execution-permits",
            r#"[{"id":"unissued","accepted_revision":1}]"#,
        )
        .send()
        .await
        .unwrap();
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["Failed"]["code"], "EXECUTION_ADMISSION_REFUSED");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // A refused permit must not claim the action ID against a corrected retry.
    let response = client
        .post(&url)
        .bearer_auth("maya-secret")
        .json(&denied)
        .header("x-acteon-execution-permits", permits)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert!(body.get("Executed").is_some(), "{body}");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let batch = [
        Action::new(
            "prod",
            "acme",
            "incident",
            "execute",
            json!({"incident":44}),
        ),
        Action::new(
            "prod",
            "acme",
            "incident",
            "execute",
            json!({"incident":45}),
        ),
    ];
    let response = client
        .post(format!("{}/v1/dispatch/batch", server.url))
        .bearer_auth("maya-secret")
        .json(&batch)
        .header("x-acteon-execution-permits", permits)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let results: Vec<Value> = response.json().await.unwrap();
    assert_eq!(results.len(), 2);
    assert!(
        results.iter().all(|body| body.get("Executed").is_some()),
        "{results:?}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    receiver.abort();
}

#[test]
fn public_execution_configuration_example_has_valid_independent_bounds() {
    let guide = include_str!("../../../docs/book/features/execution-permits.md");
    let example = guide
        .split("```toml\n")
        .nth(1)
        .unwrap()
        .split("```")
        .next()
        .unwrap();
    let config: acteon_server::config::ActeonConfig = toml::from_str(example).unwrap();
    let control = config.auth.authority.as_ref().unwrap();
    config
        .execution_authority
        .as_ref()
        .unwrap()
        .validate((&control.namespace, &control.tenant))
        .unwrap();
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires DATABASE_URL; failed startup must not publish authority"]
async fn invalid_authentication_is_rejected_before_execution_scope_publication() {
    let url = std::env::var("DATABASE_URL").unwrap();
    let prefix = format!("bad_auth_{}_", uuid::Uuid::new_v4().simple());
    let state = format!("backend = 'postgres'\nurl = {url:?}\nprefix = {prefix:?}");
    let mut server = Server::start_with_role("http://127.0.0.1:1/incident", &state, "invalid-role");
    let status = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if let Some(status) = server.child.try_wait().unwrap() {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "startup did not exit: {}",
            fs::read_to_string(server.directory.join("server.log")).unwrap()
        )
    });
    assert!(!status.success());
    let log = fs::read_to_string(server.directory.join("server.log")).unwrap();
    assert!(log.contains("invalid") && log.contains("role"), "{log}");
    let config: acteon_server::config::ActeonConfig =
        toml::from_str(&format!("[state]\n{state}")).unwrap();
    let (store, _) = acteon_server::state_factory::create_state(&config.state)
        .await
        .unwrap();
    assert!(
        acteon_governance::AuthorityCoordinator::connect(store.clone(), "prod", "acme")
            .await
            .is_err()
    );
    assert!(
        acteon_governance::AuthorityCoordinator::connect(store, "auth-control", "deployment")
            .await
            .is_err()
    );
    drop(server);
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    for suffix in ["state", "locks", "timeout_index", "chain_ready_index"] {
        sqlx::query(&format!("DROP TABLE public.{prefix}{suffix}"))
            .execute(&pool)
            .await
            .unwrap();
    }
    pool.close().await;
}
