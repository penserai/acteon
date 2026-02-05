use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{self, Request, StatusCode};
use tokio::sync::RwLock;
use tower::ServiceExt;

use acteon_audit::store::AuditStore;
use acteon_audit_memory::MemoryAuditStore;
use acteon_core::{Action, ProviderResponse};
use acteon_executor::ExecutorConfig;
use acteon_gateway::GatewayBuilder;
use acteon_provider::{DynProvider, ProviderError};
use acteon_rules::ir::expr::{BinaryOp, Expr};
use acteon_rules::ir::rule::{Rule, RuleAction};
use acteon_server::api::AppState;
use acteon_state::StateStore;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

// -- Mock provider --------------------------------------------------------

struct MockProvider {
    provider_name: String,
}

impl MockProvider {
    fn new(name: &str) -> Self {
        Self {
            provider_name: name.to_owned(),
        }
    }
}

#[async_trait]
impl DynProvider for MockProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    async fn execute(&self, _action: &Action) -> Result<ProviderResponse, ProviderError> {
        Ok(ProviderResponse::success(serde_json::json!({"ok": true})))
    }

    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}

// -- Helpers --------------------------------------------------------------

fn build_test_state(rules: Vec<Rule>) -> AppState {
    build_test_state_with_audit(rules, None)
}

fn build_test_state_with_audit(rules: Vec<Rule>, audit: Option<Arc<dyn AuditStore>>) -> AppState {
    let store = Arc::new(MemoryStateStore::new());
    let lock = Arc::new(MemoryDistributedLock::new());

    let mut builder = GatewayBuilder::new()
        .state(store)
        .lock(lock)
        .rules(rules)
        .provider(Arc::new(MockProvider::new("email")))
        .executor_config(ExecutorConfig {
            max_retries: 0,
            execution_timeout: Duration::from_secs(5),
            max_concurrent: 10,
            ..ExecutorConfig::default()
        });

    if let Some(ref a) = audit {
        builder = builder.audit(Arc::clone(a)).audit_store_payload(true);
    }

    let gw = builder.build().expect("gateway should build");

    AppState {
        gateway: Arc::new(RwLock::new(gw)),
        audit,
        auth: None,
        rate_limiter: None,
    }
}

fn test_action() -> Action {
    Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_email",
        serde_json::json!({"to": "user@example.com"}),
    )
}

fn build_app(state: AppState) -> axum::Router {
    acteon_server::api::router(state)
}

// -- Tests ----------------------------------------------------------------

