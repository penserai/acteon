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
        Self::configured(webhook, state, role, false)
    }
    fn governance(webhook: &str, state: &str) -> Self {
        Self::configured(webhook, state, "executor", true)
    }
    fn configured(webhook: &str, state: &str, role: &str, management: bool) -> Self {
        Self::configured_with_workforce(webhook, state, role, management, false)
    }
    fn workforce(webhook: &str, state: &str) -> Self {
        Self::configured_with_workforce(webhook, state, "executor", true, true)
    }
    fn configured_with_workforce(
        webhook: &str,
        state: &str,
        role: &str,
        management: bool,
        workforce: bool,
    ) -> Self {
        let directory =
            std::env::temp_dir().join(format!("acteon-execution-http-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        drop(socket);
        let mut config = format!(
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
        if management {
            config = config.replace(
                "subjects = [{id=\"agent/maya\",kind=\"agent\"}]",
                "subjects = [{id=\"agent/maya\",kind=\"agent\"},{id=\"operator\",kind=\"human\"}]",
            );
            config.push_str(
                r#"
[[execution_authority.scopes.managers]]
principal = {id="operator",kind="human"}
subjects = [{id="agent/maya",kind="agent"}]
routes = [{provider="incident",action_type="execute"}]
valid_from_ms = 0
limits = {max_units=5,max_concurrent=2,deadline_ms=4102444800000}
can_issue_permits = true
can_intervene = true
"#,
            );
        }
        if workforce {
            config = config.replace(
                "subjects = [{id=\"agent/maya\",kind=\"agent\"}]",
                "subjects = [{id=\"agent/maya\",kind=\"agent\"},{id=\"operator\",kind=\"human\"}]",
            );
            config.push_str("workforce = {teams=[{domain='prod',tenant='acme',id='reliability'}],job_classes=['execute'],can_manage_roster=true,can_issue_mandates=true}\n");
        }
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
        if management {
            use std::io::Write;
            let mut auth = fs::OpenOptions::new()
                .append(true)
                .open(directory.join("auth.toml"))
                .unwrap();
            for (name, principal, role, namespace) in [
                ("operator", "operator", "operator", "prod"),
                ("imposter", "operator", "executor", "prod"),
                ("foreign", "operator", "operator", "other"),
                ("unlisted", "unlisted", "operator", "prod"),
            ] {
                writeln!(
                    auth,
                    r#"
[[api_keys]]
name = {name:?}
authority_id = "credential/{name}"
principal = {{id={principal:?},kind="human"}}
key_hash = {:?}
role = {role:?}
[[api_keys.grants]]
namespaces = [{namespace:?}]
tenants = ["acme"]
providers = ["audit"]
actions = ["read"]
"#,
                    acteon_server::auth::api_key::hash_api_key(&format!("{name}-secret"))
                )
                .unwrap();
            }
        }
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
        tokio::time::timeout(Duration::from_secs(60), async {
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

#[tokio::test]
async fn authenticated_operator_controls_real_execution_without_dispatch_authority() {
    governance_http_contract("backend = 'memory'", false).await;
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires DATABASE_URL; actual governance controls survive restart"]
async fn postgres_governance_controls_survive_server_restart() {
    let url = std::env::var("DATABASE_URL").unwrap();
    let prefix = format!("gov_http_{}_", uuid::Uuid::new_v4().simple());
    governance_http_contract(
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

async fn governance_http_contract(state: &str, restart: bool) {
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
                Json(json!({"ok": true}))
            }
        }),
    );
    let receiver = tokio::spawn(axum::serve(listener, router).into_future());
    let mut server = Server::governance(&webhook, state);
    let client = reqwest::Client::new();
    server.ready(&client).await;
    let inspect_url = format!("{}/v1/governance?namespace=prod&tenant=acme", server.url);
    for credential in [
        "maya-secret",
        "imposter-secret",
        "foreign-secret",
        "unlisted-secret",
    ] {
        assert_eq!(
            client
                .get(&inspect_url)
                .bearer_auth(credential)
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
    }
    let snapshot = client
        .get(&inspect_url)
        .bearer_auth("operator-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(snapshot.status(), 200);
    let view: acteon_core::GovernanceScopeView = snapshot.json().await.unwrap();
    assert_eq!(view.routes.len(), 1);
    assert_eq!(view.permits.len(), 1);
    let resource = view.routes[0].effect.resources[0].clone();
    let issue = json!({"namespace":"prod", "tenant":"acme", "change_id":"issue-online", "expected_revision":0,
        "permit":{"id":"online-maya", "revision":1, "subject":{"id":"agent/maya","kind":"agent"},
            "routes":[{"provider":"incident","action_type":"execute"}], "valid_from_ms":0,
            "limits":{"max_units":5,"max_concurrent":1,"deadline_ms":4102444800000_i64}}, "reason":"incident response"});
    for credential in [
        "maya-secret",
        "imposter-secret",
        "foreign-secret",
        "unlisted-secret",
    ] {
        assert_eq!(
            client
                .post(format!("{}/v1/governance/permits", server.url))
                .bearer_auth(credential)
                .json(&issue)
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
    }
    let issued = client
        .post(format!("{}/v1/governance/permits", server.url))
        .bearer_auth("operator-secret")
        .json(&issue)
        .send()
        .await
        .unwrap();
    assert_eq!(issued.status(), 200);
    let receipt: acteon_core::GovernanceChangeReceipt = issued.json().await.unwrap();
    assert_eq!(receipt.actor, "operator");
    assert!(receipt.pending);
    let replay: acteon_core::GovernanceChangeReceipt = client
        .post(format!("{}/v1/governance/permits", server.url))
        .bearer_auth("operator-secret")
        .json(&issue)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(receipt, replay);
    let mut oversized = issue.clone();
    oversized["change_id"] = json!("oversized");
    oversized["permit"]["id"] = json!("oversized");
    oversized["permit"]["limits"]["max_units"] = json!(6);
    assert_eq!(
        client
            .post(format!("{}/v1/governance/permits", server.url))
            .bearer_auth("operator-secret")
            .json(&oversized)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let send = |credential: &'static str| {
        client
            .post(format!("{}/v1/dispatch", server.url))
            .bearer_auth(credential)
            .header(
                "x-acteon-execution-permits",
                r#"[{"id":"online-maya","accepted_revision":1}]"#,
            )
            .json(&Action::new(
                "prod",
                "acme",
                "incident",
                "execute",
                json!({"ticket":42}),
            ))
    };
    assert_eq!(send("operator-secret").send().await.unwrap().status(), 403);
    let outcome: Value = send("maya-secret")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(outcome.get("Executed").is_some(), "{outcome}");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let close = json!({"namespace":"prod", "tenant":"acme", "change_id":"road-closure", "reason":"maintenance", "change":{"kind":"close_resource","resource":resource}});
    for credential in [
        "maya-secret",
        "imposter-secret",
        "foreign-secret",
        "unlisted-secret",
    ] {
        assert_eq!(
            client
                .post(format!("{}/v1/governance/changes", server.url))
                .bearer_auth(credential)
                .json(&close)
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
    }
    assert_eq!(
        client
            .post(format!("{}/v1/governance/changes", server.url))
            .bearer_auth("operator-secret")
            .json(&close)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    if restart {
        server.restart();
        server.ready(&client).await;
    }
    let closed: acteon_core::GovernanceScopeView = client
        .get(&inspect_url)
        .bearer_auth("operator-secret")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(closed.routes[0].closed);
    assert!(closed.closed_resources.contains(&resource));
    let blocked: Value = client
        .post(format!("{}/v1/dispatch", server.url))
        .bearer_auth("maya-secret")
        .header(
            "x-acteon-execution-permits",
            r#"[{"id":"online-maya","accepted_revision":1}]"#,
        )
        .json(&Action::new(
            "prod",
            "acme",
            "incident",
            "execute",
            json!({"ticket":43}),
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(blocked.get("Failed").is_some(), "{blocked}");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let reopen = json!({"namespace":"prod", "tenant":"acme", "change_id":"road-reopen", "reason":"maintenance complete", "change":{"kind":"reopen_resource","resource":resource}});
    assert_eq!(
        client
            .post(format!("{}/v1/governance/changes", server.url))
            .bearer_auth("operator-secret")
            .json(&reopen)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let outcome: Value = client
        .post(format!("{}/v1/dispatch", server.url))
        .bearer_auth("maya-secret")
        .header(
            "x-acteon-execution-permits",
            r#"[{"id":"online-maya","accepted_revision":1}]"#,
        )
        .json(&Action::new(
            "prod",
            "acme",
            "incident",
            "execute",
            json!({"ticket":44}),
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(outcome.get("Executed").is_some(), "{outcome}");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let revoke = json!({"namespace":"prod", "tenant":"acme", "change_id":"permit-revoked", "reason":"incident resolved", "change":{"kind":"revoke_permit","permit_id":"online-maya","expected_revision":1}});
    assert_eq!(
        client
            .post(format!("{}/v1/governance/changes", server.url))
            .bearer_auth("operator-secret")
            .json(&revoke)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let blocked: Value = client
        .post(format!("{}/v1/dispatch", server.url))
        .bearer_auth("maya-secret")
        .header(
            "x-acteon-execution-permits",
            r#"[{"id":"online-maya","accepted_revision":1}]"#,
        )
        .json(&Action::new(
            "prod",
            "acme",
            "incident",
            "execute",
            json!({"ticket":45}),
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(blocked.get("Failed").is_some(), "{blocked}");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let revoke_credential = json!({"namespace":"prod", "tenant":"acme", "change_id":"credential-revoked", "reason":"offboarding", "change":{"kind":"revoke_credential","credential_id":"credential/maya","expected_revision":1}});
    assert_eq!(
        client
            .post(format!("{}/v1/governance/changes", server.url))
            .bearer_auth("operator-secret")
            .json(&revoke_credential)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        client
            .post(format!("{}/v1/dispatch", server.url))
            .bearer_auth("maya-secret")
            .header(
                "x-acteon-execution-permits",
                r#"[{"id":"maya-incident","accepted_revision":1}]"#
            )
            .json(&Action::new(
                "prod",
                "acme",
                "incident",
                "execute",
                json!({"ticket":46})
            ))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    receiver.abort();
}

#[tokio::test]
async fn workforce_management_controls_real_represented_agent_operations() {
    workforce_http_contract("backend = 'memory'", false).await;
}

async fn workforce_http_contract(state: &str, restart: bool) {
    use acteon_core::workforce::WorkforceScopeView;
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let webhook = format!("http://{}/incident", listener.local_addr().unwrap());
    let receiver = tokio::spawn(
        axum::serve(
            listener,
            Router::new().route(
                "/incident",
                post(move |Json(_): Json<Value>| {
                    let counter = counter.clone();
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Json(json!({"accepted":true}))
                    }
                }),
            ),
        )
        .into_future(),
    );
    let client = reqwest::Client::new();
    let mut server = Server::workforce(&webhook, state);
    server.ready(&client).await;
    let changes_url = format!("{}/v1/workforce/changes", server.url);
    let inspect_url = format!("{}/v1/workforce?namespace=prod&tenant=acme", server.url);
    let team = json!({"domain":"prod","tenant":"acme","id":"reliability"});
    let actor = json!({"id":"agent/maya","kind":"agent"});
    let limits = json!({"max_units":5,"max_concurrent":1,"deadline_ms":4102444800000_i64});
    let routes = json!([{"provider":"incident","action_type":"execute"}]);
    let change = |id: &str, body: Value| json!({"namespace":"prod","tenant":"acme","change_id":id,"reason":"reviewed","change":body});
    let put_team = change(
        "put-reliability",
        json!({"kind":"put_team","team":{"team":team,"revision":1,"name":"Reliability"}}),
    );
    for key in [
        "maya-secret",
        "imposter-secret",
        "foreign-secret",
        "unlisted-secret",
    ] {
        assert_eq!(
            client
                .get(&inspect_url)
                .bearer_auth(key)
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
        assert_eq!(
            client
                .post(&changes_url)
                .bearer_auth(key)
                .json(&put_team)
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
    }
    let empty: WorkforceScopeView = client
        .get(&inspect_url)
        .bearer_auth("operator-secret")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(empty.teams.is_empty());
    assert!(empty.management.can_manage_roster);
    for forbidden in [
        change(
            "foreign-team",
            json!({"kind":"put_team","team":{"team":{"domain":"prod","tenant":"acme","id":"release"},"revision":1,"name":"Release"}}),
        ),
        change(
            "wrong-actor",
            json!({"kind":"put_ownership","ownership":{"agent":{"id":"other-agent","kind":"agent"},"revision":1,"owner":{"kind":"team","team":team}}}),
        ),
    ] {
        assert_eq!(
            client
                .post(&changes_url)
                .bearer_auth("operator-secret")
                .json(&forbidden)
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
    }
    for body in [
        put_team.clone(),
        change(
            "owner",
            json!({"kind":"put_ownership","ownership":{"agent":actor,"revision":1,"owner":{"kind":"team","team":team}}}),
        ),
        change(
            "mandate",
            json!({"kind":"put_mandate","mandate":{
                "id":"standing-duty","revision":1,"represented":{"kind":"team","team":team},"actor":actor,
                "job_class":"execute","eligible_initiators":[actor],"ownership":{"id":"agent/maya","accepted_revision":1},
                "dependencies":[],"routes":routes,"valid_from_ms":0,"limits":limits
            }}),
        ),
        change(
            "permit",
            json!({"kind":"publish_represented_permit","permit":{
            "id":"represented-permit","revision":1,"subject":actor,"routes":routes,"valid_from_ms":0,"limits":limits
        },"mandate":{"id":"standing-duty","accepted_revision":1}}),
        ),
    ] {
        let response = client
            .post(&changes_url)
            .bearer_auth("operator-secret")
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let value: Value = response.json().await.unwrap();
        assert_eq!(status, 200, "{value}");
        assert_eq!(value["actor"], "operator");
    }
    let view: WorkforceScopeView = client
        .get(&inspect_url)
        .bearer_auth("operator-secret")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(view.teams.len(), 1);
    assert_eq!(view.mandates.len(), 1);
    assert_eq!(view.permit_bindings.len(), 1);
    let generation = view.generation;
    let replay: Value = client
        .post(&changes_url)
        .bearer_auth("operator-secret")
        .json(&put_team)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(replay["generation"].as_u64().unwrap() < generation);
    let view: WorkforceScopeView = client
        .get(&inspect_url)
        .bearer_auth("operator-secret")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(view.generation, generation);
    let action = Action::new(
        "prod",
        "acme",
        "incident",
        "execute",
        json!({"initiator":"forged-human","represented":"release","job_class":"override"}),
    );
    let dispatch_url = format!("{}/v1/dispatch", server.url);
    let dispatch = |action: &Action| {
        client
            .post(&dispatch_url)
            .bearer_auth("maya-secret")
            .header(
                "x-acteon-execution-permits",
                r#"[{"id":"represented-permit","accepted_revision":1}]"#,
            )
            .json(action)
    };
    for _ in 0..2 {
        let response = dispatch(&action).send().await.unwrap();
        let status = response.status();
        let outcome: Value = response.json().await.unwrap();
        assert_eq!(status, 200, "{outcome}");
        assert!(outcome.get("Executed").is_some(), "{outcome}");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let revoke = change(
        "revoke-standing",
        json!({"kind":"revoke_mandate","id":"standing-duty","expected_revision":1}),
    );
    assert_eq!(
        client
            .post(&changes_url)
            .bearer_auth("operator-secret")
            .json(&revoke)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    if restart {
        server.restart();
        server.ready(&client).await;
    }
    let next = Action::new("prod", "acme", "incident", "execute", json!({"ticket":43}));
    let refused: acteon_core::ActionOutcome =
        dispatch(&next).send().await.unwrap().json().await.unwrap();
    assert!(
        matches!(&refused, acteon_core::ActionOutcome::Failed(error) if error.code == "EXECUTION_ADMISSION_REFUSED" && !error.retryable),
        "{refused:?}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let revoked: WorkforceScopeView = client
        .get(&inspect_url)
        .bearer_auth("operator-secret")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(revoked.mandates[0].revoked);
    receiver.abort();
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires DATABASE_URL; actual workforce controls survive server restart"]
async fn postgres_workforce_controls_survive_server_restart() {
    let url = std::env::var("DATABASE_URL").unwrap();
    let prefix = format!("workforce_http_{}_", uuid::Uuid::new_v4().simple());
    workforce_http_contract(
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
