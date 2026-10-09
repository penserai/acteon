//! Real binary and middleware, independently authenticated caller and recipient.
#![recursion_limit = "256"]
use acteon_core::{AgentCard, AgentCardInterface, Skill};
#[cfg(feature = "redis")]
use acteon_state::StateStore;
use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

struct Server {
    process: Child,
    directory: PathBuf,
    url: String,
    worker_secret: Option<String>,
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
        let _ = fs::remove_dir_all(&self.directory);
    }
}
impl Server {
    fn start(webhook: &str, worker_secret: Option<&str>, worker_grant: &str) -> Self {
        Self::configured(
            webhook,
            worker_secret,
            worker_grant,
            false,
            "human",
            json!({"backend":"memory"}),
        )
    }
    #[allow(clippy::too_many_lines)] // One isolated binary deployment fixture.
    fn configured(
        webhook: &str,
        worker_secret: Option<&str>,
        worker_grant: &str,
        driver: bool,
        source_kind: &str,
        state: Value,
    ) -> Self {
        let directory =
            std::env::temp_dir().join(format!("acteon-agent-services-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        drop(socket);
        let mut card = AgentCard::new("notifier", "prod", "acme", "Incident notifier", "1");
        card.skills.push(Skill::new("notify"));
        card.interfaces.push(AgentCardInterface {
            kind: "rest".into(),
            url: "https://agents.example/notifier".into(),
        });
        let limits = json!({"max_units":5,"max_concurrent":2,"deadline_ms":4_102_444_800_000_i64});
        let human = json!({"id":"alice","kind":source_kind});
        let worker = json!({"id":"agent/notifier","kind":"agent"});
        let route = json!({"provider":"incident","action_type":"execute"});
        let configuration = json!({
            "server":{"host":"127.0.0.1","port":port,"cors_allowed_origins":["https://console.example"]},
            "state":state, "ui":{"enabled":false},
            "auth":{"enabled":true,"config_path":"auth.toml","watch":false,
                "authority":{"namespace":"auth-control","tenant":"deployment","source_id":"service-auth","bootstrap":true}},
            "providers":[{"name":"incident","type":"webhook","url":webhook,"internal_hosts":["127.0.0.1"]}],
            "execution_authority":{"agent_driver":{"enabled":driver,"poll_interval_ms":100,"max_parallel":2,"scan_batch_size":4},"scopes":[{
                "namespace":"prod","tenant":"acme","bootstrap":true,
                "publisher":{"id":"operator","kind":"human"},"subjects":[human,worker],
                "routes":[route],"valid_from_ms":0,"credential_limits":limits,
                "root_max_units":5,"root_max_concurrent":1,"root_lifetime_ms":60000,
                "agent_services":[{
                    "card":card,"principal":worker,"skill":"notify","endpoint":"https://agents.example/notifier",
                    "endpoint_id":"notifier-api","route":route,"recipient_key_env":"ACTEON_TEST_AGENT_RECIPIENT",
                    "recipient_permits":[{"id":"worker-provider","accepted_revision":1}],
                    "grants":[{"id":"alice-notifier","revision":1,"source":human,
                        "source_permits":[{"id":"alice-service","accepted_revision":1}],"valid_from_ms":0,"limits":limits,"max_depth":4}]
                }],
                "permits":[
                    {"id":"alice-service","revision":1,"subject":human,"routes":[],"agents":["notifier"],"valid_from_ms":0,"limits":limits},
                    {"id":"worker-provider","revision":1,"subject":worker,"routes":[route],"valid_from_ms":0,"limits":limits}
                ]
            }]}
        });
        fs::write(
            directory.join("acteon.toml"),
            toml::to_string(&configuration).unwrap(),
        )
        .unwrap();
        fs::write(
            directory.join("auth.toml"),
            format!(
                r#"
authority_revision = 1
[settings]
jwt_secret = "test-jwt-secret-at-least-32-bytes"
[[api_keys]]
name = "alice"
authority_id = "credential/alice"
principal = {{id="alice",kind={source_kind:?}}}
key_hash = {:?}
role = "executor"
[[api_keys.grants]]
namespaces = ["prod"]
tenants = ["acme"]
providers = ["agent.notifier"]
actions = ["invoke"]
[[api_keys]]
name = "notifier"
authority_id = "credential/notifier"
principal = {{id="agent/notifier",kind="agent"}}
key_hash = {:?}
role = "executor"
[[api_keys.grants]]
namespaces = ["prod"]
tenants = ["acme"]
providers = [{worker_grant:?}]
actions = ["execute"]
"#,
                acteon_server::auth::api_key::hash_api_key("alice-secret"),
                acteon_server::auth::api_key::hash_api_key("notifier-secret")
            ),
        )
        .unwrap();
        let mut auth = fs::OpenOptions::new()
            .append(true)
            .open(directory.join("auth.toml"))
            .unwrap();
        writeln!(
            auth,
            r#"
[[api_keys]]
name = "legacy-observer"
authority_id = "credential/observer"
principal = {{id="alice",kind={source_kind:?}}}
key_hash = {:?}
role = "operator"
[[api_keys.grants]]
namespaces = ["prod"]
tenants = ["acme"]
providers = ["a2a"]
actions = ["rpc"]
"#,
            acteon_server::auth::api_key::hash_api_key("observer-secret")
        )
        .unwrap();
        Self {
            process: Self::launch(&directory, worker_secret),
            directory,
            url: format!("http://127.0.0.1:{port}"),
            worker_secret: worker_secret.map(str::to_owned),
        }
    }
    fn launch(directory: &std::path::Path, worker_secret: Option<&str>) -> Child {
        let log = fs::File::create(directory.join("server.log")).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_acteon-server"));
        command
            .arg("-c")
            .arg(directory.join("acteon.toml"))
            .env("ACTEON_AUTH_KEY", "11".repeat(32))
            .env(
                "ACTEON_AUTH_AUTHORITY_KEY",
                "service-auth-fingerprint-at-least-32-bytes",
            )
            .env(
                "ACTEON_EXECUTION_AUTHORITY_KEY",
                "service-context-signing-at-least-32-bytes",
            )
            .env_remove("ACTEON_TEST_AGENT_RECIPIENT")
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log));
        if let Some(secret) = worker_secret {
            command.env("ACTEON_TEST_AGENT_RECIPIENT", secret);
        }
        command.spawn().unwrap()
    }
    fn restart(&mut self, driver: bool) {
        self.process.kill().unwrap();
        self.process.wait().unwrap();
        let path = self.directory.join("acteon.toml");
        let mut config: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        config["execution_authority"]["agent_driver"]["enabled"] = toml::Value::Boolean(driver);
        fs::write(path, toml::to_string(&config).unwrap()).unwrap();
        self.process = Self::launch(&self.directory, self.worker_secret.as_deref());
    }
    fn replace_service_and_retain(&mut self, binding_digest: &str, driver: bool) {
        self.process.kill().unwrap();
        self.process.wait().unwrap();
        let path = self.directory.join("acteon.toml");
        let mut config: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        config["execution_authority"]["agent_driver"]["enabled"] = toml::Value::Boolean(driver);
        let scope = &mut config["execution_authority"]["scopes"][0];
        let current = scope["agent_services"][0].clone();
        let current_table = current.as_table().unwrap();
        let mut retained = toml::map::Map::new();
        for field in [
            "card",
            "principal",
            "skill",
            "endpoint",
            "endpoint_id",
            "route",
        ] {
            retained.insert(field.into(), current_table[field].clone());
        }
        retained.insert("registry_revision".into(), toml::Value::Integer(1));
        retained.insert(
            "binding_digest".into(),
            toml::Value::String(binding_digest.into()),
        );
        scope.as_table_mut().unwrap().insert(
            "retained_agent_services".into(),
            toml::Value::Array(vec![toml::Value::Table(retained)]),
        );
        scope["agent_services"][0]
            .as_table_mut()
            .unwrap()
            .insert("registry_revision".into(), toml::Value::Integer(2));
        scope["agent_services"][0]["card"]["version"] = toml::Value::String("2".into());
        scope["agent_services"][0]["grants"][0]["id"] =
            toml::Value::String("alice-notifier-v2".into());
        scope["agent_services"][0]["grants"][0]["source_permits"][0]["accepted_revision"] =
            toml::Value::Integer(2);
        scope["permits"][0]["revision"] = toml::Value::Integer(2);
        fs::write(path, toml::to_string(&config).unwrap()).unwrap();
        let auth_path = self.directory.join("auth.toml");
        let auth = fs::read_to_string(&auth_path).unwrap().replacen(
            "authority_revision = 1",
            "authority_revision = 2",
            1,
        );
        fs::write(auth_path, auth).unwrap();
        self.process = Self::launch(&self.directory, self.worker_secret.as_deref());
    }
    fn remove_service_and_retain_current(&mut self, binding_digest: &str) {
        self.process.kill().unwrap();
        self.process.wait().unwrap();
        let path = self.directory.join("acteon.toml");
        let mut config: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let scope = &mut config["execution_authority"]["scopes"][0];
        let current = scope["agent_services"][0].as_table().unwrap();
        let mut retained = toml::map::Map::new();
        for field in [
            "card",
            "registry_revision",
            "principal",
            "skill",
            "endpoint",
            "endpoint_id",
            "route",
        ] {
            retained.insert(field.into(), current[field].clone());
        }
        retained.insert(
            "binding_digest".into(),
            toml::Value::String(binding_digest.into()),
        );
        scope["retained_agent_services"]
            .as_array_mut()
            .unwrap()
            .push(toml::Value::Table(retained));
        scope["agent_services"] = toml::Value::Array(Vec::new());
        scope["permits"].as_array_mut().unwrap().remove(0);
        fs::write(path, toml::to_string(&config).unwrap()).unwrap();
        let auth_path = self.directory.join("auth.toml");
        let auth = fs::read_to_string(&auth_path).unwrap().replacen(
            "authority_revision = 2",
            "authority_revision = 3",
            1,
        );
        fs::write(auth_path, auth).unwrap();
        self.process = Self::launch(&self.directory, self.worker_secret.as_deref());
    }
    fn task_url(&self, id: &str) -> String {
        format!("{}/a2a/prod/acme/agents/notifier/v1/tasks/{id}", self.url)
    }
    async fn ready(&mut self, client: &reqwest::Client) {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                assert!(self.process.try_wait().unwrap().is_none(), "{}", self.log());
                if client
                    .get(format!("{}/health", self.url))
                    .send()
                    .await
                    .is_ok()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
    }
    fn log(&self) -> String {
        fs::read_to_string(self.directory.join("server.log")).unwrap()
    }
    async fn rejected_startup(&mut self) {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Some(status) = self.process.try_wait().unwrap() {
                    assert!(!status.success());
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
    }
    fn endpoint(&self) -> String {
        format!("{}/a2a/prod/acme/agents/notifier/v1/message:send", self.url)
    }
}

async fn webhook() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let app = Router::new().route(
        "/incident",
        post(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Json(json!({"delivered":true}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}/incident"), calls, task)
}
fn message(id: &str) -> Value {
    json!({"message":{"role":"user","messageId":id,"parts":[{"kind":"text","text":"Notify the incident owner"}]}})
}

#[tokio::test]
async fn missing_wrong_or_unqualified_recipient_prevents_listener_startup() {
    let (url, calls, webhook_task) = webhook().await;
    for (secret, grant) in [
        (None, "incident"),
        (Some("alice-secret"), "incident"),
        (Some("notifier-secret"), "unrelated"),
    ] {
        let mut server = Server::start(&url, secret, grant);
        server.rejected_startup().await;
        assert!(
            server
                .log()
                .contains("agent service state or runtime unavailable"),
            "{}",
            server.log()
        );
        assert!(!server.log().contains("alice-secret"));
        assert!(!server.log().contains("notifier-secret"));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    webhook_task.abort();
}

#[tokio::test]
async fn peer_submission_action_suffixes_reach_the_governed_handlers() {
    let (url, calls, webhook_task) = webhook().await;
    let mut server = Server::start(&url, Some("notifier-secret"), "incident");
    let client = reqwest::Client::new();
    server.ready(&client).await;
    let task = "11111111-1111-4111-8111-111111111111";
    let submission = "22222222-2222-4222-8222-222222222222";
    let base = format!(
        "{}/a2a/prod/acme/agents/notifier/v1/tasks/{task}/peers/notifier/notify/submissions/{submission}",
        server.url
    );

    for action in ["refresh", "cancel"] {
        let response = client
            .post(format!("{base}:{action}"))
            .bearer_auth("alice-secret")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 404);
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({"error":"agent_peer_source_unavailable"})
        );
    }
    let response = client
        .post(format!("{base}:unknown"))
        .bearer_auth("alice-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"error":"invalid_agent_peer_request"})
    );
    let response = client
        .post(format!("{base}:unknown"))
        .bearer_auth("observer-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"error":"agent_peer_authority_required"})
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    webhook_task.abort();
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One admitted task and its complete refusal/replay contract.
async fn authenticated_individual_service_accepts_and_replays_without_effect_start() {
    let (url, calls, webhook_task) = webhook().await;
    let mut server = Server::start(&url, Some("notifier-secret"), "incident");
    let client = reqwest::Client::new();
    server.ready(&client).await;
    let request = message("same-message");
    let send = || {
        client
            .post(server.endpoint())
            .bearer_auth("alice-secret")
            .json(&request)
    };
    let response = send().send().await.unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    let accepted: Value = response.json().await.unwrap();
    assert_eq!(accepted["status"]["state"], "submitted");
    let response = send().send().await.unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    let replayed: Value = response.json().await.unwrap();
    assert_eq!(accepted["id"], replayed["id"]);
    let response = client
        .post(server.endpoint())
        .bearer_auth("notifier-secret")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let response = client
        .post(server.endpoint())
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    let response = client
        .post(format!("{}/v1/dispatch", server.url))
        .bearer_auth("alice-secret")
        .header(
            "x-acteon-execution-permits",
            r#"[{"id":"worker-provider","accepted_revision":1}]"#,
        )
        .json(&acteon_core::Action::new(
            "prod",
            "acme",
            "incident",
            "execute",
            json!({}),
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        403,
        "service authority cannot borrow recipient provider authority: {}",
        response.text().await.unwrap()
    );
    let mut changed = request.clone();
    changed["message"]["parts"][0]["text"] = json!("A different operation");
    let response = client
        .post(server.endpoint())
        .bearer_auth("alice-secret")
        .json(&changed)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    let response = send().header("a2a-version", "999").send().await.unwrap();
    assert_eq!(response.status(), 400);
    let task_id = accepted["id"].as_str().unwrap();
    let legacy = format!("{}/a2a/prod/acme/v1/tasks/{task_id}", server.url);
    for response in [
        client
            .get(&legacy)
            .bearer_auth("observer-secret")
            .send()
            .await
            .unwrap(),
        client
            .post(&legacy)
            .bearer_auth("observer-secret")
            .send()
            .await
            .unwrap(),
        client
            .get(format!("{legacy}/events"))
            .bearer_auth("observer-secret")
            .send()
            .await
            .unwrap(),
        client
            .post(format!("{legacy}/pushNotificationConfigs"))
            .bearer_auth("observer-secret")
            .json(&json!({"url":"https://callbacks.example/task"}))
            .send()
            .await
            .unwrap(),
    ] {
        assert_eq!(response.status(), 404, "{}", response.text().await.unwrap());
    }
    let mut continuation = message("legacy-continuation");
    continuation["message"]["taskId"] = json!(task_id);
    let response = client
        .post(format!("{}/a2a/prod/acme/v1/message:send", server.url))
        .bearer_auth("observer-secret")
        .json(&continuation)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404, "{}", response.text().await.unwrap());
    let mut injected = request;
    injected["principal"] = json!({"id":"operator","kind":"human"});
    let response = client
        .post(server.endpoint())
        .bearer_auth("alice-secret")
        .json(&injected)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 422);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "acceptance cannot itself start a provider effect"
    );
    webhook_task.abort();
}

async fn send_task(server: &Server, client: &reqwest::Client, id: &str) -> (Value, String) {
    let response = client
        .post(server.endpoint())
        .bearer_auth("alice-secret")
        .json(&message(id))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    let source = response.headers()["x-acteon-agent-source-context"]
        .to_str()
        .unwrap()
        .to_owned();
    (response.json().await.unwrap(), source)
}

#[tokio::test]
async fn governed_parent_handoff_creates_a_verified_child_and_requires_paired_headers() {
    let (url, calls, webhook_task) = webhook().await;
    let mut server = Server::start(&url, Some("notifier-secret"), "incident");
    let client = reqwest::Client::new();
    server.ready(&client).await;
    let (_, parent) = send_task(&server, &client, "parent-root").await;
    let permits = r#"[{"id":"alice-service","accepted_revision":1}]"#;

    let response = client
        .post(server.endpoint())
        .bearer_auth("alice-secret")
        .header("x-acteon-execution-context", &parent)
        .header("x-acteon-execution-permits", permits)
        .json(&message("delegated-child"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());

    for request in [
        client
            .post(server.endpoint())
            .bearer_auth("alice-secret")
            .header("x-acteon-execution-context", &parent),
        client
            .post(server.endpoint())
            .bearer_auth("alice-secret")
            .header("x-acteon-execution-permits", permits),
    ] {
        let response = request
            .json(&message("unpaired-parent"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{}", response.text().await.unwrap());
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    webhook_task.abort();
}

async fn await_completed(server: &Server, client: &reqwest::Client, id: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let response = client
                .get(server.task_url(id))
                .bearer_auth("alice-secret")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
            let task: Value = response.json().await.unwrap();
            if task["status"]["state"] == "completed" {
                return task;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn requester_observation_never_drives_work_and_agent_identity_requires_exact_job() {
    let (url, calls, webhook_task) = webhook().await;
    let mut server = Server::configured(
        &url,
        Some("notifier-secret"),
        "incident",
        false,
        "agent",
        json!({"backend":"memory"}),
    );
    let client = reqwest::Client::new();
    server.ready(&client).await;
    let (task, source) = send_task(&server, &client, "agent-job-one").await;
    let (other, other_source) = send_task(&server, &client, "agent-job-two").await;
    let endpoint = server.task_url(task["id"].as_str().unwrap());
    for (token, context, expected) in [
        ("alice-secret", None, 404),
        ("alice-secret", Some(other_source.as_str()), 404),
        ("observer-secret", Some(source.as_str()), 404),
        ("notifier-secret", Some(source.as_str()), 404),
        ("alice-secret", Some(source.as_str()), 200),
    ] {
        let mut request = client.get(&endpoint).bearer_auth(token);
        if let Some(context) = context {
            request = request.header("x-acteon-agent-source-context", context);
        }
        let response = request.send().await.unwrap();
        assert_eq!(
            response.status(),
            expected,
            "{}",
            response.text().await.unwrap()
        );
        if expected == 200 {
            let observed: Value = response.json().await.unwrap();
            assert_eq!(observed["id"], task["id"]);
            assert_eq!(observed["status"]["state"], "submitted");
        }
    }
    assert_ne!(task["id"], other["id"]);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    webhook_task.abort();
}

#[tokio::test]
async fn enabled_driver_executes_once_and_requester_observes_real_result() {
    let (url, calls, webhook_task) = webhook().await;
    let mut server = Server::configured(
        &url,
        Some("notifier-secret"),
        "incident",
        true,
        "human",
        json!({"backend":"memory"}),
    );
    let client = reqwest::Client::new();
    server.ready(&client).await;
    let (task, source) = send_task(&server, &client, "driver-job").await;
    let completed = await_completed(&server, &client, task["id"].as_str().unwrap()).await;
    assert_eq!(completed["artifacts"].as_array().unwrap().len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let (replayed, replayed_source) = send_task(&server, &client, "driver-job").await;
    assert_eq!(replayed["id"], task["id"]);
    assert_eq!(source, replayed_source);
    for token in ["observer-secret", "notifier-secret"] {
        let response = client
            .get(server.task_url(task["id"].as_str().unwrap()))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 404);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    webhook_task.abort();
}

#[cfg(feature = "redis")]
fn redis_state() -> (Value, acteon_state_redis::RedisConfig) {
    let config = acteon_state_redis::RedisConfig {
        url: std::env::var("ACTEON_GOVERNANCE_REDIS_URL").expect("set ACTEON_GOVERNANCE_REDIS_URL"),
        prefix: format!("agent-services-{}", uuid::Uuid::new_v4()),
        ..Default::default()
    };
    (
        json!({"backend":"redis","url":config.url,"prefix":config.prefix}),
        config,
    )
}

struct PeerMeshServer {
    process: Child,
    directory: PathBuf,
    url: String,
}

impl Drop for PeerMeshServer {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
        let _ = fs::remove_dir_all(&self.directory);
    }
}

impl PeerMeshServer {
    #[allow(clippy::too_many_lines)]
    fn start(
        own_port: u16,
        notifier_port: u16,
        resolver_port: u16,
        webhook: &str,
        state: &Value,
        credential_hashes: &[String; 3],
        bootstrap: bool,
    ) -> Self {
        let directory =
            std::env::temp_dir().join(format!("acteon-peer-mesh-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        let mut params = rcgen::CertificateParams::new(vec!["localhost".into()]).unwrap();
        params
            .subject_alt_names
            .push(rcgen::SanType::IpAddress("127.0.0.1".parse().unwrap()));
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&key).unwrap();
        let cert_path = directory.join("server.crt");
        let key_path = directory.join("server.key");
        fs::write(&cert_path, cert.pem()).unwrap();
        fs::write(&key_path, key.serialize_pem()).unwrap();

        let notifier_endpoint = format!(
            "https://127.0.0.1:{notifier_port}/a2a/prod/acme/agents/notifier/v1/message:send"
        );
        let resolver_endpoint = format!(
            "https://127.0.0.1:{resolver_port}/a2a/prod/acme/agents/resolver/v1/message:send"
        );
        let mut notifier = AgentCard::new("notifier", "prod", "acme", "Notifier", "1");
        let card_time = chrono::DateTime::parse_from_rfc3339("2026-10-08T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        notifier.created_at = card_time;
        notifier.updated_at = card_time;
        notifier.skills.push(Skill::new("notify"));
        notifier.interfaces.push(AgentCardInterface {
            kind: "rest".into(),
            url: notifier_endpoint.clone(),
        });
        let mut resolver = AgentCard::new("resolver", "prod", "acme", "Resolver", "1");
        resolver.created_at = card_time;
        resolver.updated_at = card_time;
        resolver.skills.push(Skill::new("resolve"));
        resolver.interfaces.push(AgentCardInterface {
            kind: "rest".into(),
            url: resolver_endpoint.clone(),
        });
        let limits = json!({"max_units":8,"max_concurrent":3,"deadline_ms":4_102_444_800_000_i64});
        let alice = json!({"id":"alice","kind":"human"});
        let notifier_principal = json!({"id":"agent/notifier","kind":"agent"});
        let resolver_principal = json!({"id":"agent/resolver","kind":"agent"});
        let notify_route = json!({"provider":"incident","action_type":"execute"});
        let resolve_route = json!({"provider":"resolver","action_type":"execute"});
        let configuration = json!({
            "server":{"host":"127.0.0.1","port":own_port},
            "tls":{"enabled":true,
                "server":{"cert_path":cert_path,"key_path":key_path},
                "client":{"danger_accept_invalid_certs":true}},
            "state":state,
            "ui":{"enabled":false},
            "auth":{"enabled":true,"config_path":"auth.toml","watch":false,
                "authority":{"namespace":"auth-control","tenant":"deployment","source_id":"peer-mesh-auth","bootstrap":bootstrap}},
            "providers":[
                {"name":"incident","type":"webhook","url":webhook,"internal_hosts":["127.0.0.1"]},
                {"name":"resolver","type":"webhook","url":webhook,"internal_hosts":["127.0.0.1"]}
            ],
            "execution_authority":{
                "agent_driver":{"enabled":false,"poll_interval_ms":100,"max_parallel":2,"scan_batch_size":8},
                "peer_transport":{"enabled":true,"timeout_ms":3000,"adapter_revision":"peer-mesh-test-v1","internal_hosts":["127.0.0.1"]},
                "scopes":[{
                    "namespace":"prod","tenant":"acme","bootstrap":bootstrap,
                    "publisher":{"id":"operator","kind":"human"},
                    "subjects":[alice,notifier_principal,resolver_principal],
                    "routes":[notify_route,resolve_route],"valid_from_ms":0,
                    "credential_limits":limits,"root_max_units":8,"root_max_concurrent":2,"root_lifetime_ms":60000,
                    "agent_services":[
                        {"card":notifier,"principal":notifier_principal,"skill":"notify",
                         "endpoint":notifier_endpoint,"endpoint_id":"notifier-api","route":notify_route,
                         "recipient_key_env":"ACTEON_TEST_AGENT_RECIPIENT",
                         "recipient_permits":[{"id":"notifier-provider","accepted_revision":1}],
                         "onward_agents":["resolver"],
                         "grants":[{"id":"alice-notifier","revision":1,"source":alice,
                           "source_permits":[{"id":"alice-service","accepted_revision":1}],
                           "valid_from_ms":0,"limits":limits,"max_depth":4}]},
                        {"card":resolver,"principal":resolver_principal,"skill":"resolve",
                         "endpoint":resolver_endpoint,"endpoint_id":"resolver-api","route":resolve_route,
                         "recipient_key_env":"ACTEON_TEST_RESOLVER_RECIPIENT",
                         "recipient_permits":[{"id":"resolver-provider","accepted_revision":1}],
                         "grants":[{"id":"notifier-resolver","revision":1,"source":notifier_principal,
                           "source_permits":[{"id":"notifier-provider","accepted_revision":1}],
                           "valid_from_ms":0,"limits":limits,"max_depth":4}]}
                    ],
                    "permits":[
                        {"id":"alice-service","revision":1,"subject":alice,"routes":[],"agents":["notifier"],"valid_from_ms":0,"limits":limits},
                        {"id":"notifier-provider","revision":1,"subject":notifier_principal,"routes":[notify_route],"agents":["resolver"],"valid_from_ms":0,"limits":limits},
                        {"id":"resolver-provider","revision":1,"subject":resolver_principal,"routes":[resolve_route],"valid_from_ms":0,"limits":limits}
                    ]
                }]
            }
        });
        fs::write(
            directory.join("acteon.toml"),
            toml::to_string(&configuration).unwrap(),
        )
        .unwrap();
        fs::write(
            directory.join("auth.toml"),
            format!(
                r#"authority_revision = 1
[settings]
jwt_secret = "test-jwt-secret-at-least-32-bytes"
[[api_keys]]
name = "alice"
authority_id = "credential/alice"
principal = {{id="alice",kind="human"}}
key_hash = {:?}
role = "executor"
[[api_keys.grants]]
namespaces = ["prod"]
tenants = ["acme"]
providers = ["agent.notifier"]
actions = ["invoke"]
[[api_keys]]
name = "notifier"
authority_id = "credential/notifier"
principal = {{id="agent/notifier",kind="agent"}}
key_hash = {:?}
role = "executor"
[[api_keys.grants]]
namespaces = ["prod"]
tenants = ["acme"]
providers = ["incident"]
actions = ["execute"]
[[api_keys.grants]]
namespaces = ["prod"]
tenants = ["acme"]
providers = ["agent.resolver"]
actions = ["invoke"]
[[api_keys]]
name = "resolver"
authority_id = "credential/resolver"
principal = {{id="agent/resolver",kind="agent"}}
key_hash = {:?}
role = "executor"
[[api_keys.grants]]
namespaces = ["prod"]
tenants = ["acme"]
providers = ["resolver"]
actions = ["execute"]
"#,
                credential_hashes[0], credential_hashes[1], credential_hashes[2]
            ),
        )
        .unwrap();
        let process = Self::launch(&directory);
        Self {
            process,
            directory,
            url: format!("https://127.0.0.1:{own_port}"),
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
                "peer-mesh-auth-fingerprint-at-least-32-bytes",
            )
            .env(
                "ACTEON_EXECUTION_AUTHORITY_KEY",
                "peer-mesh-context-signing-at-least-32-bytes",
            )
            .env("ACTEON_TEST_AGENT_RECIPIENT", "notifier-secret")
            .env("ACTEON_TEST_RESOLVER_RECIPIENT", "resolver-secret")
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap()
    }

    async fn ready(&mut self, client: &reqwest::Client) {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                assert!(self.process.try_wait().unwrap().is_none(), "{}", self.log());
                if client
                    .get(format!("{}/health", self.url))
                    .send()
                    .await
                    .is_ok()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
    }

    fn restart(&mut self) {
        self.process.kill().unwrap();
        self.process.wait().unwrap();
        self.process = Self::launch(&self.directory);
    }

    fn log(&self) -> String {
        fs::read_to_string(self.directory.join("server.log")).unwrap()
    }
}

fn reserve_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test]
#[cfg(feature = "redis")]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; two real HTTPS servers sharing Redis"]
async fn redis_two_server_peer_cancel_survives_source_restart_as_a_durable_restriction() {
    let (webhook_url, calls, webhook_task) = webhook().await;
    let (state, redis_config) = redis_state();
    let notifier_port = reserve_port();
    let resolver_port = reserve_port();
    assert_ne!(notifier_port, resolver_port);
    let credential_hashes = [
        acteon_server::auth::api_key::hash_api_key("alice-secret"),
        acteon_server::auth::api_key::hash_api_key("notifier-secret"),
        acteon_server::auth::api_key::hash_api_key("resolver-secret"),
    ];
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();
    let mut resolver = PeerMeshServer::start(
        resolver_port,
        notifier_port,
        resolver_port,
        &webhook_url,
        &state,
        &credential_hashes,
        true,
    );
    resolver.ready(&client).await;
    let mut notifier = PeerMeshServer::start(
        notifier_port,
        notifier_port,
        resolver_port,
        &webhook_url,
        &state,
        &credential_hashes,
        true,
    );
    assert_eq!(
        fs::read_to_string(resolver.directory.join("auth.toml")).unwrap(),
        fs::read_to_string(notifier.directory.join("auth.toml")).unwrap()
    );
    let resolver_config: toml::Value =
        toml::from_str(&fs::read_to_string(resolver.directory.join("acteon.toml")).unwrap())
            .unwrap();
    let notifier_config: toml::Value =
        toml::from_str(&fs::read_to_string(notifier.directory.join("acteon.toml")).unwrap())
            .unwrap();
    assert_eq!(
        resolver_config["execution_authority"],
        notifier_config["execution_authority"]
    );
    assert_eq!(resolver_config["providers"], notifier_config["providers"]);
    let store = acteon_state_redis::RedisStateStore::new(&redis_config).unwrap();
    for index in 0..2 {
        let card: AgentCard = serde_json::from_value(
            serde_json::to_value(
                &resolver_config["execution_authority"]["scopes"][0]["agent_services"][index]
                    ["card"],
            )
            .unwrap(),
        )
        .unwrap();
        let mut agent = acteon_core::Agent::new(&card.agent_id, "prod", "acme");
        agent.last_heartbeat_at = Some(chrono::Utc::now());
        agent.has_agent_card = true;
        store
            .set(
                &acteon_state::StateKey::new(
                    "prod",
                    "acme",
                    acteon_state::KeyKind::BusAgent,
                    &card.agent_id,
                ),
                &serde_json::to_string(&agent).unwrap(),
                None,
            )
            .await
            .unwrap();
        store
            .set(
                &acteon_state::StateKey::new(
                    "prod",
                    "acme",
                    acteon_state::KeyKind::BusAgentCard,
                    &card.agent_id,
                ),
                &serde_json::to_string(&card).unwrap(),
                None,
            )
            .await
            .unwrap();
    }
    notifier.ready(&client).await;

    let response = client
        .post(format!(
            "{}/a2a/prod/acme/agents/notifier/v1/message:send",
            notifier.url
        ))
        .bearer_auth("alice-secret")
        .json(&message("mesh-root"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    let root: Value = response.json().await.unwrap();
    let root_id = root["id"].as_str().unwrap();

    let peer_base = format!(
        "{}/a2a/prod/acme/agents/notifier/v1/tasks/{root_id}/peers",
        notifier.url
    );
    let response = client
        .get(&peer_base)
        .query(&[("skill", "resolve")])
        .bearer_auth("notifier-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    let discovered: Value = response.json().await.unwrap();
    assert_eq!(discovered["peers"].as_array().unwrap().len(), 1);
    assert_eq!(discovered["peers"][0]["agent_id"], "resolver");
    assert!(discovered["peers"][0].get("endpoint").is_none());

    let response = client
        .post(format!("{peer_base}/resolver/resolve/message:send"))
        .bearer_auth("notifier-secret")
        .json(&message("mesh-child"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    let peer: Value = response.json().await.unwrap();
    assert_eq!(peer["status"]["state"], "accepted");
    assert_eq!(peer["status"]["task"]["status"]["state"], "submitted");
    let submission = peer["submission_id"].as_str().unwrap();
    let cancel_url = format!("{peer_base}/resolver/resolve/submissions/{submission}:cancel");
    let response = client
        .post(&cancel_url)
        .bearer_auth("notifier-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    let canceled: Value = response.json().await.unwrap();
    assert_eq!(canceled["status"]["state"], "restricted");
    assert_eq!(
        canceled["status"]["task"]["id"],
        peer["status"]["task"]["id"]
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let cancellation_id = canceled["cancellation_id"].clone();
    notifier.restart();
    notifier.ready(&client).await;
    let response = client
        .post(&cancel_url)
        .bearer_auth("notifier-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    let replayed: Value = response.json().await.unwrap();
    assert_eq!(replayed["cancellation_id"], cancellation_id);
    assert_eq!(replayed["status"], canceled["status"]);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    webhook_task.abort();
}

#[tokio::test]
#[cfg(feature = "redis")]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; real server restart with isolated StateStore prefix"]
async fn redis_restart_recovers_queued_work_without_requester_resubmission_and_keeps_known_result()
{
    let (url, calls, webhook_task) = webhook().await;
    let (backend, _) = redis_state();
    let mut server = Server::configured(
        &url,
        Some("notifier-secret"),
        "incident",
        false,
        "human",
        backend,
    );
    let client = reqwest::Client::new();
    server.ready(&client).await;
    let (task, source) = send_task(&server, &client, "persisted-before-restart").await;
    assert_eq!(task["status"]["state"], "submitted");
    let response = client
        .get(server.task_url(task["id"].as_str().unwrap()))
        .bearer_auth("alice-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    server.restart(true);
    server.ready(&client).await;
    let completed = await_completed(&server, &client, task["id"].as_str().unwrap()).await;
    assert_eq!(completed["id"], task["id"]);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    server.restart(true);
    server.ready(&client).await;
    let recovered = await_completed(&server, &client, task["id"].as_str().unwrap()).await;
    assert_eq!(recovered["artifacts"], completed["artifacts"]);
    let (replayed, replayed_source) = send_task(&server, &client, "persisted-before-restart").await;
    assert_eq!(replayed["id"], task["id"]);
    assert_eq!(source, replayed_source);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // Observe accepted work after role offboarding. A GET cannot gain dispatch.
    let path = server.directory.join("auth.toml");
    let auth = fs::read_to_string(&path)
        .unwrap()
        .replace("authority_revision = 1", "authority_revision = 2")
        .replacen("role = \"executor\"", "role = \"viewer\"", 1);
    fs::write(path, auth).unwrap();
    server.restart(true);
    server.ready(&client).await;
    let observed = await_completed(&server, &client, task["id"].as_str().unwrap()).await;
    assert_eq!(observed["id"], task["id"]);
    let response = client
        .post(server.endpoint())
        .bearer_auth("alice-secret")
        .json(&message("new-offboarded-job"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    webhook_task.abort();
}

#[tokio::test]
#[cfg(feature = "redis")]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; retained service replacement contract"]
async fn redis_service_replacement_recovers_old_tasks_and_routes_new_work_to_the_new_binding() {
    use acteon_governance::AuthorityCoordinator;
    let (url, calls, webhook_task) = webhook().await;
    let (backend, settings) = redis_state();
    let store: Arc<dyn acteon_state::StateStore> =
        Arc::new(acteon_state_redis::RedisStateStore::new(&settings).unwrap());
    let mut server = Server::configured(
        &url,
        Some("notifier-secret"),
        "incident",
        true,
        "human",
        backend,
    );
    let client = reqwest::Client::new();
    server.ready(&client).await;
    let coordinator = AuthorityCoordinator::connect(store, "prod", "acme")
        .await
        .unwrap();
    let old_digest = coordinator.snapshot().await.unwrap().agent_registry["notifier"]
        .qualification
        .bindings["notify"]
        .clone();
    let (completed_task, completed_source) =
        send_task(&server, &client, "completed-before-replacement").await;
    let completed = await_completed(&server, &client, completed_task["id"].as_str().unwrap()).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    server.restart(false);
    server.ready(&client).await;
    let (queued_task, _) = send_task(&server, &client, "queued-before-replacement").await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    server.replace_service_and_retain(&old_digest, true);
    server.ready(&client).await;
    let replacement = coordinator.snapshot().await.unwrap();
    let registry = &replacement.agent_registry["notifier"];
    assert_eq!(registry.qualification.revision, 2);
    let new_digest = &registry.qualification.bindings["notify"];
    assert_ne!(new_digest, &old_digest);

    let response = client
        .get(server.task_url(queued_task["id"].as_str().unwrap()))
        .bearer_auth("alice-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    let retained: Value = response.json().await.unwrap();
    assert!(matches!(
        retained["status"]["state"].as_str(),
        Some("submitted" | "working")
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let roots_before_replay = coordinator.snapshot().await.unwrap().roots.len();
    let (replayed, replayed_source) =
        send_task(&server, &client, "completed-before-replacement").await;
    assert_eq!(replayed["id"], completed_task["id"]);
    assert_eq!(replayed_source, completed_source);
    assert_eq!(
        replayed["metadata"]["acteon_governed_execution"]["binding_digest"],
        old_digest
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        coordinator.snapshot().await.unwrap().roots.len(),
        roots_before_replay
    );
    let mut altered = message("completed-before-replacement");
    altered["message"]["parts"][0]["text"] = json!("Different work under the same ID");
    let response = client
        .post(server.endpoint())
        .bearer_auth("alice-secret")
        .json(&altered)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    assert_eq!(
        coordinator.snapshot().await.unwrap().roots.len(),
        roots_before_replay
    );
    let response = client
        .post(format!(
            "{}/stop",
            server.task_url(queued_task["id"].as_str().unwrap())
        ))
        .bearer_auth("alice-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    let stopped: Value = response.json().await.unwrap();
    assert_eq!(stopped["task"]["id"], queued_task["id"]);
    assert_eq!(stopped["future_starts_blocked"], true);

    let (new_task, _) = send_task(&server, &client, "after-replacement").await;
    assert_ne!(new_task["id"], completed_task["id"]);
    assert_ne!(new_task["id"], queued_task["id"]);
    assert_eq!(
        new_task["metadata"]["acteon_governed_execution"]["binding_digest"],
        *new_digest
    );
    let new_completed = await_completed(&server, &client, new_task["id"].as_str().unwrap()).await;
    assert_eq!(new_completed["id"], new_task["id"]);
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    server.remove_service_and_retain_current(new_digest);
    server.ready(&client).await;
    for (message_id, expected_id) in [
        ("completed-before-replacement", &completed_task["id"]),
        ("after-replacement", &new_task["id"]),
    ] {
        let (replayed, _) = send_task(&server, &client, message_id).await;
        assert_eq!(&replayed["id"], expected_id);
    }
    let response = client
        .post(server.endpoint())
        .bearer_auth("alice-secret")
        .json(&message("fresh-after-removal"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    server.restart(true);
    server.ready(&client).await;
    let response = client
        .get(server.task_url(queued_task["id"].as_str().unwrap()))
        .bearer_auth("alice-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    assert!(matches!(
        response.json::<Value>().await.unwrap()["status"]["state"].as_str(),
        Some("submitted" | "working")
    ));
    let recovered = await_completed(&server, &client, completed_task["id"].as_str().unwrap()).await;
    assert_eq!(recovered["artifacts"], completed["artifacts"]);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    webhook_task.abort();
}

#[tokio::test]
async fn invalid_messages_are_bad_requests_and_missing_work_is_not_found() {
    let (url, calls, webhook_task) = webhook().await;
    let mut server = Server::start(&url, Some("notifier-secret"), "incident");
    let client = reqwest::Client::new();
    server.ready(&client).await;
    for request in [
        json!({"message":{"role":"user","messageId":"empty","parts":[]}}),
        json!({"message":{"role":"user","messageId":"continuation","taskId":"unaccepted","parts":[{"kind":"text","text":"Continue"}]}}),
    ] {
        let response = client
            .post(server.endpoint())
            .bearer_auth("alice-secret")
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(
            response.json::<Value>().await.unwrap()["error"],
            "invalid_agent_service_request"
        );
    }
    let response = client
        .get(server.task_url(&uuid::Uuid::new_v4().to_string()))
        .bearer_auth("alice-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    assert_eq!(
        response.json::<Value>().await.unwrap()["error"],
        "service_task_unavailable"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    webhook_task.abort();
}

#[cfg(feature = "redis")]
async fn response_lost_webhook() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut headers = Vec::new();
            while !headers.ends_with(b"\r\n\r\n") {
                headers.push(socket.read_u8().await.unwrap());
                assert!(headers.len() <= 16_384);
            }
            let headers = String::from_utf8(headers).unwrap();
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap();
            assert!(length <= 2 * 1024 * 1024);
            let mut body = vec![0; length];
            socket.read_exact(&mut body).await.unwrap();
            assert!(
                serde_json::from_slice::<Value>(&body)
                    .unwrap()
                    .pointer("/payload/a2a_message")
                    .is_some()
            );
            counter.fetch_add(1, Ordering::SeqCst);
            // The operation was received, but the caller never gets an ACK.
            drop(socket);
        }
    });
    (format!("http://{address}/incident"), calls, task)
}

#[tokio::test]
#[cfg(feature = "redis")]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; isolated prefix and actual lost HTTP response"]
async fn redis_restart_preserves_uncertain_delivery_without_resend_or_released_capacity() {
    use acteon_governance::{AttemptStatus, AuthorityCoordinator};
    let (url, calls, webhook_task) = response_lost_webhook().await;
    let (backend, settings) = redis_state();
    let store: Arc<dyn acteon_state::StateStore> =
        Arc::new(acteon_state_redis::RedisStateStore::new(&settings).unwrap());
    let mut server = Server::configured(
        &url,
        Some("notifier-secret"),
        "incident",
        true,
        "human",
        backend,
    );
    let client = reqwest::Client::new();
    server.ready(&client).await;
    let (accepted, _) = send_task(&server, &client, "response-lost").await;
    let coordinator = AuthorityCoordinator::connect(store, "prod", "acme")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let state = coordinator.snapshot().await.unwrap();
            if state
                .starts
                .values()
                .any(|start| start.status == AttemptStatus::Uncertain)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let response = client
        .post(format!(
            "{}/stop",
            server.task_url(accepted["id"].as_str().unwrap())
        ))
        .bearer_auth("alice-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let stopped: Value = response.json().await.unwrap();
    assert_eq!(stopped["future_starts_blocked"], true);
    assert_eq!(stopped["task"]["status"]["state"], "working");
    server.restart(true);
    server.ready(&client).await;
    // Observe multiple driver ticks after restart; neither reads nor replay resend.
    for _ in 0..12 {
        let response = client
            .get(server.task_url(accepted["id"].as_str().unwrap()))
            .bearer_auth("alice-secret")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let task = response.json::<Value>().await.unwrap();
        assert_eq!(task["status"]["state"], "working");
        assert_eq!(task["artifacts"].as_array().map_or(0, Vec::len), 0);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (replayed, _) = send_task(&server, &client, "response-lost").await;
    assert_eq!(replayed["id"], accepted["id"]);
    let retained = coordinator.snapshot().await.unwrap();
    assert_eq!(retained.starts.len(), 1);
    assert_eq!(
        retained.starts.values().next().unwrap().status,
        AttemptStatus::Uncertain
    );
    assert_eq!(retained.roots.len(), 2);
    assert!(retained.roots[accepted["id"].as_str().unwrap()].cancelled);
    assert!(
        retained
            .roots
            .values()
            .all(|root| root.active_attempts == 1)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    webhook_task.abort();
}

#[tokio::test]
#[cfg(feature = "redis")]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; isolated corrupt projection contract"]
async fn redis_corrupt_projection_reports_unavailable_without_hiding_or_starting_work() {
    let (url, calls, webhook_task) = webhook().await;
    let (backend, settings) = redis_state();
    let store: Arc<dyn acteon_state::StateStore> =
        Arc::new(acteon_state_redis::RedisStateStore::new(&settings).unwrap());
    let mut server = Server::configured(
        &url,
        Some("notifier-secret"),
        "incident",
        false,
        "human",
        backend,
    );
    let client = reqwest::Client::new();
    server.ready(&client).await;
    let (accepted, _) = send_task(&server, &client, "corrupt-projection").await;
    let id = accepted["id"].as_str().unwrap();
    let key = acteon_state::StateKey::new("prod", "acme", acteon_state::KeyKind::A2aTask, id);
    let original = store.get(&key).await.unwrap().unwrap();
    store
        .set(&key, "private backend corruption", None)
        .await
        .unwrap();
    let response = client
        .get(server.task_url(id))
        .bearer_auth("alice-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"error":"agent_services_unavailable"})
    );
    store.set(&key, &original, None).await.unwrap();
    let response = client
        .get(server.task_url(id))
        .bearer_auth("alice-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["status"]["state"],
        "submitted"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    webhook_task.abort();
}

#[tokio::test]
async fn native_sdk_observes_exact_agent_job_and_cors_exposes_host_receipt() {
    let (url, calls, webhook_task) = webhook().await;
    let mut server = Server::configured(
        &url,
        Some("notifier-secret"),
        "incident",
        false,
        "agent",
        json!({"backend":"memory"}),
    );
    let http = reqwest::Client::new();
    server.ready(&http).await;
    let client = acteon_client::ActeonClient::builder(&server.url)
        .api_key("alice-secret")
        .build()
        .unwrap();
    let message = acteon_core::TaskMessage::text(
        "sdk-original",
        acteon_core::TaskRole::User,
        "Notify incident owner",
    );
    let receipt = client
        .agent_service_send_message("prod", "acme", "notifier", &message)
        .await
        .unwrap();
    let saved = serde_json::to_string(&receipt).unwrap();
    let mut restored: acteon_client::AgentServiceReceipt = serde_json::from_str(&saved).unwrap();
    restored.task.id = "model-mutated-task-data".into();
    let task = client.agent_service_get_task(&restored).await.unwrap();
    assert_eq!(task.id, receipt.task_id());
    assert_eq!(task.status.state, acteon_core::TaskState::Submitted);
    let stopped = client.agent_service_stop_task(&restored).await.unwrap();
    assert!(stopped.future_starts_blocked);
    assert_eq!(stopped.task.id, receipt.task_id());
    assert_eq!(stopped.task.status.state, acteon_core::TaskState::Submitted);
    let repeated = client.agent_service_stop_task(&restored).await.unwrap();
    assert!(repeated.future_starts_blocked);
    assert_eq!(repeated.task.id, receipt.task_id());
    let response = http
        .post(server.endpoint())
        .bearer_auth("alice-secret")
        .header("origin", "https://console.example")
        .json(&serde_json::json!({"message":message}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        "https://console.example"
    );
    let exposed = response.headers()["access-control-expose-headers"]
        .to_str()
        .unwrap();
    assert!(exposed.contains("x-acteon-agent-source-context"));
    assert!(exposed.contains("a2a-version"));
    assert_eq!(
        response.headers()["x-acteon-agent-source-context"],
        receipt.source_context()
    );
    let preflight = http
        .request(reqwest::Method::OPTIONS, server.task_url(receipt.task_id()))
        .header("origin", "https://console.example")
        .header("access-control-request-method", "GET")
        .header(
            "access-control-request-headers",
            "authorization,x-acteon-agent-source-context",
        )
        .send()
        .await
        .unwrap();
    assert!(preflight.status().is_success());
    assert_eq!(
        preflight.headers()["access-control-allow-origin"],
        "https://console.example"
    );
    let outsider = http
        .get(server.task_url(receipt.task_id()))
        .bearer_auth("observer-secret")
        .header("x-acteon-agent-source-context", receipt.source_context())
        .send()
        .await
        .unwrap();
    assert_eq!(
        outsider.status(),
        404,
        "opaque reference does not replace original private authentication"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    webhook_task.abort();
}

#[tokio::test]
async fn requester_stop_requires_original_job_provenance_and_never_starts_effects() {
    let (url, calls, webhook_task) = webhook().await;
    let mut server = Server::configured(
        &url,
        Some("notifier-secret"),
        "incident",
        false,
        "agent",
        json!({"backend":"memory"}),
    );
    let client = reqwest::Client::new();
    server.ready(&client).await;
    let (task, source) = send_task(&server, &client, "stop-original").await;
    let (_, other_source) = send_task(&server, &client, "stop-other").await;
    let url = format!("{}/stop", server.task_url(task["id"].as_str().unwrap()));
    for (token, context, expected) in [
        ("alice-secret", None, 404),
        ("alice-secret", Some(other_source.as_str()), 404),
        ("observer-secret", Some(source.as_str()), 404),
        ("notifier-secret", Some(source.as_str()), 404),
        ("alice-secret", Some(source.as_str()), 200),
        ("alice-secret", Some(source.as_str()), 200),
    ] {
        let mut request = client.post(&url).bearer_auth(token);
        if let Some(context) = context {
            request = request.header("x-acteon-agent-source-context", context);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), expected);
        assert_eq!(response.headers()["cache-control"], "no-store");
        if expected == 200 {
            let body: Value = response.json().await.unwrap();
            assert_eq!(body["future_starts_blocked"], true);
            assert_eq!(body["task"]["id"], task["id"]);
            assert_eq!(body["task"]["status"]["state"], "submitted");
        }
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    webhook_task.abort();
}

#[tokio::test]
#[cfg(feature = "redis")]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; isolated durable requester stop contract"]
async fn redis_restart_does_not_start_stopped_queued_service_work() {
    let (url, calls, webhook_task) = webhook().await;
    let (backend, settings) = redis_state();
    let store: Arc<dyn acteon_state::StateStore> =
        Arc::new(acteon_state_redis::RedisStateStore::new(&settings).unwrap());
    let mut server = Server::configured(
        &url,
        Some("notifier-secret"),
        "incident",
        false,
        "agent",
        backend,
    );
    let client = reqwest::Client::new();
    server.ready(&client).await;
    let (task, source) = send_task(&server, &client, "stopped-before-restart").await;
    let stop_url = format!("{}/stop", server.task_url(task["id"].as_str().unwrap()));
    let response = client
        .post(&stop_url)
        .bearer_auth("alice-secret")
        .header("x-acteon-agent-source-context", &source)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let stopped: Value = response.json().await.unwrap();
    assert_eq!(stopped["future_starts_blocked"], true);
    server.restart(true);
    server.ready(&client).await;
    // Observe twelve independent recovery ticks and repeat the same control.
    for _ in 0..12 {
        let response = client
            .post(&stop_url)
            .bearer_auth("alice-secret")
            .header("x-acteon-agent-source-context", &source)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let stopped: Value = response.json().await.unwrap();
        assert_eq!(stopped["future_starts_blocked"], true);
        assert_eq!(stopped["task"]["id"], task["id"]);
        assert_eq!(stopped["task"]["status"]["state"], "submitted");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let coordinator = acteon_governance::AuthorityCoordinator::connect(store, "prod", "acme")
        .await
        .unwrap();
    let snapshot = coordinator.snapshot().await.unwrap();
    assert!(snapshot.roots[task["id"].as_str().unwrap()].cancelled);
    assert!(snapshot.starts.is_empty());
    assert!(
        snapshot
            .roots
            .values()
            .all(|root| root.active_attempts == 0 && root.spent_units == 0)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    webhook_task.abort();
}

#[tokio::test]
#[cfg(feature = "redis")]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; isolated registry retirement contract"]
async fn redis_registry_retirement_blocks_admission_and_republication_without_hiding_history() {
    use acteon_governance::{AuthorityChange, AuthorityCoordinator};
    let (url, calls, webhook_task) = webhook().await;
    let (backend, settings) = redis_state();
    let store: Arc<dyn acteon_state::StateStore> =
        Arc::new(acteon_state_redis::RedisStateStore::new(&settings).unwrap());
    let mut server = Server::configured(
        &url,
        Some("notifier-secret"),
        "incident",
        false,
        "agent",
        backend,
    );
    let client = reqwest::Client::new();
    server.ready(&client).await;
    let coordinator = AuthorityCoordinator::connect(store, "prod", "acme")
        .await
        .unwrap();
    let before = coordinator.snapshot().await.unwrap();
    let record = &before.agent_registry["notifier"];
    assert_eq!(record.qualification.revision, 1);
    assert!(!record.retired);
    let (task, source) = send_task(&server, &client, "registry-retired-job").await;
    coordinator
        .change(
            "operator-registry-retirement",
            AuthorityChange::RetireAgentRegistry {
                agent: record.qualification.agent.clone(),
                expected_revision: 1,
            },
            "operator",
            "reviewed registry withdrawal",
        )
        .await
        .unwrap();
    let before_denied = coordinator.snapshot().await.unwrap();
    let response = client
        .post(server.endpoint())
        .bearer_auth("alice-secret")
        .json(&message("new-after-retirement"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let after_denied = coordinator.snapshot().await.unwrap();
    assert_eq!(before_denied.roots.len(), after_denied.roots.len());
    assert!(after_denied.starts.is_empty());
    let response = client
        .get(server.task_url(task["id"].as_str().unwrap()))
        .bearer_auth("alice-secret")
        .header("x-acteon-agent-source-context", source)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.json::<Value>().await.unwrap()["id"], task["id"]);
    server.restart(true);
    server.rejected_startup().await;
    assert!(
        server
            .log()
            .contains("registry qualification retired or replaced"),
        "{}",
        server.log()
    );
    assert!(coordinator.snapshot().await.unwrap().agent_registry["notifier"].retired);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    webhook_task.abort();
}