#[tokio::test]
async fn health_returns_200() {
    let state = build_test_state(vec![]);
    let app = build_app(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
    assert!(json["metrics"].is_object());
}

#[tokio::test]
async fn metrics_returns_200() {
    let state = build_test_state(vec![]);
    let app = build_app(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["dispatched"], 0);
}

#[tokio::test]
async fn dispatch_returns_valid_outcome() {
    let state = build_test_state(vec![]);
    let app = build_app(state);

    let action = test_action();
    let body = serde_json::to_string(&action).unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri("/v1/dispatch")
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // No rules means Allow -> Executed
    assert!(
        json.get("Executed").is_some(),
        "expected Executed, got {json}"
    );
}

#[tokio::test]
async fn dispatch_batch_returns_array() {
    let state = build_test_state(vec![]);
    let app = build_app(state);

    let actions = vec![test_action(), test_action()];
    let body = serde_json::to_string(&actions).unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri("/v1/dispatch/batch")
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.is_array());
    assert_eq!(json.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn list_rules_returns_rule_list() {
    let rules = vec![
        Rule::new("rule-a", Expr::Bool(true), RuleAction::Allow)
            .with_priority(10)
            .with_description("First rule"),
        Rule::new("rule-b", Expr::Bool(false), RuleAction::Deny).with_priority(5),
    ];
    let state = build_test_state(rules);
    let app = build_app(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/rules")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let arr = json.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    // Rules should be sorted by priority: rule-b(5), rule-a(10)
    assert_eq!(arr[0]["name"], "rule-b");
    assert_eq!(arr[1]["name"], "rule-a");
    assert_eq!(arr[1]["description"], "First rule");
}

#[tokio::test]
async fn reload_rules_returns_200() {
    let state = build_test_state(vec![]);
    let app = build_app(state);

    // Create a temporary directory with a YAML rule file.
    let tmpdir = std::env::temp_dir().join("acteon-test-rules");
    let _ = std::fs::create_dir_all(&tmpdir);
    let rule_file = tmpdir.join("test.yaml");
    std::fs::write(
        &rule_file,
        r#"
rules:
  - name: test-rule
    priority: 1
    condition:
      field: action.action_type
      eq: "send_email"
    action:
      type: allow
"#,
    )
    .unwrap();

    let body = serde_json::json!({
        "directory": tmpdir.to_str().unwrap()
    });

    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri("/v1/rules/reload")
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["reloaded"], 1);

    // Clean up.
    let _ = std::fs::remove_dir_all(&tmpdir);
}

#[tokio::test]
async fn set_rule_enabled_toggles() {
    let rules = vec![Rule::new("toggle-me", Expr::Bool(true), RuleAction::Allow)];
    let state = build_test_state(rules);

    // First, disable the rule.
    let app = build_app(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::PUT)
                .uri("/v1/rules/toggle-me/enabled")
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"enabled": false}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["enabled"], false);
    assert_eq!(json["name"], "toggle-me");

    // Verify the rule is actually disabled.
    {
        let gw = state.gateway.read().await;
        assert!(!gw.rules()[0].enabled);
    }

    // Re-enable -- rebuild the router since `oneshot` consumes it.
    let app2 = build_app(state.clone());
    let response = app2
        .oneshot(
            Request::builder()
                .method(http::Method::PUT)
                .uri("/v1/rules/toggle-me/enabled")
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"enabled": true}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["enabled"], true);

    // Verify the rule is enabled again.
    {
        let gw = state.gateway.read().await;
        assert!(gw.rules()[0].enabled);
    }
}

