//! Durable operator routes work without Kafka and enforce scope before lookup.
#![cfg(feature = "bus")]
use acteon_bus::{
    ManagedStreamStage, StreamCheckpointConfig, StreamCheckpointCoordinator, StreamStageConfig,
    stream_checkpoint_key,
};
use acteon_gateway::GatewayBuilder;
use acteon_server::{
    api::{self, AppState, bus_stages},
    auth::{config::Grant, identity::CallerIdentity, role::Role},
    bus_sessions::BusSessionRegistry,
    config::{BusSessionConfig, ConfigSnapshot},
};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use axum::http::{Method, StatusCode};
use axum::{
    Extension, Router,
    routing::{get, post},
};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceExt;
async fn server(identity: CallerIdentity) -> TestServer {
    seeded_server(identity, false).await
}
async fn seeded_server(identity: CallerIdentity, quarantine: bool) -> TestServer {
    let store = Arc::new(MemoryStateStore::new());
    let c = StreamCheckpointCoordinator::<u64, u64>::initialize(
        store.clone(),
        stream_checkpoint_key("test", "tenant", "stage"),
        0,
        StreamCheckpointConfig::default(),
    )
    .await
    .unwrap();
    ManagedStreamStage::initialize(c, "worker", "v1", "source", StreamStageConfig::default())
        .await
        .unwrap();
    if quarantine {
        use acteon_state::StateStore;
        let key = stream_checkpoint_key("test", "tenant", "stage");
        let mut snapshot: serde_json::Value =
            serde_json::from_str(&store.get(&key).await.unwrap().unwrap()).unwrap();
        snapshot["schema_version"] = serde_json::json!(4);
        snapshot["processing"]["config"]["input"] =
            serde_json::to_value(acteon_bus::StreamInputPolicy {
                poison_policy: acteon_bus::StreamPoisonPolicy::Quarantine,
                ..Default::default()
            })
            .unwrap();
        snapshot["processing"]["counters"]["quarantined_records"] = serde_json::json!(2);
        let entries: Vec<_> = (0..2)
            .map(|offset| {
                let position = acteon_bus::StreamPosition {
                    lane: acteon_bus::StreamPositionLane {
                        source: "input".into(),
                        topic: "test.tenant.input".into(),
                        consumer_group: "group".into(),
                        partition: 0,
                    },
                    offset,
                };
                let mut message = acteon_bus::BusMessage::new(
                    "test.tenant.input",
                    serde_json::json!({"secret":"retained telemetry"}),
                );
                message.partition = Some(0);
                message.offset = Some(offset);
                acteon_bus::StreamQuarantinedInput {
                    id: uuid::Uuid::new_v4().to_string(),
                    position,
                    message,
                    failed_at: chrono::Utc::now(),
                    failure: acteon_bus::StreamInputFailure::TypedDecode,
                    contract_sha256: None,
                    reason: "typed decode failed".into(),
                }
            })
            .collect();
        snapshot["positions"] = serde_json::json!([entries[1].position]);
        snapshot["processing"]["quarantine"] = serde_json::to_value(entries).unwrap();
        store.set(&key, &snapshot.to_string(), None).await.unwrap();
    }
    let gateway = GatewayBuilder::new()
        .state(store)
        .lock(Arc::new(MemoryDistributedLock::new()))
        .build()
        .unwrap();
    let metrics = gateway.metrics_arc();
    let state = AppState {
        gateway: Arc::new(RwLock::new(gateway)),
        metrics,
        audit: None,
        analytics: None,
        auth: None,
        rate_limiter: None,
        embedding: None,
        embedding_metrics: None,
        connection_registry: None,
        a2a_discovery_cache: Arc::new(api::a2a_discovery_cache::DiscoveryCache::new()),
        dispatch_semaphore: Arc::new(tokio::sync::Semaphore::new(100)),
        config: ConfigSnapshot::default(),
        static_quotas: None,
        static_templates: None,
        ui_path: None,
        ui_enabled: false,
        cors_allowed_origins: vec![],
        signature_verifier: None,
        replay_protection: None,
        #[cfg(feature = "swarm")]
        swarm_registry: None,
        bus_backend: None,
        bus_schema_validator: acteon_bus::SchemaValidator::new(),
        bus_sessions: Arc::new(BusSessionRegistry::new(BusSessionConfig::default()).unwrap()),
    };
    TestServer(
        Router::new()
            .route(
                "/v1/bus/stages/{namespace}/{tenant}/{id}",
                get(bus_stages::status),
            )
            .route(
                "/v1/bus/stages/{namespace}/{tenant}/{id}/quarantine",
                get(bus_stages::list),
            )
            .route(
                "/v1/bus/stages/{namespace}/{tenant}/{id}/quarantine/{entry}",
                get(bus_stages::get).delete(bus_stages::discard),
            )
            .route(
                "/v1/bus/stages/{namespace}/{tenant}/{id}/quarantine/{entry}/replay",
                post(bus_stages::replay),
            )
            .route(
                "/v1/bus/stages/{namespace}/{tenant}/{id}/control",
                post(bus_stages::control),
            )
            .route(
                "/v1/bus/stages/{namespace}/{tenant}/{id}/controls/{request}",
                get(bus_stages::control_audit),
            )
            .route(
                "/v1/bus/stages/{namespace}/{tenant}/{id}/replays/{request}",
                get(bus_stages::replay_audit),
            )
            .layer(Extension(identity))
            .with_state(state),
    )
}
fn identity(role: Role, actions: Vec<&str>) -> CallerIdentity {
    CallerIdentity {
        id: "operator".into(),
        principal: None,
        role,
        auth_method: "api_key".into(),
        grants: vec![Grant {
            tenants: vec!["tenant".into()],
            namespaces: vec!["test".into()],
            providers: vec!["bus".into()],
            actions: actions.into_iter().map(str::to_owned).collect(),
            agent_id: None,
        }],
    }
}
#[tokio::test]
async fn scoped_reads_and_discard_are_independently_authorized_without_broker() {
    let viewer = server(identity(Role::Viewer, vec!["stage_read", "stage_manage"])).await;
    let base = "/v1/bus/stages/test/tenant/stage";
    let status = viewer.get(base).await;
    status.assert_status_ok();
    let body: serde_json::Value = status.json();
    assert_eq!(body["quarantined_records"], 0);
    assert!(body.get("state").is_none());
    assert!(body.get("lease").is_none());
    viewer
        .get(&format!("{base}/quarantine?limit=1"))
        .await
        .assert_status_ok();
    viewer
        .get(&format!("{base}/quarantine?limit=101"))
        .await
        .assert_status_bad_request();
    viewer
        .get(&format!("{base}/quarantine?after=deleted"))
        .await
        .assert_status_conflict();
    viewer
        .get(&format!("{base}/quarantine/missing"))
        .await
        .assert_status_not_found();
    viewer
        .delete(&format!("{base}/quarantine/missing"))
        .await
        .assert_status_forbidden();
    viewer
        .get("/v1/bus/stages/test/other/stage")
        .await
        .assert_status_forbidden();
    viewer
        .get("/v1/bus/stages/other/tenant/stage")
        .await
        .assert_status_forbidden();
    viewer
        .get("/v1/bus/stages/test/tenant/missing")
        .await
        .assert_status_not_found();
    let operator = server(identity(Role::Operator, vec!["stage_manage"])).await;
    operator.get(base).await.assert_status_forbidden();
    let response = operator.delete(&format!("{base}/quarantine/missing")).await;
    response.assert_status_ok();
    assert_eq!(response.json::<serde_json::Value>()["discarded"], false);
    let subscriber = server(identity(Role::Operator, vec!["subscribe", "manage"])).await;
    subscriber.get(base).await.assert_status_forbidden();
    subscriber
        .delete(&format!("{base}/quarantine/missing"))
        .await
        .assert_status_forbidden();
}

