use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{self, Request, StatusCode};
use tokio::sync::RwLock;
use tower::ServiceExt;

use acteon_core::{Action, ProviderResponse};
use acteon_executor::ExecutorConfig;
use acteon_gateway::GatewayBuilder;
use acteon_provider::{DynProvider, ProviderError};
use acteon_rules::ir::expr::Expr;
use acteon_rules::ir::rule::{Rule, RuleAction};
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

fn build_test_gateway(rules: Vec<Rule>) -> Arc<RwLock<acteon_gateway::Gateway>> {
    let store = Arc::new(MemoryStateStore::new());
    let lock = Arc::new(MemoryDistributedLock::new());

    let gw = GatewayBuilder::new()
        .state(store)
        .lock(lock)
        .rules(rules)
        .provider(Arc::new(MockProvider::new("email")))
        .executor_config(ExecutorConfig {
            max_retries: 0,
            execution_timeout: Duration::from_secs(5),
            max_concurrent: 10,
            ..ExecutorConfig::default()
        })
        .build()
        .expect("gateway should build");

    Arc::new(RwLock::new(gw))
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

fn build_app(gateway: Arc<RwLock<acteon_gateway::Gateway>>) -> axum::Router {
    acteon_server::api::router(gateway)
}

// -- Tests ----------------------------------------------------------------

#[tokio::test]
async fn health_returns_200() {
    let gateway = build_test_gateway(vec![]);
    let app = build_app(gateway);

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
    let gateway = build_test_gateway(vec![]);
    let app = build_app(gateway);

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
    let gateway = build_test_gateway(vec![]);
    let app = build_app(gateway);

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
    let gateway = build_test_gateway(vec![]);
    let app = build_app(gateway);

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
    let gateway = build_test_gateway(rules);
    let app = build_app(gateway);

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
    let gateway = build_test_gateway(vec![]);
    let app = build_app(gateway);

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
    let gateway = build_test_gateway(rules);

    // First, disable the rule.
    let app = build_app(Arc::clone(&gateway));
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
        let gw = gateway.read().await;
        assert!(!gw.rules()[0].enabled);
    }

    // Re-enable -- rebuild the router since `oneshot` consumes it.
    let app2 = build_app(Arc::clone(&gateway));
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
        let gw = gateway.read().await;
        assert!(gw.rules()[0].enabled);
    }
}

#[tokio::test]
async fn set_rule_enabled_not_found() {
    let gateway = build_test_gateway(vec![]);
    let app = build_app(gateway);

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
    let gateway = build_test_gateway(vec![]);
    let app = build_app(gateway);

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
    let gateway = build_test_gateway(vec![]);
    let app = build_app(gateway);

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
}
