//! Authentication, not HTTP data, selects the tenant context.
use std::sync::Arc;

use acteon_core::TenantContext;
use acteon_server::auth::{
    AuthProvider,
    api_key::hash_api_key,
    config::{ApiKeyConfig, AuthFileConfig, AuthSettings, Grant, UserConfig},
    context::{ScopedBody, ScopedJson, ScopedPath, ScopedQuery, bind_envelope, bind_root},
    middleware::AuthLayer,
};
use acteon_state_memory::MemoryStateStore;
use axum::{
    Json, Router,
    body::Body,
    http::{Request, StatusCode},
    response::Response,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tower::ServiceExt;

#[derive(Deserialize, Serialize)]
struct Record {
    namespace: String,
    tenant: String,
    payload: Value,
}
impl ScopedBody for Record {
    fn bind_context(body: &mut Value, scope: &TenantContext) -> Result<(), Response> {
        bind_root(body, scope)
    }
}
#[derive(Deserialize, Serialize)]
struct Envelope {
    scope: TenantContext,
    attributes: Value,
}
impl ScopedBody for Envelope {
    fn bind_context(body: &mut Value, scope: &TenantContext) -> Result<(), Response> {
        bind_envelope(body, scope)
    }
}
#[derive(Deserialize, Serialize)]
struct Filter {
    namespace: String,
    tenant: String,
    limit: Option<usize>,
}
fn config() -> AuthFileConfig {
    let grants = vec![Grant {
        tenants: vec!["*".into()],
        namespaces: vec!["*".into()],
        providers: vec!["*".into()],
        actions: vec!["*".into()],
        agent_id: None,
    }];
    AuthFileConfig {
        settings: AuthSettings {
            jwt_secret: "context-test-secret".into(),
            jwt_expiry_seconds: 3600,
        },
        authority_revision: None,
        users: vec![UserConfig {
            username: "alice".into(),
            scope: TenantContext::new("prod", "bank-a"),
            authority_id: None,
            principal: None,
            password_hash: password_hash().into(),
            role: "admin".into(),
            grants: grants.clone(),
        }],
        api_keys: vec![ApiKeyConfig {
            name: "bank-service".into(),
            scope: TenantContext::new("prod", "bank-b"),
            authority_id: None,
            principal: None,
            key_hash: hash_api_key("service-secret").into(),
            role: "admin".into(),
            grants,
        }],
    }
}
fn app(provider: Arc<AuthProvider>) -> Router {
    Router::new()
        .route(
            "/records",
            post(async |ScopedJson(body): ScopedJson<Record>| Json(body)),
        )
        .route(
            "/batch",
            post(async |ScopedJson(body): ScopedJson<Vec<Record>>| Json(body)),
        )
        .route(
            "/envelopes",
            post(async |ScopedJson(body): ScopedJson<Envelope>| Json(body)),
        )
        .route(
            "/query",
            get(async |ScopedQuery(query): ScopedQuery<Filter>| Json(query)),
        )
        .route(
            "/resources",
            get(async |ScopedPath(scope): ScopedPath<(String, String)>| Json(scope)),
        )
        .route(
            "/resources/{id}/{version}",
            get(async |ScopedPath(scope): ScopedPath<(String, String, String, u32)>| Json(scope)),
        )
        .layer(AuthLayer::new(Some(provider)))
}
async fn call(
    app: Router,
    path: &str,
    credential: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(if body.is_some() { "POST" } else { "GET" })
        .uri(path)
        .header("authorization", format!("Bearer {credential}"))
        // Scope headers are untrusted and must have no effect.
        .header("x-tenant-id", "bank-evil")
        .header("x-namespace", "evil")
        .header("content-type", "application/json")
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    let response = app.oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
#[tokio::test]
async fn jwt_and_service_key_supply_scope_without_request_metadata() {
    let auth = Arc::new(AuthProvider::new(&config(), Arc::new(MemoryStateStore::new())).unwrap());
    let (token, _) = auth.login("alice", "password").await.unwrap();
    for (credential, tenant) in [(token.as_str(), "bank-a"), ("service-secret", "bank-b")] {
        let payload = json!({"tenant":"business-attribute", "nested":{"scope":"untouched"}});
        let (status, body) = call(
            app(auth.clone()),
            "/records",
            credential,
            Some(json!({"payload":payload})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            json!({"namespace":"prod", "tenant":tenant, "payload":payload})
        );
        let (status, body) = call(app(auth.clone()), "/query?limit=2", credential, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            json!({"namespace":"prod", "tenant":tenant, "limit":2})
        );
        let (status, body) = call(
            app(auth.clone()),
            "/envelopes",
            credential,
            Some(json!({"attributes":payload})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["scope"], json!({"namespace":"prod", "tenant":tenant}));
        assert_eq!(body["attributes"], payload);
    }
}
#[tokio::test]
async fn metadata_is_rejected_even_when_equal_to_the_authenticated_scope() {
    let auth = Arc::new(AuthProvider::new(&config(), Arc::new(MemoryStateStore::new())).unwrap());
    for (path, body) in [
        ("/records", json!({"tenant":"bank-b", "payload":{}})),
        ("/records", json!({"namespace":"evil", "payload":{}})),
        ("/records", json!({"tenant_scope":["*"], "payload":{}})),
        (
            "/envelopes",
            json!({"scope":{"tenant":"bank-a", "namespace":"prod"}, "attributes":{}}),
        ),
        (
            "/batch",
            json!([{"payload":{}}, {"tenant":"bank-a", "payload":{}}]),
        ),
    ] {
        assert_eq!(
            call(app(auth.clone()), path, "service-secret", Some(body))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    for query in [
        "tenant=bank-b",
        "%74enant=bank-a",
        "namespace=prod",
        "tenant_scope=bank-a",
        "tenant=bank-b&tenant=bank-a",
    ] {
        assert_eq!(
            call(
                app(auth.clone()),
                &format!("/query?{query}"),
                "service-secret",
                None
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
}
#[tokio::test]
async fn a_changed_credential_context_invalidates_existing_jwts() {
    let mut cfg = config();
    let auth = Arc::new(AuthProvider::new(&cfg, Arc::new(MemoryStateStore::new())).unwrap());
    let (token, _) = auth.login("alice", "password").await.unwrap();
    cfg.users[0].scope.tenant = "bank-other".into();
    auth.reload(&cfg).await.unwrap();
    assert_eq!(
        call(app(auth.clone()), "/query", &token, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (token, _) = auth.login("alice", "password").await.unwrap();
    assert_eq!(
        call(app(auth), "/query", &token, None).await.1["tenant"],
        "bank-other"
    );
}
#[tokio::test]
async fn broad_grants_do_not_expand_the_credential_context() {
    let auth = AuthProvider::new(&config(), Arc::new(MemoryStateStore::new())).unwrap();
    let identity = auth.authenticate_api_key("service-secret").await.unwrap();
    assert!(identity.can_manage_scope("bank-b", "prod"));
    for (namespace, tenant) in [
        ("prod", "bank-a"),
        ("prod", "bank-b.child"),
        ("dev", "bank-b"),
    ] {
        assert!(!identity.can_manage_scope(tenant, namespace));
        assert!(!identity.is_authorized(tenant, namespace, "email", "send"));
    }
    assert_eq!(
        identity
            .resolve_tenant_query_scope(None)
            .unwrap()
            .tenant
            .as_deref(),
        Some("bank-b")
    );
}
#[test]
fn ambiguous_or_ungranted_context_is_not_a_valid_credential_configuration() {
    for invalid in [
        TenantContext::new("*", "bank-b"),
        TenantContext::new("prod", ""),
        TenantContext::new("prod", "../bank-b"),
    ] {
        let mut cfg = config();
        cfg.api_keys[0].scope = invalid;
        assert!(AuthProvider::new(&cfg, Arc::new(MemoryStateStore::new())).is_err());
    }
    let mut cfg = config();
    cfg.api_keys[0].grants[0].tenants = vec!["bank-other".into()];
    assert!(AuthProvider::new(&cfg, Arc::new(MemoryStateStore::new())).is_err());
}

fn password_hash() -> String {
    use argon2::{Argon2, PasswordHasher, password_hash::SaltString};
    let salt = SaltString::encode_b64(b"credential-context-test").unwrap();
    Argon2::default()
        .hash_password(b"password", &salt)
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn resource_paths_bind_context_without_tenant_url_segments() {
    let auth = Arc::new(AuthProvider::new(&config(), Arc::new(MemoryStateStore::new())).unwrap());
    let (status, body) = call(app(auth.clone()), "/resources", "service-secret", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!(["prod", "bank-b"]));
    let (status, body) = call(
        app(auth.clone()),
        "/resources/job-123/2",
        "service-secret",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!(["prod", "bank-b", "job-123", 2]));
    let (status, _) = call(
        app(auth),
        "/resources/job-123/2?tenant=bank-b",
        "service-secret",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn explicit_api_key_header_supplies_context() {
    let auth = Arc::new(AuthProvider::new(&config(), Arc::new(MemoryStateStore::new())).unwrap());
    let response = app(auth)
        .oneshot(
            Request::builder()
                .uri("/resources")
                .header("x-api-key", "service-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap(),
        json!(["prod", "bank-b"])
    );
}

async fn real_app(auth: Arc<AuthProvider>, directory: &std::path::Path) -> Router {
    use acteon_audit::store::AuditStore;
    use acteon_server::api::AppState;
    use tokio::sync::RwLock;
    let store = Arc::new(MemoryStateStore::new());
    let audit = Arc::new(acteon_audit_memory::MemoryAuditStore::new());
    let dlq = Arc::new(acteon_executor::DeadLetterQueue::new());
    for (namespace, tenant, id) in [
        ("prod", "bank-a", "audit-a"),
        ("prod", "bank-b", "audit-b"),
        ("stage", "bank-a", "audit-stage"),
    ] {
        let now = chrono::Utc::now();
        let record: acteon_audit::record::AuditRecord = serde_json::from_value(json!({
            "id":id,"action_id":id,"namespace":namespace,"tenant":tenant,"provider":"email","action_type":"alert",
            "verdict":"allow","outcome":"executed","verdict_details":{},"outcome_details":{},"metadata":{},
            "dispatched_at":now,"completed_at":now,"duration_ms":0
        })).unwrap();
        audit.record(record).await.unwrap();
        dlq.push(
            acteon_core::Action::new(namespace, tenant, "email", "alert", json!({})),
            "test".into(),
            1,
        );
    }
    let gw = acteon_gateway::GatewayBuilder::new()
        .state(store.clone())
        .lock(Arc::new(acteon_state_memory::MemoryDistributedLock::new()))
        .audit(audit.clone())
        .dlq_sink(dlq)
        .build()
        .unwrap();
    let metrics = gw.metrics_arc();
    let timeline = Arc::new(
        acteon_timeline::TimelineService::new(
            Arc::new(acteon_timeline::memory::MemoryTimelineStore::default()),
            Arc::new(acteon_timeline::ledger::DiskLedger::open(directory, 10_000_000).unwrap()),
            store,
            acteon_timeline::config::TimelineConfig::default(),
            None,
        )
        .unwrap(),
    );
    let state = AppState {
        timeline: Some(timeline),
        gateway: Arc::new(RwLock::new(gw)),
        metrics,
        audit: Some(audit),
        analytics: None,
        auth: Some(auth),
        execution_authority: None,
        rate_limiter: None,
        embedding: None,
        embedding_metrics: None,
        connection_registry: None,
        a2a_discovery_cache: Arc::new(
            acteon_server::api::a2a_discovery_cache::DiscoveryCache::new(),
        ),
        dispatch_semaphore: Arc::new(tokio::sync::Semaphore::new(1000)),
        config: acteon_server::config::ConfigSnapshot::default(),
        static_quotas: None,
        static_templates: None,
        ui_path: None,
        ui_enabled: false,
        cors_allowed_origins: Vec::new(),
        signature_verifier: None,
        replay_protection: None,
        #[cfg(feature = "swarm")]
        swarm_registry: None,
        #[cfg(feature = "bus")]
        bus_backend: None,
        #[cfg(feature = "bus")]
        bus_schema_validator: acteon_bus::SchemaValidator::new(),
        #[cfg(feature = "bus")]
        bus_sessions: Arc::new(acteon_server::bus_sessions::BusSessionRegistry::default()),
    };
    acteon_server::api::router(state)
}

#[tokio::test]
async fn timeline_real_routes_isolate_same_event_keys_across_credentials() {
    let directory =
        std::env::temp_dir().join(format!("acteon-context-api-{}", uuid::Uuid::new_v4()));
    let mut cfg = config();
    let mut stage = config().api_keys.remove(0);
    stage.name = "bank-a-stage".into();
    stage.key_hash = hash_api_key("stage-secret").into();
    stage.scope = TenantContext::new("stage", "bank-a");
    cfg.api_keys.push(stage);
    let auth = Arc::new(AuthProvider::new(&cfg, Arc::new(MemoryStateStore::new())).unwrap());
    let (token, _) = auth.login("alice", "password").await.unwrap();
    let app = real_app(auth, &directory).await;
    for (credential, ns, tenant) in [
        (token.as_str(), "prod", "bank-a"),
        ("service-secret", "prod", "bank-b"),
        ("stage-secret", "stage", "bank-a"),
    ] {
        let (status, receipt) = call(app.clone(), "/v1/timeline/changes", credential, Some(json!({"change": {
            "schemaVersion":1,"changeId":"same-change","source":"transactions","eventId":"same-event","revision":1,"operation":"create",
            "after":{"eventGroup":"same-group","occurredAt":chrono::Utc::now().to_rfc3339(),"eventType":"suspicious",
                "attributes":{"tenant":"business-value", "context":format!("{ns}/{tenant}")}}
        }}))).await;
        assert_eq!(status, StatusCode::OK, "{receipt}");
    }
    for (credential, ns, tenant) in [
        (token.as_str(), "prod", "bank-a"),
        ("service-secret", "prod", "bank-b"),
        ("stage-secret", "stage", "bank-a"),
    ] {
        let (status, page) = call(
            app.clone(),
            "/v1/timeline/query",
            credential,
            Some(json!({"query":{"eventGroup":"same-group"}})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert_eq!(page["events"].as_array().unwrap().len(), 1);
        let event = &page["events"][0];
        assert_eq!(event["scope"], json!({"namespace":ns,"tenant":tenant}));
        assert_eq!(
            event["event"]["attributes"]["context"],
            format!("{ns}/{tenant}")
        );
        assert_eq!(event["event"]["attributes"]["tenant"], "business-value");
        let (status, event) = call(
            app.clone(),
            "/v1/timeline/events/transactions/same-event",
            credential,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(event["scope"], json!({"namespace":ns,"tenant":tenant}));
        let (status, _) = call(
            app.clone(),
            "/v1/timeline/query",
            credential,
            Some(json!({"scope":{"namespace":ns,"tenant":tenant},"query":{}})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    drop(app);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn jwt_context_is_required_and_signed_and_bound_to_current_user() {
    use base64::Engine;
    let auth = Arc::new(AuthProvider::new(&config(), Arc::new(MemoryStateStore::new())).unwrap());
    let (token, _) = auth.login("alice", "password").await.unwrap();
    let payload = token.split('.').nth(1).unwrap();
    let claims: Value = serde_json::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .unwrap(),
    )
    .unwrap();
    for context in [
        None,
        Some(json!({"namespace":"prod","tenant":"bank-b"})),
        Some(json!({"namespace":"*","tenant":"bank-a"})),
    ] {
        let mut changed = claims.clone();
        if let Some(context) = context {
            changed["scope"] = context;
        } else {
            changed.as_object_mut().unwrap().remove("scope");
        }
        // Even a correctly signed token cannot select a different current user context.
        for secret in ["context-test-secret", "attacker-secret"] {
            let forged = jsonwebtoken::encode(
                &jsonwebtoken::Header::default(),
                &changed,
                &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
            )
            .unwrap();
            let (status, _) = call(app(auth.clone()), "/resources", &forged, None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
    }
}

#[test]
fn openapi_input_contract_excludes_credential_context() {
    use utoipa::OpenApi;
    let document = serde_json::to_value(acteon_server::api::openapi::ApiDoc::openapi()).unwrap();
    for path in document["paths"].as_object().unwrap().keys() {
        assert!(
            !path.contains("{tenant}") && !path.contains("{namespace}"),
            "{path}"
        );
    }
    for (path, method) in [
        ("/v1/dispatch", "post"),
        ("/v1/timeline/query", "post"),
        ("/v1/timeline/changes", "post"),
        ("/v1/governance/permits", "post"),
        ("/v1/workforce/changes", "post"),
    ] {
        let schema = &document["paths"][path][method]["requestBody"]["content"]["application/json"]
            ["schema"];
        let schema = if let Some(reference) = schema["$ref"].as_str() {
            document.pointer(&reference[1..]).expect(reference)
        } else {
            schema
        };
        let properties = schema["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{path}: {schema}"));
        for field in ["namespace", "tenant", "scope", "tenant_scope"] {
            assert!(!properties.contains_key(field), "{path} exposes {field}");
        }
    }
}

#[tokio::test]
async fn audit_lookup_verification_and_dlq_are_credential_scoped() {
    let directory =
        std::env::temp_dir().join(format!("acteon-context-audit-{}", uuid::Uuid::new_v4()));
    let auth = Arc::new(AuthProvider::new(&config(), Arc::new(MemoryStateStore::new())).unwrap());
    let (token, _) = auth.login("alice", "password").await.unwrap();
    let app = real_app(auth, &directory).await;
    for (credential, own, other) in [
        (token.as_str(), "audit-a", "audit-b"),
        ("service-secret", "audit-b", "audit-a"),
    ] {
        let (status, body) = call(app.clone(), "/v1/audit", credential, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["records"].as_array().unwrap().len(), 1);
        assert_eq!(body["records"][0]["action_id"], own);
        let (status, _) = call(
            app.clone(),
            &format!("/v1/actions/{own}/verify"),
            credential,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        for id in [other, "audit-stage"] {
            let (status, _) = call(
                app.clone(),
                &format!("/v1/actions/{id}/verify"),
                credential,
                None,
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            let (status, _) = call(app.clone(), &format!("/v1/audit/{id}"), credential, None).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
        }
        let (status, stats) = call(app.clone(), "/v1/dlq/stats", credential, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(stats["count"], 1);
        let (status, entries) =
            call(app.clone(), "/v1/dlq/drain", credential, Some(json!({}))).await;
        assert_eq!(status, StatusCode::OK, "{entries}");
        assert_eq!(entries["count"], 1);
        let (_, stats) = call(app.clone(), "/v1/dlq/stats", credential, None).await;
        assert_eq!(stats["count"], 0);
    }
    drop(app);
    std::fs::remove_dir_all(directory).unwrap();
}
