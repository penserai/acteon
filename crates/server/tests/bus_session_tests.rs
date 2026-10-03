//! HTTP receipt-session conformance. The Kafka CI job supplies a real broker.
#![cfg(feature = "bus")]
use acteon_bus::{BusBackend, BusMessage, KafkaBackend, KafkaBusConfig};
use acteon_core::{Subscription, Topic};
use acteon_gateway::GatewayBuilder;
use acteon_server::{
    api::{
        self, AppState,
        bus_sessions::{self as http, ReceiveResponse, SessionResponse},
    },
    auth::identity::CallerIdentity,
    bus_sessions::BusSessionRegistry,
    config::{BusSessionConfig, ConfigSnapshot},
};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use axum::http::{Method, StatusCode};
use axum::{
    Extension, Router,
    routing::{get, post},
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::sync::RwLock;
use tower::ServiceExt;
use uuid::Uuid;

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
    fn assert_status_too_many_requests(&self) {
        self.assert_status(StatusCode::TOO_MANY_REQUESTS);
    }
}

struct Fixture {
    backend: Arc<KafkaBackend>,
    sub: Subscription,
    state: AppState,
}
impl Fixture {
    async fn new(config: BusSessionConfig) -> Option<Self> {
        let bootstrap = std::env::var("ACTEON_KAFKA_BOOTSTRAP").ok()?;
        let backend = KafkaBackend::new(&KafkaBusConfig {
            bootstrap_servers: bootstrap,
            extra: vec![
                ("session.timeout.ms".into(), "6000".into()),
                ("heartbeat.interval.ms".into(), "1000".into()),
                ("group.id".into(), "acteon-live-wrong-global".into()),
            ],
            ..Default::default()
        })
        .unwrap();
        let mut topic = Topic::new(
            format!("http-{}", Uuid::new_v4().simple()),
            "test",
            "tenant",
        );
        topic.partitions = 1;
        topic.replication_factor = 1;
        backend.create_topic(&topic).await.unwrap();
        // Admin acknowledgement can precede topic/leader metadata propagation.
        // Verify the partition serves watermark requests before opening consumers.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
        loop {
            if backend
                .scan_topic_watermarks(&topic.kafka_topic_name())
                .await
                .is_ok_and(|w| w.high_water_marks.contains_key(&0))
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "topic partition readiness deadline"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let mut sub = Subscription::new(
            format!("sub-{}", Uuid::new_v4().simple()),
            topic.kafka_topic_name(),
            "test",
            "tenant",
        );
        sub.receipt_required = true;
        sub.starting_offset = acteon_core::SubscriptionStartOffset::Earliest;
        sub.ack_timeout_ms = 60_000;
        let store = Arc::new(MemoryStateStore::new());
        store
            .set(
                &StateKey::new("test", "tenant", KeyKind::BusSubscription, &sub.id),
                &serde_json::to_string(&sub).unwrap(),
                None,
            )
            .await
            .unwrap();
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
            bus_backend: Some(backend.clone()),
            bus_schema_validator: acteon_bus::SchemaValidator::new(),
            bus_sessions: Arc::new(BusSessionRegistry::new(config).unwrap()),
        };
        Some(Self {
            backend,
            sub,
            state,
        })
    }
    fn server(&self, identity: CallerIdentity) -> TestServer {
        // Exercise the endpoint authorization with a fresh principal for every request.
        let router = Router::new()
            .route(
                "/v1/bus/subscriptions/{namespace}/{tenant}/{id}/sessions",
                post(http::open),
            )
            .route(
                "/v1/bus/subscriptions/{namespace}/{tenant}/{id}/sessions/{session}",
                get(http::get).delete(http::close),
            )
            .route(
                "/v1/bus/subscriptions/{namespace}/{tenant}/{id}/sessions/{session}/receive",
                post(http::receive),
            )
            .route(
                "/v1/bus/subscriptions/{namespace}/{tenant}/{id}/sessions/{session}/validate",
                post(http::validate),
            )
            .route(
                "/v1/bus/subscriptions/{namespace}/{tenant}/{id}/sessions/{session}/ack",
                post(http::ack),
            )
            .route(
                "/v1/bus/subscriptions/{namespace}/{tenant}/{id}/ack",
                post(api::bus::ack_subscription),
            )
            .route(
                "/v1/bus/subscribe/{subscription_id}",
                get(api::bus::subscribe),
            )
            .layer(Extension(identity))
            .with_state(self.state.clone());
        TestServer(router)
    }
    fn path(&self) -> String {
        format!("/v1/bus/subscriptions/test/tenant/{}/sessions", self.sub.id)
    }
    async fn open(&self, server: &TestServer, request: Uuid) -> SessionResponse {
        let response = server
            .post(&self.path())
            .json(&json!({"request_id":request}))
            .await;
        response.assert_status_ok();
        response.json()
    }
    async fn produce(&self, n: usize) {
        for i in 0..n {
            self.backend
                .produce(BusMessage::new(&self.sub.topic, json!({"i":i})))
                .await
                .unwrap();
        }
    }
    async fn receive(&self, server: &TestServer, session: Uuid, n: usize) -> ReceiveResponse {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
        loop {
            let r = server
                .post(&format!("{}/{session}/receive", self.path()))
                .json(&json!({"max_messages":n,"wait_ms":1000}))
                .await;
            r.assert_status_ok();
            let body: ReceiveResponse = r.json();
            if body.deliveries.len() >= n {
                return body;
            }
            assert!(tokio::time::Instant::now() < deadline, "receive deadline");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    async fn cleanup(&self) {
        self.state.bus_sessions.shutdown().await;
        self.backend.delete_topic(&self.sub.topic).await.unwrap();
    }
}

#[tokio::test]
async fn http_sessions_replay_responses_validate_prefix_and_block_raw_bypasses() {
    let Some(f) = Fixture::new(BusSessionConfig::default()).await else {
        return;
    };
    let server = f.server(CallerIdentity::anonymous());
    f.produce(3).await;
    let request = Uuid::new_v4();
    let s = f.open(&server, request).await;
    assert_eq!(s.consumer_group, f.sub.consumer_group());
    assert_eq!(s.session_id, f.open(&server, request).await.session_id);
    let batch = f.receive(&server, s.session_id, 3).await;
    let retry = f.receive(&server, s.session_id, 3).await;
    assert_eq!(
        batch
            .deliveries
            .iter()
            .map(|d| d.receipt_id)
            .collect::<Vec<_>>(),
        retry
            .deliveries
            .iter()
            .map(|d| d.receipt_id)
            .collect::<Vec<_>>()
    );
    let ack_path = format!("{}/{}/ack", f.path(), s.session_id);
    let validate_path = format!("{}/{}/validate", f.path(), s.session_id);
    server
        .post(&ack_path)
        .json(&json!({"receipt_ids":[batch.deliveries[2].receipt_id]}))
        .await
        .assert_status_conflict();
    server
        .post(&ack_path)
        .json(&json!({"receipt_ids":[batch.deliveries[0].receipt_id,Uuid::new_v4()]}))
        .await
        .assert_status_not_found();
    server
        .post(&ack_path)
        .json(&json!({"receipt_ids":[],"partition":0,"offset":100}))
        .await
        .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    server
        .post(&format!(
            "/v1/bus/subscriptions/test/tenant/{}/ack",
            f.sub.id
        ))
        .json(&json!({"partition":0,"offset":100}))
        .await
        .assert_status_conflict();
    server
        .get(&format!(
            "/v1/bus/subscribe/{}?topic={}&from=earliest",
            f.sub.consumer_group(),
            f.sub.topic
        ))
        .await
        .assert_status_bad_request();
    let ids = batch
        .deliveries
        .iter()
        .map(|d| d.receipt_id)
        .collect::<Vec<_>>();
    let validated = server
        .post(&validate_path)
        .json(&json!({"receipt_ids":ids}))
        .await;
    validated.assert_status_ok();
    assert_eq!(
        validated.json::<serde_json::Value>()["positions"][0]["offset"],
        2
    );
    assert_eq!(
        f.backend
            .consumer_lag(&f.sub.topic, &s.consumer_group)
            .await
            .unwrap()[0]
            .committed,
        -1
    );
    // Model a dropped response: the actor completes the commit; retry only confirms history.
    server
        .post(&ack_path)
        .json(&json!({"receipt_ids":ids}))
        .await
        .assert_status_ok();
    server
        .post(&ack_path)
        .json(&json!({"receipt_ids":ids}))
        .await
        .assert_status_ok();
    assert_eq!(
        f.backend
            .consumer_lag(&f.sub.topic, &s.consumer_group)
            .await
            .unwrap()[0]
            .committed,
        2
    );
    server
        .delete(&format!("{}/{}", f.path(), s.session_id))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    server
        .post(&ack_path)
        .json(&json!({"receipt_ids":ids}))
        .await
        .assert_status(StatusCode::GONE);
    // Same open key is retained as a closed tombstone, never a resurrected capability.
    tokio::time::sleep(Duration::from_millis(150)).await;
    server
        .post(&f.path())
        .json(&json!({"request_id":request}))
        .await
        .assert_status(StatusCode::GONE);
    f.cleanup().await;
}

#[tokio::test]
async fn http_sessions_bind_owner_fresh_grants_scope_and_definition() {
    let Some(f) = Fixture::new(BusSessionConfig::default()).await else {
        return;
    };
    let mut owner = CallerIdentity::anonymous();
    owner.id = "owner".into();
    let server = f.server(owner.clone());
    let s = f.open(&server, Uuid::new_v4()).await;
    let path = format!("{}/{}", f.path(), s.session_id);
    let mut stranger = owner.clone();
    stranger.id = "stranger".into();
    f.server(stranger)
        .get(&path)
        .await
        .assert_status_not_found();
    let mut revoked = owner;
    revoked.grants.clear();
    f.server(revoked).get(&path).await.assert_status_forbidden();
    server
        .get(&path.replace("/tenant/", "/another/"))
        .await
        .assert_status_not_found();
    let mut changed = f.sub.clone();
    changed.updated_at = chrono::Utc::now();
    f.state
        .gateway
        .read()
        .await
        .state_store()
        .set(
            &StateKey::new("test", "tenant", KeyKind::BusSubscription, &f.sub.id),
            &serde_json::to_string(&changed).unwrap(),
            None,
        )
        .await
        .unwrap();
    server.get(&path).await.assert_status_conflict();
    f.cleanup().await;
}

#[tokio::test]
async fn http_sessions_expire_and_bound_retained_capacity() {
    let config = BusSessionConfig {
        max_sessions: 1,
        max_sessions_per_tenant: 1,
        idle_timeout_ms: 300,
        lifetime_ms: 1000,
        closed_retention_ms: 200,
        ..Default::default()
    };
    let Some(f) = Fixture::new(config).await else {
        return;
    };
    let server = f.server(CallerIdentity::anonymous());
    let s = f.open(&server, Uuid::new_v4()).await;
    server
        .post(&f.path())
        .json(&json!({"request_id":Uuid::new_v4()}))
        .await
        .assert_status_too_many_requests();
    tokio::time::sleep(Duration::from_millis(450)).await;
    let r = server.get(&format!("{}/{}", f.path(), s.session_id)).await;
    r.assert_status_ok();
    assert_eq!(r.json::<SessionResponse>().phase, "closed");
    // The closed snapshot precedes blocking Kafka teardown. Retention starts
    // when teardown completes, so a fixed sleep from observing closure races it.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    let request_id = Uuid::new_v4();
    let next = loop {
        let response = server
            .post(&f.path())
            .json(&json!({"request_id":request_id}))
            .await;
        if response.status == StatusCode::OK {
            break response.json::<SessionResponse>();
        }
        response.assert_status_too_many_requests();
        assert!(
            tokio::time::Instant::now() < deadline,
            "closed-session capacity release deadline"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_ne!(next.session_id, s.session_id);
    f.cleanup().await;
}

#[tokio::test]
async fn http_sessions_fence_revoked_receipts_and_replay_uncommitted_prefix() {
    let Some(f) = Fixture::new(BusSessionConfig::default()).await else {
        return;
    };
    let server = f.server(CallerIdentity::anonymous());
    f.produce(1).await;
    let original = f.open(&server, Uuid::new_v4()).await;
    let old = f
        .receive(&server, original.session_id, 1)
        .await
        .deliveries
        .remove(0);
    let peer = f.open(&server, Uuid::new_v4()).await;
    let original_path = format!("{}/{}", f.path(), original.session_id);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    loop {
        let s: SessionResponse = server.get(&original_path).await.json();
        if s.assignment_epoch > old.assignment_epoch {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "revocation deadline"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    server
        .post(&format!("{original_path}/ack"))
        .json(&json!({"receipt_ids":[old.receipt_id]}))
        .await
        .assert_status_conflict();
    server
        .delete(&original_path)
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let redelivered = f
        .receive(&server, peer.session_id, 1)
        .await
        .deliveries
        .remove(0);
    assert_eq!(redelivered.offset, 0);
    server
        .post(&format!("{}/{}/ack", f.path(), peer.session_id))
        .json(&json!({"receipt_ids":[redelivered.receipt_id]}))
        .await
        .assert_status_ok();
    f.cleanup().await;
}

#[tokio::test]
async fn http_sessions_bound_records_and_expire_unacknowledged_work_without_commits() {
    let config = BusSessionConfig {
        max_in_flight: 2,
        ..Default::default()
    };
    let Some(mut f) = Fixture::new(config).await else {
        return;
    };
    // Set a short acknowledgement budget independently of idle/lifetime limits.
    f.sub.ack_timeout_ms = 1500;
    f.state
        .gateway
        .read()
        .await
        .state_store()
        .set(
            &StateKey::new("test", "tenant", KeyKind::BusSubscription, &f.sub.id),
            &serde_json::to_string(&f.sub).unwrap(),
            None,
        )
        .await
        .unwrap();
    let server = f.server(CallerIdentity::anonymous());
    f.produce(3).await;
    let s = f.open(&server, Uuid::new_v4()).await;
    let batch = f.receive(&server, s.session_id, 2).await;
    assert_eq!(batch.deliveries.len(), 2);
    let path = format!("{}/{}", f.path(), s.session_id);
    server
        .post(&format!("{path}/receive"))
        .json(&json!({"max_messages":3,"wait_ms":0}))
        .await
        .assert_status_bad_request();
    tokio::time::sleep(Duration::from_millis(1700)).await;
    let status: SessionResponse = server.get(&path).await.json();
    assert_eq!(status.phase, "closed");
    assert!(
        status
            .closed_reason
            .unwrap()
            .contains("acknowledgement timeout")
    );
    assert_eq!(
        f.backend
            .consumer_lag(&f.sub.topic, &s.consumer_group)
            .await
            .unwrap()[0]
            .committed,
        -1
    );
    f.cleanup().await;
}

#[tokio::test]
async fn http_sessions_close_on_payload_limits_and_absolute_lifetime() {
    let config = BusSessionConfig {
        max_buffer_bytes: 1,
        ..Default::default()
    };
    let Some(f) = Fixture::new(config).await else {
        return;
    };
    let server = f.server(CallerIdentity::anonymous());
    let s = f.open(&server, Uuid::new_v4()).await;
    f.produce(1).await;
    let path = format!("{}/{}", f.path(), s.session_id);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    loop {
        let status: SessionResponse = server.get(&path).await.json();
        if status.phase == "closed" {
            let reason = status.closed_reason.unwrap();
            assert!(
                reason.contains("buffer limit"),
                "unexpected closure: {reason}"
            );
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        f.backend
            .consumer_lag(&f.sub.topic, &s.consumer_group)
            .await
            .unwrap()[0]
            .committed,
        -1
    );
    f.cleanup().await;
    let config = BusSessionConfig {
        lifetime_ms: 400,
        idle_timeout_ms: 10_000,
        ..Default::default()
    };
    let f = Fixture::new(config).await.unwrap();
    let server = f.server(CallerIdentity::anonymous());
    let s = f.open(&server, Uuid::new_v4()).await;
    tokio::time::sleep(Duration::from_millis(550)).await;
    let status: SessionResponse = server
        .get(&format!("{}/{}", f.path(), s.session_id))
        .await
        .json();
    assert_eq!(status.phase, "closed");
    assert!(status.closed_reason.unwrap().contains("absolute lifetime"));
    f.cleanup().await;
}
