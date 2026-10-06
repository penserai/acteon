//! Real binary and middleware, independently authenticated caller and recipient.
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
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
        let _ = fs::remove_dir_all(&self.directory);
    }
}
impl Server {
    #[allow(clippy::too_many_lines)] // One isolated binary deployment fixture.
    fn start(webhook: &str, worker_secret: Option<&str>, worker_grant: &str) -> Self {
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
        let human = json!({"id":"alice","kind":"human"});
        let worker = json!({"id":"agent/notifier","kind":"agent"});
        let route = json!({"provider":"incident","action_type":"execute"});
        let configuration = json!({
            "server":{"host":"127.0.0.1","port":port},
            "state":{"backend":"memory"}, "ui":{"enabled":false},
            "auth":{"enabled":true,"config_path":"auth.toml","watch":false,
                "authority":{"namespace":"auth-control","tenant":"deployment","source_id":"service-auth","bootstrap":true}},
            "providers":[{"name":"incident","type":"webhook","url":webhook,"internal_hosts":["127.0.0.1"]}],
            "execution_authority":{"scopes":[{
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
principal = {{id="alice",kind="human"}}
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
        Self {
            process: command.spawn().unwrap(),
            directory,
            url: format!("http://127.0.0.1:{port}"),
        }
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
    for (secret, grant, expected) in [
        (None, "incident", "recipient authentication unavailable"),
        (
            Some("alice-secret"),
            "incident",
            "recipient authentication differs from configured agent",
        ),
        (
            Some("notifier-secret"),
            "unrelated",
            "original scope credential binding is no longer eligible",
        ),
    ] {
        let mut server = Server::start(&url, secret, grant);
        server.rejected_startup().await;
        assert!(server.log().contains(expected), "{}", server.log());
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
