//! Real binary and middleware, independently authenticated caller and recipient.
#![recursion_limit = "256"]
use acteon_core::{AgentCard, AgentCardInterface, PauseKind, Skill, TaskState};
#[cfg(any(feature = "redis", feature = "postgres"))]
use acteon_gateway::{TaskEngine, TaskScope};
#[cfg(any(feature = "redis", feature = "postgres"))]
use acteon_state::{KeyKind, StateKey, StateStore};
use axum::{Json, Router, routing::post};
#[cfg(any(feature = "redis", feature = "postgres"))]
use axum::{
    body::{Body, Bytes, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, Method, Response, StatusCode, header},
    response::IntoResponse,
    routing::any,
};
#[cfg(feature = "postgres")]
use futures::FutureExt;
use serde_json::{Value, json};
#[cfg(any(feature = "redis", feature = "postgres"))]
use std::sync::{Mutex, atomic::AtomicBool};
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
            &json!({"backend":"memory"}),
        )
    }
    fn configured(
        webhook: &str,
        worker_secret: Option<&str>,
        worker_grant: &str,
        driver: bool,
        source_kind: &str,
        state: &Value,
    ) -> Self {
        Self::configured_inner(
            webhook,
            worker_secret,
            worker_grant,
            driver,
            source_kind,
            state,
            None,
        )
    }
    fn configured_authorization(webhook: &str, verifier: &str, driver: bool) -> Self {
        Self::configured_inner(
            webhook,
            Some("notifier-secret"),
            "incident",
            driver,
            "human",
            &json!({"backend":"memory"}),
            Some(verifier),
        )
    }
    #[allow(clippy::too_many_lines)] // One isolated binary deployment fixture.
    fn configured_inner(
        webhook: &str,
        worker_secret: Option<&str>,
        worker_grant: &str,
        driver: bool,
        source_kind: &str,
        state: &Value,
        verifier: Option<&str>,
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
        let mut configuration = json!({
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
        if let Some(endpoint) = verifier {
            configuration["execution_authority"]["authorization_verifiers"] = json!([{
                "id":"city-workload","revision":3,"endpoint":endpoint,
                "credential_env":"ACTEON_TEST_AUTH_VERIFIER","timeout_ms":2000,
                "internal_hosts":["127.0.0.1"]
            }]);
            configuration["execution_authority"]["scopes"][0]["agent_services"][0]["authorization"] = json!({
                "verifier_id":"city-workload","verifier_revision":3,
                "credential_authority":"city-identity","audience":"incident-api",
                "required_scopes":["incident.resolve"],"challenge_ttl_ms":300_000
            });
            // The provider consumes the root's only unit before authorization
            // resolves; rechecking an existing effect must not demand a new unit.
            configuration["execution_authority"]["scopes"][0]["root_max_units"] = json!(1);
        }
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
            .env("ACTEON_TEST_AUTH_VERIFIER", "verifier-secret")
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
                    .is_ok_and(|response| response.status().is_success())
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

async fn authorization_verifier() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    use sha2::Digest;
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let app = Router::new().route("/verify", post(move |headers: axum::http::HeaderMap, Json(body): Json<Value>| {
        let counter = counter.clone();
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            assert_eq!(headers["authorization"], "Bearer verifier-secret");
            assert_eq!(body["requirement"]["verifierId"], "city-workload");
            assert_eq!(body["requirement"]["recipient"]["id"], "agent/notifier");
            let request_id = body["requirement"]["authorizationRequestId"].as_str().unwrap();
            let now = chrono::Utc::now();
            Json(json!({
                "schema":1,"taskId":body["taskId"],"challengeId":body["challengeId"],
                "authorizationRequestDigest":hex::encode(sha2::Sha256::digest(request_id.as_bytes())),
                "requirementDigest":body["requirementDigest"],
                "decisionId":"decision-42","subject":{"id":"agent/notifier","kind":"agent"},
                "verifiedAt":now,"validUntil":now+chrono::Duration::minutes(5)
            }))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}/verify"), calls, task)
}

#[cfg(any(feature = "redis", feature = "postgres"))]
async fn controlled_peer_authorization_verifier() -> (
    String,
    Arc<AtomicUsize>,
    Arc<AtomicBool>,
    tokio::task::JoinHandle<()>,
) {
    use sha2::Digest;
    let calls = Arc::new(AtomicUsize::new(0));
    let allowed = Arc::new(AtomicBool::new(true));
    let counter = calls.clone();
    let decision = allowed.clone();
    let app = Router::new().route(
        "/verify",
        post(
            move |headers: axum::http::HeaderMap, Json(body): Json<Value>| {
                let counter = counter.clone();
                let decision = decision.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(headers["authorization"], "Bearer verifier-secret");
                    assert_eq!(body["requirement"]["verifierId"], "city-workload");
                    assert_eq!(
                        body["requirement"]["recipient"]["id"],
                        "agent/resolver"
                    );
                    if !decision.load(Ordering::SeqCst) {
                        return (
                            StatusCode::FORBIDDEN,
                            Json(json!({"error":"authorization_revoked"})),
                        );
                    }
                    let request_id = body["requirement"]["authorizationRequestId"]
                        .as_str()
                        .unwrap();
                    let now = chrono::Utc::now();
                    (
                        StatusCode::OK,
                        Json(json!({
                            "schema":1,"taskId":body["taskId"],"challengeId":body["challengeId"],
                            "authorizationRequestDigest":hex::encode(sha2::Sha256::digest(request_id.as_bytes())),
                            "requirementDigest":body["requirementDigest"],
                            "decisionId":"peer-decision-42",
                            "subject":{"id":"agent/resolver","kind":"agent"},
                            "verifiedAt":now,"validUntil":now+chrono::Duration::minutes(5)
                        })),
                    )
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}/verify"), calls, allowed, task)
}

async fn pausing_webhook() -> (
    String,
    Arc<AtomicUsize>,
    Arc<tokio::sync::Semaphore>,
    Arc<tokio::sync::Semaphore>,
    tokio::task::JoinHandle<()>,
) {
    let calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let counter = calls.clone();
    let entered_handler = entered.clone();
    let release_handler = release.clone();
    let app = Router::new().route(
        "/incident",
        post(move || {
            let counter = counter.clone();
            let entered = entered_handler.clone();
            let release = release_handler.clone();
            async move {
                if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                    entered.add_permits(1);
                    release.acquire().await.unwrap().forget();
                }
                Json(json!({"delivered":true}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (
        format!("http://{address}/incident"),
        calls,
        entered,
        release,
        task,
    )
}
fn message(id: &str) -> Value {
    json!({"message":{"role":"user","messageId":id,"parts":[{"kind":"text","text":"Notify the incident owner"}]}})
}

#[tokio::test]
async fn hosted_authorization_uses_recipient_to_open_and_original_requester_to_resolve() {
    let (webhook_url, provider_calls, entered, release, webhook_task) = pausing_webhook().await;
    let (verifier_url, verifier_calls, verifier_task) = authorization_verifier().await;
    let mut server = Server::configured_authorization(&webhook_url, &verifier_url, true);
    let client = reqwest::Client::new();
    server.ready(&client).await;
    let (task, source) = send_task(&server, &client, "authorization-job").await;
    tokio::time::timeout(Duration::from_secs(10), entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let task_id = task["id"].as_str().unwrap();
    let base = server.task_url(task_id);
    let wrong_recipient = client
        .post(format!("{base}/authorization:request"))
        .bearer_auth("alice-secret")
        .header("a2a-version", "1.0")
        .json(&json!({"authorizationRequestId":"opaque-flow-42"}))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_recipient.status(), 404);
    let opened = client
        .post(format!("{base}/authorization:request"))
        .bearer_auth("notifier-secret")
        .header("a2a-version", "1.0")
        .json(&json!({"authorizationRequestId":"opaque-flow-42"}))
        .send()
        .await
        .unwrap();
    assert_eq!(opened.status(), 200, "{}", opened.text().await.unwrap());
    let opened: Value = opened.json().await.unwrap();
    assert_eq!(opened["status"]["state"], "auth_required");
    let challenge = opened["pendingApprovalId"].as_str().unwrap();
    let wrong_requester = client
        .post(format!("{base}/authorization:resolve"))
        .bearer_auth("notifier-secret")
        .header("a2a-version", "1.0")
        .header("x-acteon-agent-source-context", &source)
        .json(&json!({"challengeId":challenge}))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_requester.status(), 404);
    assert_eq!(verifier_calls.load(Ordering::SeqCst), 0);
    let resolved = client
        .post(format!("{base}/authorization:resolve"))
        .bearer_auth("alice-secret")
        .header("a2a-version", "1.0")
        .header("x-acteon-agent-source-context", source)
        .json(&json!({"challengeId":challenge}))
        .send()
        .await
        .unwrap();
    let resolved_status = resolved.status();
    let resolved_body = resolved.text().await.unwrap();
    assert_eq!(
        resolved_status,
        200,
        "{resolved_body}; verifier calls={}; log={}",
        verifier_calls.load(Ordering::SeqCst),
        fs::read_to_string(server.directory.join("server.log")).unwrap_or_default()
    );
    let resolved: Value = serde_json::from_str(&resolved_body).unwrap();
    assert_eq!(resolved["status"]["state"], "working");
    assert_eq!(verifier_calls.load(Ordering::SeqCst), 1);
    release.add_permits(1);
    let completed = await_completed(&server, &client, task_id).await;
    assert_eq!(completed["status"]["state"], "completed");
    assert_eq!(provider_calls.load(Ordering::SeqCst), 1);
    webhook_task.abort();
    verifier_task.abort();
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
        &json!({"backend":"memory"}),
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
        &json!({"backend":"memory"}),
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

#[cfg(any(feature = "redis", feature = "postgres"))]
struct PeerMeshServer {
    process: Child,
    directory: PathBuf,
    url: String,
}

#[cfg(any(feature = "redis", feature = "postgres"))]
impl Drop for PeerMeshServer {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[cfg(any(feature = "redis", feature = "postgres"))]
impl PeerMeshServer {
    #[allow(clippy::too_many_arguments)]
    fn start(
        own_port: u16,
        notifier_port: u16,
        resolver_endpoint_port: u16,
        webhook: &str,
        state: &Value,
        credential_hashes: &[String; 3],
        bootstrap: bool,
        driver: bool,
    ) -> Self {
        Self::start_inner(
            own_port,
            notifier_port,
            resolver_endpoint_port,
            webhook,
            state,
            credential_hashes,
            bootstrap,
            driver,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn start_authorization(
        own_port: u16,
        notifier_port: u16,
        resolver_endpoint_port: u16,
        webhook: &str,
        state: &Value,
        credential_hashes: &[String; 3],
        bootstrap: bool,
        driver: bool,
        verifier: &str,
    ) -> Self {
        Self::start_inner(
            own_port,
            notifier_port,
            resolver_endpoint_port,
            webhook,
            state,
            credential_hashes,
            bootstrap,
            driver,
            Some(verifier),
        )
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn start_inner(
        own_port: u16,
        notifier_port: u16,
        resolver_endpoint_port: u16,
        webhook: &str,
        state: &Value,
        credential_hashes: &[String; 3],
        bootstrap: bool,
        driver: bool,
        verifier: Option<&str>,
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
            "https://127.0.0.1:{resolver_endpoint_port}/a2a/prod/acme/agents/resolver/v1/message:send"
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
        let mut configuration = json!({
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
                "agent_driver":{"enabled":driver,"poll_interval_ms":100,"max_parallel":2,"scan_batch_size":8},
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
        if let Some(endpoint) = verifier {
            configuration["execution_authority"]["authorization_verifiers"] = json!([{
                "id":"city-workload","revision":3,"endpoint":endpoint,
                "credential_env":"ACTEON_TEST_AUTH_VERIFIER","timeout_ms":2000,
                "internal_hosts":["127.0.0.1"]
            }]);
            configuration["execution_authority"]["scopes"][0]["agent_services"][1]["authorization"] = json!({
                "verifier_id":"city-workload","verifier_revision":3,
                "credential_authority":"city-identity","audience":"incident-api",
                "required_scopes":["incident.resolve"],"challenge_ttl_ms":300_000
            });
        }
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
            .env("ACTEON_TEST_AUTH_VERIFIER", "verifier-secret")
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
                    .is_ok_and(|response| response.status().is_success())
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

#[cfg(any(feature = "redis", feature = "postgres"))]
struct ResponseLossProxyState {
    upstream: String,
    client: reqwest::Client,
    lose_next_stop_response: AtomicBool,
    lose_next_authorization_response: AtomicBool,
    stop_deliveries: AtomicUsize,
    authorization_deliveries: AtomicUsize,
    task_observations: AtomicUsize,
    conditional_task_observations: AtomicUsize,
    not_modified_observations: AtomicUsize,
    committed_stop: Mutex<Option<Value>>,
}

#[cfg(any(feature = "redis", feature = "postgres"))]
struct ResponseLossProxy {
    url: String,
    state: Arc<ResponseLossProxyState>,
    task: tokio::task::JoinHandle<()>,
    directory: PathBuf,
}

#[cfg(any(feature = "redis", feature = "postgres"))]
impl Drop for ResponseLossProxy {
    fn drop(&mut self) {
        self.task.abort();
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[cfg(any(feature = "redis", feature = "postgres"))]
impl ResponseLossProxy {
    async fn start(port: u16, upstream: String, lose_next_stop_response: bool) -> Self {
        let directory =
            std::env::temp_dir().join(format!("acteon-response-loss-{}", uuid::Uuid::new_v4()));
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
        let tls = acteon_crypto::tls::build_server_config(
            cert_path.to_str().unwrap(),
            key_path.to_str().unwrap(),
            None,
            acteon_crypto::tls::MinTlsVersion::Tls12,
        )
        .unwrap();
        let state = Arc::new(ResponseLossProxyState {
            upstream,
            client: reqwest::Client::builder()
                .danger_accept_invalid_certs(true)
                .build()
                .unwrap(),
            lose_next_stop_response: AtomicBool::new(lose_next_stop_response),
            lose_next_authorization_response: AtomicBool::new(false),
            stop_deliveries: AtomicUsize::new(0),
            authorization_deliveries: AtomicUsize::new(0),
            task_observations: AtomicUsize::new(0),
            conditional_task_observations: AtomicUsize::new(0),
            not_modified_observations: AtomicUsize::new(0),
            committed_stop: Mutex::new(None),
        });
        let app = Router::new()
            .fallback(any(proxy_peer_request))
            .with_state(Arc::clone(&state));
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .unwrap();
        let task = tokio::spawn(serve_test_tls(listener, app, tls));
        Self {
            url: format!("https://127.0.0.1:{port}"),
            state,
            task,
            directory,
        }
    }

    async fn ready(&self, client: &reqwest::Client) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if client
                    .get(format!("{}/health", self.url))
                    .send()
                    .await
                    .is_ok_and(|response| response.status().is_success())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
    }
}

#[cfg(any(feature = "redis", feature = "postgres"))]
#[allow(clippy::too_many_lines)]
async fn proxy_peer_request(
    State(state): State<Arc<ResponseLossProxyState>>,
    request: Request,
) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let path = parts
        .uri
        .path_and_query()
        .map_or_else(|| "/".into(), ToString::to_string);
    let is_stop = parts.method == Method::POST && parts.uri.path().ends_with("/stop");
    let is_authorization =
        parts.method == Method::POST && parts.uri.path().ends_with("/authorization:resolve");
    let is_task_observation = parts.method == Method::GET
        && parts.uri.path().contains("/tasks/")
        && !parts.uri.path().ends_with("/stop");
    let is_conditional_task_observation =
        is_task_observation && parts.headers.contains_key(header::IF_NONE_MATCH);
    let Ok(body) = to_bytes(body, 2 * 1024 * 1024).await else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let mut headers = HeaderMap::new();
    for (name, value) in &parts.headers {
        if !matches!(
            name,
            &header::HOST | &header::CONTENT_LENGTH | &header::CONNECTION
        ) {
            headers.append(name, value.clone());
        }
    }
    let Ok(upstream) = state
        .client
        .request(parts.method, format!("{}{path}", state.upstream))
        .headers(headers)
        .body(body)
        .send()
        .await
    else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    let status = upstream.status();
    let upstream_headers = upstream.headers().clone();
    let Ok(body) = upstream.bytes().await else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    if is_task_observation {
        state.task_observations.fetch_add(1, Ordering::SeqCst);
        if is_conditional_task_observation {
            state
                .conditional_task_observations
                .fetch_add(1, Ordering::SeqCst);
        }
        if status == StatusCode::NOT_MODIFIED {
            state
                .not_modified_observations
                .fetch_add(1, Ordering::SeqCst);
        }
    }
    if is_stop {
        state.stop_deliveries.fetch_add(1, Ordering::SeqCst);
        if status.is_success()
            && let Ok(committed) = serde_json::from_slice(&body)
        {
            *state.committed_stop.lock().unwrap() = Some(committed);
        }
        if state
            .lose_next_stop_response
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let failed = futures::stream::once(async {
                Err::<Bytes, std::io::Error>(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "injected response loss after target commit",
                ))
            });
            return response_with_forwarded_headers(
                status,
                &upstream_headers,
                Body::from_stream(failed),
            );
        }
    }
    if is_authorization {
        state
            .authorization_deliveries
            .fetch_add(1, Ordering::SeqCst);
        if state
            .lose_next_authorization_response
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let failed = futures::stream::once(async {
                Err::<Bytes, std::io::Error>(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "injected authorization response loss after target commit",
                ))
            });
            return response_with_forwarded_headers(
                status,
                &upstream_headers,
                Body::from_stream(failed),
            );
        }
    }
    response_with_forwarded_headers(status, &upstream_headers, Body::from(body))
}

#[cfg(any(feature = "redis", feature = "postgres"))]
fn response_with_forwarded_headers(
    status: StatusCode,
    upstream: &HeaderMap,
    body: Body,
) -> Response<Body> {
    let mut response = Response::builder().status(status);
    for name in [
        header::CONTENT_TYPE,
        header::ETAG,
        header::HeaderName::from_static("a2a-version"),
        header::HeaderName::from_static("x-acteon-agent-source-context"),
    ] {
        for value in upstream.get_all(&name) {
            response = response.header(&name, value);
        }
    }
    response.body(body).unwrap()
}

#[cfg(any(feature = "redis", feature = "postgres"))]
async fn serve_test_tls(
    listener: tokio::net::TcpListener,
    app: Router,
    tls: Arc<rustls::ServerConfig>,
) {
    use tower::ServiceExt;

    let acceptor = tokio_rustls::TlsAcceptor::from(tls);
    loop {
        let (stream, _) = listener.accept().await.unwrap();
        let acceptor = acceptor.clone();
        let app = app.clone();
        tokio::spawn(async move {
            let Ok(stream) = acceptor.accept(stream).await else {
                return;
            };
            let service = hyper::service::service_fn(
                move |request: hyper::Request<hyper::body::Incoming>| app.clone().oneshot(request),
            );
            let _ =
                hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new())
                    .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                    .await;
        });
    }
}

#[cfg(any(feature = "redis", feature = "postgres"))]
fn reserve_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[cfg(any(feature = "redis", feature = "postgres"))]
async fn publish_peer_cards(store: &dyn StateStore, server_config: &toml::Value) {
    for index in 0..2 {
        let card: AgentCard = serde_json::from_value(
            serde_json::to_value(
                &server_config["execution_authority"]["scopes"][0]["agent_services"][index]["card"],
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
}

#[tokio::test]
#[cfg(feature = "redis")]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; two real HTTPS servers sharing Redis"]
#[allow(clippy::too_many_lines)]
async fn redis_two_server_peer_cancel_survives_source_restart_as_a_durable_restriction() {
    let (state, redis_config) = redis_state();
    let store = Arc::new(acteon_state_redis::RedisStateStore::new(&redis_config).unwrap());
    two_server_peer_cancel_restart_contract(state, store).await;
}

#[tokio::test]
#[cfg(feature = "postgres")]
#[ignore = "requires DATABASE_URL; two real HTTPS servers sharing PostgreSQL"]
async fn postgres_two_server_peer_cancel_survives_source_restart_as_a_durable_restriction() {
    let config = acteon_state_postgres::PostgresConfig {
        url: std::env::var("DATABASE_URL").expect("set DATABASE_URL"),
        table_prefix: format!("peer_lifecycle_{}_", uuid::Uuid::new_v4().simple()),
        ..Default::default()
    };
    let state = json!({
        "backend":"postgres",
        "url":config.url,
        "prefix":config.table_prefix,
    });
    let store = Arc::new(
        acteon_state_postgres::PostgresStateStore::new(config.clone())
            .await
            .unwrap(),
    );
    let contract =
        std::panic::AssertUnwindSafe(two_server_peer_cancel_restart_contract(state, store))
            .catch_unwind()
            .await;
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
    if let Err(payload) = contract {
        std::panic::resume_unwind(payload);
    }
}

#[tokio::test]
#[cfg(feature = "redis")]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; two real HTTPS servers sharing Redis"]
async fn redis_two_server_peer_continuation_survives_source_restart() {
    let (state, redis_config) = redis_state();
    let store = Arc::new(acteon_state_redis::RedisStateStore::new(&redis_config).unwrap());
    two_server_peer_continuation_contract(state, store).await;
}

#[tokio::test]
#[cfg(feature = "postgres")]
#[ignore = "requires DATABASE_URL; two real HTTPS servers sharing PostgreSQL"]
async fn postgres_two_server_peer_continuation_survives_source_restart() {
    let config = acteon_state_postgres::PostgresConfig {
        url: std::env::var("DATABASE_URL").expect("set DATABASE_URL"),
        table_prefix: format!("peer_continue_{}_", uuid::Uuid::new_v4().simple()),
        ..Default::default()
    };
    let state = json!({"backend":"postgres","url":config.url,"prefix":config.table_prefix});
    let store = Arc::new(
        acteon_state_postgres::PostgresStateStore::new(config.clone())
            .await
            .unwrap(),
    );
    let contract =
        std::panic::AssertUnwindSafe(two_server_peer_continuation_contract(state, store))
            .catch_unwind()
            .await;
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
    if let Err(payload) = contract {
        std::panic::resume_unwind(payload);
    }
}

#[tokio::test]
#[cfg(feature = "redis")]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; two authenticated HTTPS servers and verifier"]
async fn redis_two_server_peer_authorization_survives_restart_denial_revocation_and_response_loss()
{
    let (state, redis_config) = redis_state();
    let store = Arc::new(acteon_state_redis::RedisStateStore::new(&redis_config).unwrap());
    two_server_peer_authorization_contract(state, store).await;
}

#[tokio::test]
#[cfg(feature = "postgres")]
#[ignore = "requires DATABASE_URL; two authenticated HTTPS servers and verifier"]
async fn postgres_two_server_peer_authorization_survives_restart_denial_revocation_and_response_loss()
 {
    let config = acteon_state_postgres::PostgresConfig {
        url: std::env::var("DATABASE_URL").expect("set DATABASE_URL"),
        table_prefix: format!("peer_authorize_{}_", uuid::Uuid::new_v4().simple()),
        ..Default::default()
    };
    let state = json!({"backend":"postgres","url":config.url,"prefix":config.table_prefix});
    let store = Arc::new(
        acteon_state_postgres::PostgresStateStore::new(config.clone())
            .await
            .unwrap(),
    );
    let contract =
        std::panic::AssertUnwindSafe(two_server_peer_authorization_contract(state, store))
            .catch_unwind()
            .await;
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
    if let Err(payload) = contract {
        std::panic::resume_unwind(payload);
    }
}

#[cfg(any(feature = "redis", feature = "postgres"))]
#[allow(clippy::too_many_lines)]
async fn two_server_peer_authorization_contract(state: Value, store: Arc<dyn StateStore>) {
    let (webhook_url, provider_calls, webhook_task) = webhook().await;
    let (verifier_url, verifier_calls, verifier_allowed, verifier_task) =
        controlled_peer_authorization_verifier().await;
    let notifier_port = reserve_port();
    let resolver_port = reserve_port();
    let proxy_port = reserve_port();
    let credential_hashes = [
        acteon_server::auth::api_key::hash_api_key("alice-secret"),
        acteon_server::auth::api_key::hash_api_key("notifier-secret"),
        acteon_server::auth::api_key::hash_api_key("resolver-secret"),
    ];
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();
    let mut resolver = PeerMeshServer::start_authorization(
        resolver_port,
        notifier_port,
        proxy_port,
        &webhook_url,
        &state,
        &credential_hashes,
        true,
        false,
        &verifier_url,
    );
    resolver.ready(&client).await;
    let proxy = ResponseLossProxy::start(proxy_port, resolver.url.clone(), false).await;
    proxy.ready(&client).await;
    let mut notifier = PeerMeshServer::start_authorization(
        notifier_port,
        notifier_port,
        proxy_port,
        &webhook_url,
        &state,
        &credential_hashes,
        true,
        false,
        &verifier_url,
    );
    notifier.ready(&client).await;
    let config: toml::Value =
        toml::from_str(&fs::read_to_string(resolver.directory.join("acteon.toml")).unwrap())
            .unwrap();
    publish_peer_cards(store.as_ref(), &config).await;

    let root_response = client
        .post(format!(
            "{}/a2a/prod/acme/agents/notifier/v1/message:send",
            notifier.url
        ))
        .bearer_auth("alice-secret")
        .json(&message("authorize-root"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        root_response.status(),
        200,
        "{}",
        root_response.text().await.unwrap()
    );
    let root: Value = root_response.json().await.unwrap();
    let peer_base = format!(
        "{}/a2a/prod/acme/agents/notifier/v1/tasks/{}/peers",
        notifier.url,
        root["id"].as_str().unwrap()
    );
    let engine = TaskEngine::new(store.clone());
    let scope = TaskScope::new("prod", "acme");

    let denied_send = client
        .post(format!("{peer_base}/resolver/resolve/message:send"))
        .bearer_auth("notifier-secret")
        .json(&message("authorize-denied-child"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied_send.status(), 200);
    let denied_peer: Value = denied_send.json().await.unwrap();
    let denied_remote = denied_peer["status"]["task"]["id"].as_str().unwrap();
    engine
        .transition_task(&scope, denied_remote, TaskState::Working, None)
        .await
        .unwrap();
    let denied_open = client
        .post(format!(
            "{}/a2a/prod/acme/agents/resolver/v1/tasks/{denied_remote}/authorization:request",
            resolver.url
        ))
        .bearer_auth("resolver-secret")
        .header("a2a-version", "1.0")
        .json(&json!({"authorizationRequestId":"revoked-peer-flow"}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied_open.status(), 200);
    let denied_open: Value = denied_open.json().await.unwrap();
    let denied_challenge = denied_open["pendingApprovalId"].as_str().unwrap();
    let denied_submission = denied_peer["submission_id"].as_str().unwrap();
    let denied_refresh = client
        .post(format!(
            "{peer_base}/resolver/resolve/submissions/{denied_submission}:refresh"
        ))
        .bearer_auth("notifier-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(denied_refresh.status(), 200);
    verifier_allowed.store(false, Ordering::SeqCst);
    let denied = client
        .post(format!(
            "{peer_base}/resolver/resolve/submissions/{denied_submission}/authorization:resolve"
        ))
        .bearer_auth("notifier-secret")
        .json(&json!({"challengeId":denied_challenge}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 200, "{}", denied.text().await.unwrap());
    let denied: Value = denied.json().await.unwrap();
    assert_eq!(denied["status"]["state"], "rejected");
    assert_eq!(
        engine
            .get_task(&scope, denied_remote)
            .await
            .unwrap()
            .unwrap()
            .status
            .state,
        TaskState::AuthRequired
    );
    assert_eq!(verifier_calls.load(Ordering::SeqCst), 1);

    verifier_allowed.store(true, Ordering::SeqCst);
    let accepted_send = client
        .post(format!("{peer_base}/resolver/resolve/message:send"))
        .bearer_auth("notifier-secret")
        .json(&message("authorize-success-child"))
        .send()
        .await
        .unwrap();
    assert_eq!(accepted_send.status(), 200);
    let accepted_peer: Value = accepted_send.json().await.unwrap();
    let accepted_remote = accepted_peer["status"]["task"]["id"].as_str().unwrap();
    engine
        .transition_task(&scope, accepted_remote, TaskState::Working, None)
        .await
        .unwrap();
    let accepted_open = client
        .post(format!(
            "{}/a2a/prod/acme/agents/resolver/v1/tasks/{accepted_remote}/authorization:request",
            resolver.url
        ))
        .bearer_auth("resolver-secret")
        .header("a2a-version", "1.0")
        .json(&json!({"authorizationRequestId":"successful-peer-flow"}))
        .send()
        .await
        .unwrap();
    assert_eq!(accepted_open.status(), 200);
    let accepted_open: Value = accepted_open.json().await.unwrap();
    let accepted_challenge = accepted_open["pendingApprovalId"].as_str().unwrap();
    let accepted_submission = accepted_peer["submission_id"].as_str().unwrap();
    let accepted_refresh = client
        .post(format!(
            "{peer_base}/resolver/resolve/submissions/{accepted_submission}:refresh"
        ))
        .bearer_auth("notifier-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(accepted_refresh.status(), 200);
    proxy
        .state
        .lose_next_authorization_response
        .store(true, Ordering::SeqCst);
    let authorization_url = format!(
        "{peer_base}/resolver/resolve/submissions/{accepted_submission}/authorization:resolve"
    );
    let first = client
        .post(&authorization_url)
        .bearer_auth("notifier-secret")
        .json(&json!({"challengeId":accepted_challenge}))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 200, "{}", first.text().await.unwrap());
    let first: Value = first.json().await.unwrap();
    assert_eq!(first["status"]["state"], "uncertain");
    assert_eq!(
        engine
            .get_task(&scope, accepted_remote)
            .await
            .unwrap()
            .unwrap()
            .status
            .state,
        TaskState::Working
    );
    assert_eq!(verifier_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        proxy.state.authorization_deliveries.load(Ordering::SeqCst),
        2
    );

    notifier.restart();
    notifier.ready(&client).await;
    let recovered = client
        .post(&authorization_url)
        .bearer_auth("notifier-secret")
        .json(&json!({"challengeId":accepted_challenge}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        recovered.status(),
        200,
        "{}",
        recovered.text().await.unwrap()
    );
    let recovered: Value = recovered.json().await.unwrap();
    assert_eq!(recovered["status"]["state"], "resolved");
    assert_eq!(recovered["status"]["task"]["id"], accepted_remote);
    assert_eq!(recovered["status"]["task"]["status"]["state"], "working");
    assert_eq!(verifier_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        proxy.state.authorization_deliveries.load(Ordering::SeqCst),
        2
    );
    assert!(proxy.state.task_observations.load(Ordering::SeqCst) >= 1);
    assert_eq!(provider_calls.load(Ordering::SeqCst), 0);
    webhook_task.abort();
    verifier_task.abort();
}

#[cfg(any(feature = "redis", feature = "postgres"))]
#[allow(clippy::too_many_lines)]
async fn two_server_peer_continuation_contract(state: Value, store: Arc<dyn StateStore>) {
    let (webhook_url, calls, provider_entered, provider_release, webhook_task) =
        pausing_webhook().await;
    let notifier_port = reserve_port();
    let resolver_port = reserve_port();
    let credential_hashes = [
        acteon_server::auth::api_key::hash_api_key("alice-secret"),
        acteon_server::auth::api_key::hash_api_key("notifier-secret"),
        acteon_server::auth::api_key::hash_api_key("resolver-secret"),
    ];
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();
    let mut notifier = PeerMeshServer::start(
        notifier_port,
        notifier_port,
        resolver_port,
        &webhook_url,
        &state,
        &credential_hashes,
        true,
        false,
    );
    notifier.ready(&client).await;

    let response = client
        .post(format!(
            "{}/a2a/prod/acme/agents/notifier/v1/message:send",
            notifier.url
        ))
        .bearer_auth("alice-secret")
        .json(&message("continue-root"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    let root: Value = response.json().await.unwrap();
    let engine = TaskEngine::new(store.clone());
    let scope = TaskScope::new("prod", "acme");
    let root_id = root["id"].as_str().unwrap();
    engine
        .transition_task(&scope, root_id, TaskState::Working, None)
        .await
        .unwrap();
    engine
        .transition_task(&scope, root_id, TaskState::Completed, None)
        .await
        .unwrap();
    let mut resolver = PeerMeshServer::start(
        resolver_port,
        notifier_port,
        resolver_port,
        &webhook_url,
        &state,
        &credential_hashes,
        false,
        true,
    );
    resolver.ready(&client).await;
    let config: toml::Value =
        toml::from_str(&fs::read_to_string(resolver.directory.join("acteon.toml")).unwrap())
            .unwrap();
    publish_peer_cards(store.as_ref(), &config).await;
    let peer_base = format!(
        "{}/a2a/prod/acme/agents/notifier/v1/tasks/{}/peers",
        notifier.url,
        root["id"].as_str().unwrap()
    );
    let response = client
        .post(format!("{peer_base}/resolver/resolve/message:send"))
        .bearer_auth("notifier-secret")
        .json(&message("continue-child"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    let peer: Value = response.json().await.unwrap();
    assert_eq!(peer["status"]["state"], "accepted");
    let remote_id = peer["status"]["task"]["id"].as_str().unwrap();

    tokio::time::timeout(Duration::from_secs(30), provider_entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    engine
        .get_task(&scope, remote_id)
        .await
        .unwrap()
        .filter(|task| task.status.state == TaskState::Working)
        .expect("provider delivery starts before the webhook is invoked");
    let (_, challenge) = engine
        .pause_for_human(
            &scope,
            remote_id,
            PauseKind::UserInput,
            Some("Choose the remediation window".into()),
            None,
        )
        .await
        .unwrap();
    let response = client
        .post(format!(
            "{peer_base}/resolver/resolve/submissions/{}:refresh",
            peer["submission_id"].as_str().unwrap()
        ))
        .bearer_auth("notifier-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    let refreshed: Value = response.json().await.unwrap();
    assert_eq!(
        refreshed["status"]["task"]["status"]["state"],
        "input_required"
    );
    assert_eq!(
        refreshed["status"]["task"]["pendingApprovalId"],
        challenge.approval_id
    );
    provider_release.add_permits(1);
    let result_key = StateKey::new(
        "prod",
        "acme",
        KeyKind::Custom(acteon_executor::governed::RESULT_KIND.into()),
        uuid::Uuid::new_v5(
            &uuid::Uuid::parse_str(remote_id).unwrap(),
            &0_u32.to_be_bytes(),
        )
        .to_string(),
    );
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if store.get(&result_key).await.unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();

    let continuation_url = format!(
        "{peer_base}/resolver/resolve/submissions/{}/message:send",
        peer["submission_id"].as_str().unwrap()
    );
    let response = client
        .post(&continuation_url)
        .bearer_auth("notifier-secret")
        .json(&json!({"message": {
            "role": "user", "messageId": "continue-answer",
            "parts": [{"kind": "text", "text": "02:00 UTC"}]
        }}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    let continued: Value = response.json().await.unwrap();
    assert_eq!(
        continued["status"]["state"],
        "accepted",
        "{continued}\nresolver:\n{}\nnotifier:\n{}",
        resolver.log(),
        notifier.log()
    );
    assert_eq!(continued["status"]["task"]["id"], remote_id);
    assert_eq!(continued["status"]["task"]["status"]["state"], "completed");
    assert!(continued["status"]["progress_cursor"].as_str().is_some());
    assert!(continued["continuation_id"].as_str().is_some());
    let task_after = engine.get_task(&scope, remote_id).await.unwrap().unwrap();
    assert_eq!(task_after.status.state, TaskState::Completed);
    assert_eq!(
        task_after
            .history
            .iter()
            .filter(|item| item.message_id == "continue-answer")
            .count(),
        1
    );
    assert_ne!(challenge.approval_id, "");

    notifier.restart();
    notifier.ready(&client).await;
    let response = client
        .post(&continuation_url)
        .bearer_auth("notifier-secret")
        .json(&json!({"message": {
            "role": "user", "messageId": "continue-answer",
            "parts": [{"kind": "text", "text": "02:00 UTC"}]
        }}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    assert_eq!(response.json::<Value>().await.unwrap(), continued);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    webhook_task.abort();
}

#[cfg(any(feature = "redis", feature = "postgres"))]
#[allow(clippy::too_many_lines)]
async fn two_server_peer_cancel_restart_contract(state: Value, store: Arc<dyn StateStore>) {
    let (webhook_url, calls, webhook_task) = webhook().await;
    let notifier_port = reserve_port();
    let resolver_port = reserve_port();
    let proxy_port = reserve_port();
    assert_eq!(
        [notifier_port, resolver_port, proxy_port]
            .into_iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
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
        proxy_port,
        &webhook_url,
        &state,
        &credential_hashes,
        true,
        false,
    );
    resolver.ready(&client).await;
    let proxy = ResponseLossProxy::start(proxy_port, resolver.url.clone(), false).await;
    proxy.ready(&client).await;
    let mut notifier = PeerMeshServer::start(
        notifier_port,
        notifier_port,
        proxy_port,
        &webhook_url,
        &state,
        &credential_hashes,
        true,
        false,
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
    publish_peer_cards(store.as_ref(), &resolver_config).await;
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
    assert_eq!(proxy.state.stop_deliveries.load(Ordering::SeqCst), 1);
    assert_eq!(proxy.state.task_observations.load(Ordering::SeqCst), 0);

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
    assert_eq!(replayed, canceled);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(proxy.state.stop_deliveries.load(Ordering::SeqCst), 1);
    assert_eq!(proxy.state.task_observations.load(Ordering::SeqCst), 1);
    assert_eq!(
        proxy
            .state
            .conditional_task_observations
            .load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        proxy.state.not_modified_observations.load(Ordering::SeqCst),
        1
    );
    webhook_task.abort();
}

#[tokio::test]
#[cfg(feature = "redis")]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; two real HTTPS servers and post-commit response loss"]
#[allow(clippy::too_many_lines)]
async fn redis_peer_cancel_response_loss_survives_restart_without_redelivery() {
    let (webhook_url, calls, webhook_task) = webhook().await;
    let (state, redis_config) = redis_state();
    let store = acteon_state_redis::RedisStateStore::new(&redis_config).unwrap();
    let notifier_port = reserve_port();
    let resolver_port = reserve_port();
    let proxy_port = reserve_port();
    assert_eq!(
        [notifier_port, resolver_port, proxy_port]
            .into_iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
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
        proxy_port,
        &webhook_url,
        &state,
        &credential_hashes,
        true,
        false,
    );
    resolver.ready(&client).await;
    let proxy = ResponseLossProxy::start(proxy_port, resolver.url.clone(), true).await;
    proxy.ready(&client).await;
    let mut notifier = PeerMeshServer::start(
        notifier_port,
        notifier_port,
        proxy_port,
        &webhook_url,
        &state,
        &credential_hashes,
        true,
        false,
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
    publish_peer_cards(&store, &resolver_config).await;
    notifier.ready(&client).await;

    let response = client
        .post(format!(
            "{}/a2a/prod/acme/agents/notifier/v1/message:send",
            notifier.url
        ))
        .bearer_auth("alice-secret")
        .json(&message("response-loss-root"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    let root: Value = response.json().await.unwrap();
    let peer_base = format!(
        "{}/a2a/prod/acme/agents/notifier/v1/tasks/{}/peers",
        notifier.url,
        root["id"].as_str().unwrap()
    );
    let response = client
        .post(format!("{peer_base}/resolver/resolve/message:send"))
        .bearer_auth("notifier-secret")
        .json(&message("response-loss-child"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    let peer: Value = response.json().await.unwrap();
    assert_eq!(peer["status"]["state"], "accepted");
    let task_id = peer["status"]["task"]["id"].as_str().unwrap();
    let submission = peer["submission_id"].as_str().unwrap();
    let cancel_url = format!("{peer_base}/resolver/resolve/submissions/{submission}:cancel");

    let response = client
        .post(&cancel_url)
        .bearer_auth("notifier-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    let uncertain: Value = response.json().await.unwrap();
    assert_eq!(uncertain["status"]["state"], "uncertain");
    assert_eq!(proxy.state.stop_deliveries.load(Ordering::SeqCst), 1);
    assert_eq!(proxy.state.task_observations.load(Ordering::SeqCst), 0);
    let committed = proxy.state.committed_stop.lock().unwrap().clone().unwrap();
    assert_eq!(committed["future_starts_blocked"], true);
    assert_eq!(committed["task"]["id"], task_id);
    assert_eq!(committed["task"]["status"]["state"], "submitted");
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let cancellation_id = uncertain["cancellation_id"].clone();
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
    assert_eq!(replayed, uncertain);
    assert_eq!(proxy.state.stop_deliveries.load(Ordering::SeqCst), 1);
    assert_eq!(proxy.state.task_observations.load(Ordering::SeqCst), 1);
    assert_eq!(
        proxy
            .state
            .conditional_task_observations
            .load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        proxy.state.not_modified_observations.load(Ordering::SeqCst),
        1
    );
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
        &backend,
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
#[allow(clippy::too_many_lines)]
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
        &backend,
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
        &backend,
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
        &backend,
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
        &json!({"backend":"memory"}),
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
        &json!({"backend":"memory"}),
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
        &backend,
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
        &backend,
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