#[tokio::test]
async fn set_rule_enabled_not_found() {
    let state = build_test_state(vec![]);
    let app = build_app(state);

    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::PUT)
                .uri("/v1/rules/nonexistent/enabled")
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"enabled": true}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn swagger_ui_returns_200() {
    let state = build_test_state(vec![]);
    let app = build_app(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/swagger-ui/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8_lossy(&body);
    assert!(html.contains("swagger"), "expected Swagger UI HTML");
}

#[tokio::test]
async fn openapi_json_is_valid() {
    let state = build_test_state(vec![]);
    let app = build_app(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api-doc/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let spec: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Verify it's an OpenAPI 3.1 document
    assert!(
        spec["openapi"].as_str().unwrap().starts_with("3.1"),
        "expected OpenAPI 3.1.x, got {}",
        spec["openapi"]
    );

    // Verify all expected paths are present
    let paths = spec["paths"]
        .as_object()
        .expect("paths should be an object");
    assert!(paths.contains_key("/health"), "missing /health");
    assert!(paths.contains_key("/metrics"), "missing /metrics");
    assert!(paths.contains_key("/v1/dispatch"), "missing /v1/dispatch");
    assert!(
        paths.contains_key("/v1/dispatch/batch"),
        "missing /v1/dispatch/batch"
    );
    assert!(paths.contains_key("/v1/rules"), "missing /v1/rules");
    assert!(
        paths.contains_key("/v1/rules/reload"),
        "missing /v1/rules/reload"
    );
    assert!(
        paths.contains_key("/v1/rules/{name}/enabled"),
        "missing /v1/rules/{{name}}/enabled"
    );
    // Audit paths
    assert!(paths.contains_key("/v1/audit"), "missing /v1/audit");
    assert!(
        paths.contains_key("/v1/audit/{action_id}"),
        "missing /v1/audit/{{action_id}}"
    );

    // Verify schemas exist
    let schemas = spec["components"]["schemas"]
        .as_object()
        .expect("schemas should be an object");
    assert!(schemas.contains_key("Action"), "missing Action schema");
    assert!(
        schemas.contains_key("ActionOutcome"),
        "missing ActionOutcome schema"
    );
    assert!(
        schemas.contains_key("HealthResponse"),
        "missing HealthResponse schema"
    );
    assert!(
        schemas.contains_key("ErrorResponse"),
        "missing ErrorResponse schema"
    );
    assert!(
        schemas.contains_key("AuditRecord"),
        "missing AuditRecord schema"
    );
    assert!(
        schemas.contains_key("AuditPage"),
        "missing AuditPage schema"
    );
}

// -- Audit-specific tests -------------------------------------------------

#[tokio::test]
async fn audit_disabled_returns_404() {
    let state = build_test_state(vec![]); // no audit
    let app = build_app(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/audit")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn audit_query_returns_records_after_dispatch() {
    let audit: Arc<dyn AuditStore> = Arc::new(MemoryAuditStore::new());
    let state = build_test_state_with_audit(vec![], Some(Arc::clone(&audit)));

    // Dispatch an action.
    let action = test_action();
    let action_body = serde_json::to_string(&action).unwrap();
    let app = build_app(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri("/v1/dispatch")
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(action_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Give the async audit task time to complete.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Query audit records.
    let app2 = build_app(state);
    let response = app2
        .oneshot(
            Request::builder()
                .uri("/v1/audit")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["total"].as_u64().unwrap() >= 1);
    let records = json["records"].as_array().unwrap();
    assert!(!records.is_empty());
    assert_eq!(records[0]["verdict"], "allow");
    assert_eq!(records[0]["outcome"], "executed");
}

#[tokio::test]
async fn audit_get_by_action_id() {
    let audit: Arc<dyn AuditStore> = Arc::new(MemoryAuditStore::new());
    let state = build_test_state_with_audit(vec![], Some(Arc::clone(&audit)));

    let action = test_action();
    let action_id = action.id.to_string();
    let action_body = serde_json::to_string(&action).unwrap();

    // Dispatch the action.
    let app = build_app(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri("/v1/dispatch")
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(action_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Give the async audit task time to complete.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Look up by action ID.
    let app2 = build_app(state);
    let response = app2
        .oneshot(
            Request::builder()
                .uri(format!("/v1/audit/{action_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["action_id"], action_id);
    assert_eq!(json["namespace"], "notifications");
    assert_eq!(json["provider"], "email");
}

#[tokio::test]
async fn audit_get_nonexistent_returns_404() {
    let audit: Arc<dyn AuditStore> = Arc::new(MemoryAuditStore::new());
    let state = build_test_state_with_audit(vec![], Some(audit));

    let app = build_app(state);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/audit/nonexistent-action-id")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn audit_query_filters_work() {
    let audit: Arc<dyn AuditStore> = Arc::new(MemoryAuditStore::new());
    let state = build_test_state_with_audit(vec![], Some(Arc::clone(&audit)));

    // Dispatch an action.
    let action = test_action();
    let action_body = serde_json::to_string(&action).unwrap();
    let app = build_app(state.clone());
    let _ = app
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri("/v1/dispatch")
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(action_body))
                .unwrap(),
        )
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Query with matching filter.
    let app2 = build_app(state.clone());
    let response = app2
        .oneshot(
            Request::builder()
                .uri("/v1/audit?namespace=notifications")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["total"].as_u64().unwrap() >= 1);

    // Query with non-matching filter.
    let app3 = build_app(state);
    let response = app3
        .oneshot(
            Request::builder()
                .uri("/v1/audit?namespace=other-ns")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["total"].as_u64().unwrap(), 0);
}

#[tokio::test]
async fn dispatch_without_audit_still_works() {
    let state = build_test_state(vec![]); // no audit
    let app = build_app(state);

    let action = test_action();
    let body = serde_json::to_string(&action).unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri("/v1/dispatch")
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("Executed").is_some());
}

// -- Approval REST API helpers ------------------------------------------------

struct FailingMockProvider {
    provider_name: String,
}

impl FailingMockProvider {
    fn new(name: &str) -> Self {
        Self {
            provider_name: name.to_owned(),
        }
    }
}

#[async_trait]
impl DynProvider for FailingMockProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    async fn execute(&self, _action: &Action) -> Result<ProviderResponse, ProviderError> {
        Err(ProviderError::ExecutionFailed("provider down".into()))
    }

    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}

fn refund_condition() -> Expr {
    Expr::Binary(
        BinaryOp::Eq,
        Box::new(Expr::Field(
            Box::new(Expr::Ident("action".into())),
            "action_type".into(),
        )),
        Box::new(Expr::String("process_refund".into())),
    )
}

fn approval_rule(timeout_seconds: u64) -> Rule {
    Rule::new(
        "approve-refunds",
        refund_condition(),
        RuleAction::RequestApproval {
            notify_provider: "slack".into(),
            timeout_seconds,
            message: Some("Requires approval".into()),
        },
    )
}

fn build_approval_state(rules: Vec<Rule>) -> AppState {
    build_approval_state_with_providers(
        rules,
        vec![
            Arc::new(MockProvider::new("payments")) as Arc<dyn DynProvider>,
            Arc::new(MockProvider::new("slack")),
        ],
    )
}

fn build_approval_state_with_providers(
    rules: Vec<Rule>,
    providers: Vec<Arc<dyn DynProvider>>,
) -> AppState {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let lock = Arc::new(MemoryDistributedLock::new());

    let mut builder = GatewayBuilder::new()
        .state(store)
        .lock(lock)
        .rules(rules)
        .approval_secret(b"test-secret-key-for-approvals!!")
        .external_url("https://test.example.com")
        .executor_config(ExecutorConfig {
            max_retries: 0,
            execution_timeout: Duration::from_secs(5),
            max_concurrent: 10,
            ..ExecutorConfig::default()
        });
    for p in providers {
        builder = builder.provider(p);
    }

    let gw = builder.build().expect("gateway should build");

    AppState {
        gateway: Arc::new(RwLock::new(gw)),
        audit: None,
        auth: None,
        rate_limiter: None,
    }
}

fn refund_action() -> Action {
    Action::new(
        "payments",
        "tenant-1",
        "payments",
        "process_refund",
        serde_json::json!({"order_id": "ORD-123", "amount": 99.99}),
    )
}

fn parse_query_param(url: &str, param: &str) -> Option<String> {
    let query = url.split('?').nth(1)?;
    for pair in query.split('&') {
        let mut kv = pair.splitn(2, '=');
        if kv.next()? == param {
            return kv.next().map(String::from);
        }
    }
    None
}

/// Helper: dispatch a refund action and return (approval_id, approve_url, reject_url).
async fn dispatch_refund_and_get_pending(state: &AppState) -> (String, String, String) {
    let app = build_app(state.clone());
    let action = refund_action();
    let body = serde_json::to_string(&action).unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri("/v1/dispatch")
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let pending = json
        .get("PendingApproval")
        .expect("expected PendingApproval outcome");

    let approval_id = pending["approval_id"].as_str().unwrap().to_string();
    let approve_url = pending["approve_url"].as_str().unwrap().to_string();
    let reject_url = pending["reject_url"].as_str().unwrap().to_string();

    (approval_id, approve_url, reject_url)
}

// -- Approval REST API tests --------------------------------------------------

#[tokio::test]
async fn approval_dispatch_returns_pending_with_signed_urls() {
    let state = build_approval_state(vec![approval_rule(3600)]);
    let (approval_id, approve_url, reject_url) = dispatch_refund_and_get_pending(&state).await;

    assert!(!approval_id.is_empty());
    assert!(
        approve_url.starts_with("https://test.example.com/v1/approvals/"),
        "approve_url should start with external_url prefix, got {approve_url}"
    );
    assert!(
        reject_url.starts_with("https://test.example.com/v1/approvals/"),
        "reject_url should start with external_url prefix, got {reject_url}"
    );
    assert!(parse_query_param(&approve_url, "sig").is_some());
    assert!(parse_query_param(&approve_url, "expires_at").is_some());
    assert!(parse_query_param(&reject_url, "sig").is_some());
    assert!(parse_query_param(&reject_url, "expires_at").is_some());
    assert!(
        parse_query_param(&approve_url, "kid").is_some(),
        "approve_url should contain kid parameter"
    );
    assert!(
        parse_query_param(&reject_url, "kid").is_some(),
        "reject_url should contain kid parameter"
    );
}

#[tokio::test]
async fn approval_approve_via_rest_executes_action() {
    let state = build_approval_state(vec![approval_rule(3600)]);
    let (approval_id, approve_url, _) = dispatch_refund_and_get_pending(&state).await;

    let sig = parse_query_param(&approve_url, "sig").unwrap();
    let expires_at = parse_query_param(&approve_url, "expires_at").unwrap();
    let kid = parse_query_param(&approve_url, "kid").unwrap();

    // POST /v1/approvals/{ns}/{tenant}/{id}/approve?sig=...&expires_at=...&kid=...
    let app = build_app(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri(format!(
                    "/v1/approvals/payments/tenant-1/{approval_id}/approve?sig={sig}&expires_at={expires_at}&kid={kid}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "approved");
    assert!(
        json["outcome"].is_object(),
        "approved action should have execution outcome"
    );

    // Verify status via GET
    let app2 = build_app(state);
    let response = app2
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v1/approvals/payments/tenant-1/{approval_id}?sig={sig}&expires_at={expires_at}&kid={kid}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "approved");
}

#[tokio::test]
async fn approval_reject_via_rest_updates_status() {
    let state = build_approval_state(vec![approval_rule(3600)]);
    let (approval_id, _, reject_url) = dispatch_refund_and_get_pending(&state).await;

    let sig = parse_query_param(&reject_url, "sig").unwrap();
    let expires_at = parse_query_param(&reject_url, "expires_at").unwrap();
    let kid = parse_query_param(&reject_url, "kid").unwrap();

    // POST /v1/approvals/{ns}/{tenant}/{id}/reject?sig=...&expires_at=...&kid=...
    let app = build_app(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri(format!(
                    "/v1/approvals/payments/tenant-1/{approval_id}/reject?sig={sig}&expires_at={expires_at}&kid={kid}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "rejected");
    assert!(
        json["outcome"].is_null(),
        "rejected action should have no execution outcome"
    );

    // Verify status via GET
    let app2 = build_app(state);
    let response = app2
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v1/approvals/payments/tenant-1/{approval_id}?sig={sig}&expires_at={expires_at}&kid={kid}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "rejected");
}

#[tokio::test]
async fn approval_notification_failure_still_creates_pending() {
    let state = build_approval_state_with_providers(
        vec![approval_rule(3600)],
        vec![
            Arc::new(MockProvider::new("payments")) as Arc<dyn DynProvider>,
            Arc::new(FailingMockProvider::new("slack")),
        ],
    );

    let app = build_app(state);
    let action = refund_action();
    let body = serde_json::to_string(&action).unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri("/v1/dispatch")
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let pending = json
        .get("PendingApproval")
        .expect("should still return PendingApproval even with notification failure");

    assert_eq!(
        pending["notification_sent"], false,
        "notification_sent should be false when slack provider fails"
    );
    assert!(
        !pending["approval_id"].as_str().unwrap().is_empty(),
        "approval_id should still be present"
    );
}

#[tokio::test]
async fn approval_expired_link_returns_404() {
    let state = build_approval_state(vec![approval_rule(2)]);
    let (approval_id, approve_url, _) = dispatch_refund_and_get_pending(&state).await;

    let sig = parse_query_param(&approve_url, "sig").unwrap();
    let expires_at = parse_query_param(&approve_url, "expires_at").unwrap();
    let kid = parse_query_param(&approve_url, "kid").unwrap();

    // Wait for the approval to expire (2-second timeout + buffer).
    tokio::time::sleep(Duration::from_secs(3)).await;

    let app = build_app(state);
    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri(format!(
                    "/v1/approvals/payments/tenant-1/{approval_id}/approve?sig={sig}&expires_at={expires_at}&kid={kid}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "expired approval should return 404"
    );
}
