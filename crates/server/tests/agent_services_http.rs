//! Real binary and middleware, independently authenticated caller and recipient.
#![recursion_limit = "256"]
use acteon_core::{AgentCard, AgentCardInterface, Skill};
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