struct TestServer(Router);
impl TestServer {
    fn post(&self, path: &str) -> TestRequest {
        self.request(Method::POST, path)
    }
    fn get(&self, path: &str) -> TestRequest {
        self.request(Method::GET, path)
    }
    fn delete(&self, path: &str) -> TestRequest {
        self.request(Method::DELETE, path)
    }
    fn request(&self, method: Method, path: &str) -> TestRequest {
        TestRequest {
            router: self.0.clone(),
            method,
            path: path.into(),
            body: serde_json::Value::Null,
        }
    }
}
struct TestRequest {
    router: Router,
    method: Method,
    path: String,
    body: serde_json::Value,
}
impl TestRequest {
    fn json(mut self, body: &serde_json::Value) -> Self {
        self.body = body.clone();
        self
    }
}
impl std::future::IntoFuture for TestRequest {
    type Output = TestResponse;
    type IntoFuture = std::pin::Pin<Box<dyn std::future::Future<Output = TestResponse> + Send>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let request = axum::http::Request::builder()
                .method(self.method)
                .uri(self.path)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::to_vec(&self.body).unwrap(),
                ))
                .unwrap();
            let response = self.router.oneshot(request).await.unwrap();
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), 16 * 1024 * 1024)
                .await
                .unwrap();
            TestResponse {
                status,
                body: body.to_vec(),
            }
        })
    }
}
struct TestResponse {
    status: StatusCode,
    body: Vec<u8>,
}
impl TestResponse {
    fn json<T: serde::de::DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body).unwrap()
    }
    fn assert_status(&self, status: StatusCode) {
        assert_eq!(
            self.status,
            status,
            "{}",
            String::from_utf8_lossy(&self.body)
        );
    }
    fn assert_status_ok(&self) {
        self.assert_status(StatusCode::OK);
    }
    fn assert_status_conflict(&self) {
        self.assert_status(StatusCode::CONFLICT);
    }
    fn assert_status_not_found(&self) {
        self.assert_status(StatusCode::NOT_FOUND);
    }
    fn assert_status_forbidden(&self) {
        self.assert_status(StatusCode::FORBIDDEN);
    }
    fn assert_status_bad_request(&self) {
        self.assert_status(StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn quarantine_pages_omit_payload_and_discard_invalidates_deleted_cursor() {
    let server = seeded_server(
        identity(Role::Operator, vec!["stage_read", "stage_manage"]),
        true,
    )
    .await;
    let base = "/v1/bus/stages/test/tenant/stage";
    let first = server.get(&format!("{base}/quarantine?limit=1")).await;
    first.assert_status_ok();
    let page: serde_json::Value = first.json();
    assert_eq!(page["entries"].as_array().unwrap().len(), 1);
    assert!(
        !String::from_utf8(first.body)
            .unwrap()
            .contains("retained telemetry")
    );
    let id = page["next_after"].as_str().unwrap();
    assert_eq!(id, page["entries"][0]["id"]);
    let second = server
        .get(&format!("{base}/quarantine?limit=1&after={id}"))
        .await;
    second.assert_status_ok();
    let second: serde_json::Value = second.json();
    assert_eq!(second["entries"].as_array().unwrap().len(), 1);
    assert!(second["next_after"].is_null());
    assert_ne!(second["entries"][0]["id"], id);
    let full = server.get(&format!("{base}/quarantine/{id}")).await;
    full.assert_status_ok();
    assert_eq!(
        full.json::<serde_json::Value>()["message"]["payload"]["secret"],
        "retained telemetry"
    );
    let discard = server.delete(&format!("{base}/quarantine/{id}")).await;
    discard.assert_status_ok();
    assert_eq!(discard.json::<serde_json::Value>()["discarded"], true);
    let repeat = server.delete(&format!("{base}/quarantine/{id}")).await;
    repeat.assert_status_ok();
    assert_eq!(repeat.json::<serde_json::Value>()["discarded"], false);
    server
        .get(&format!("{base}/quarantine?after={id}"))
        .await
        .assert_status_conflict();
    server
        .get(&format!("{base}/quarantine/{id}"))
        .await
        .assert_status_not_found();
    let status = server.get(base).await;
    status.assert_status_ok();
    let status: serde_json::Value = status.json();
    assert_eq!(status["quarantined_records"], 1);
    assert_eq!(status["counters"]["discarded_quarantined_records"], 1);
}

#[tokio::test]
async fn encoded_scope_delimiters_are_rejected_before_storage_lookup() {
    let server = server(CallerIdentity::anonymous()).await;
    server
        .get("/v1/bus/stages/test%3Atenant/other/stage")
        .await
        .assert_status_bad_request();
    server
        .get("/v1/bus/stages/test/tenant%3Aother/stage")
        .await
        .assert_status_bad_request();
    server
        .get("/v1/bus/stages/test/tenant/%20")
        .await
        .assert_status_bad_request();
}

#[tokio::test]
async fn replay_requires_distinct_grant_and_binds_authenticated_actor_and_body() {
    let server = seeded_server(
        identity(
            Role::Operator,
            vec!["stage_read", "stage_replay", "stage_manage"],
        ),
        true,
    )
    .await;
    let base = "/v1/bus/stages/test/tenant/stage";
    let page = server
        .get(&format!("{base}/quarantine"))
        .await
        .json::<serde_json::Value>();
    let entry = page["entries"][0]["id"].as_str().unwrap();
    let request = uuid::Uuid::new_v4();
    let path = format!("{base}/quarantine/{entry}/replay");
    let body =
        serde_json::json!({"request_id":request,"reason":"repair invalid telemetry","payload":7});
    let accepted = server.post(&path).json(&body).await;
    accepted.assert_status(StatusCode::ACCEPTED);
    let audit = accepted.json::<serde_json::Value>();
    assert_eq!(audit["actor"], "api_key:operator");
    assert_eq!(audit["status"], "pending");
    assert!(audit.get("payload").is_none());
    server
        .post(&path)
        .json(&body)
        .await
        .assert_status(StatusCode::ACCEPTED);
    let mut changed = body.clone();
    changed["payload"] = serde_json::json!(8);
    server
        .post(&path)
        .json(&changed)
        .await
        .assert_status_conflict();
    server
        .delete(&format!("{base}/quarantine/{entry}"))
        .await
        .assert_status_conflict();
    let history = server.get(&format!("{base}/replays/{request}")).await;
    history.assert_status_ok();
    assert_eq!(history.json::<serde_json::Value>(), audit);
    let manage_only = seeded_server(identity(Role::Operator, vec!["stage_manage"]), true).await;
    manage_only
        .post(&path)
        .json(&body)
        .await
        .assert_status_forbidden();
    let viewer = seeded_server(
        identity(Role::Viewer, vec!["stage_read", "stage_replay"]),
        true,
    )
    .await;
    viewer
        .post(&path)
        .json(&body)
        .await
        .assert_status_forbidden();
    server
        .post(&path.replace("/tenant/", "/other/"))
        .json(&body)
        .await
        .assert_status_forbidden();
}

#[tokio::test]
async fn control_requires_distinct_grant_revision_and_authenticated_audit() {
    let base = "/v1/bus/stages/test/tenant/stage";
    let request = uuid::Uuid::new_v4();
    let body = serde_json::json!({"request_id":request,"command":"halt","expected_control_revision":0,"reason":"maintenance"});
    for (role, grants) in [
        (
            Role::Operator,
            vec!["stage_read", "stage_manage", "stage_replay"],
        ),
        (Role::Viewer, vec!["stage_control", "stage_read"]),
    ] {
        server(identity(role, grants))
            .await
            .post(&format!("{base}/control"))
            .json(&body)
            .await
            .assert_status_forbidden();
    }
    let server = server(identity(
        Role::Operator,
        vec!["stage_control", "stage_read"],
    ))
    .await;
    let accepted = server.post(&format!("{base}/control")).json(&body).await;
    accepted.assert_status_ok();
    let audit = accepted.json::<serde_json::Value>();
    assert_eq!(audit["actor"], "api_key:operator");
    assert_eq!(audit["control_revision"], 1);
    assert_eq!(
        server
            .get(&format!("{base}/controls/{request}"))
            .await
            .json::<serde_json::Value>(),
        audit
    );
    assert_eq!(
        server
            .post(&format!("{base}/control"))
            .json(&body)
            .await
            .json::<serde_json::Value>(),
        audit
    );
    let status = server.get(base).await.json::<serde_json::Value>();
    assert_eq!(status["operator_halted"], true);
    assert_eq!(status["control_revision"], 1);
    let mut stale = body.clone();
    stale["request_id"] = serde_json::json!(uuid::Uuid::new_v4());
    stale["command"] = serde_json::json!("resume");
    server
        .post(&format!("{base}/control"))
        .json(&stale)
        .await
        .assert_status_conflict();
    stale["expected_control_revision"] = serde_json::json!(1);
    server
        .post(&format!("{base}/control"))
        .json(&stale)
        .await
        .assert_status_ok();
    assert_eq!(
        server.get(base).await.json::<serde_json::Value>()["halted"],
        false
    );
    server
        .post(&format!("{base}/control"))
        .json(&body)
        .await
        .assert_status_ok();
    assert_eq!(
        server.get(base).await.json::<serde_json::Value>()["operator_halted"],
        false
    );
    server
        .post(&format!("{base}/control").replace("/tenant/", "/other/"))
        .json(&body)
        .await
        .assert_status_forbidden();
    let mut spoof = body;
    spoof["actor"] = serde_json::json!("admin");
    server
        .post(&format!("{base}/control"))
        .json(&spoof)
        .await
        .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
}
