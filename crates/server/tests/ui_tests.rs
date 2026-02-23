use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceExt;

use acteon_executor::ExecutorConfig;
use acteon_gateway::GatewayBuilder;
use acteon_server::api::AppState;
use acteon_server::config::{ConfigSnapshot, UiConfig};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

#[tokio::test]
async fn ui_serves_index_html() {
    // 1. Create a dummy UI directory
    let tmpdir = std::env::temp_dir().join("acteon-test-ui");
    let _ = std::fs::create_dir_all(&tmpdir);
    let index_file = tmpdir.join("index.html");
    std::fs::write(&index_file, "<html><body>Acteon UI</body></html>").unwrap();

    // 2. Build test state with UI enabled
    let store = Arc::new(MemoryStateStore::new());
    let lock = Arc::new(MemoryDistributedLock::new());
    let gw = GatewayBuilder::new()
        .state(store)
        .lock(lock)
        .executor_config(ExecutorConfig::default())
        .build()
        .expect("gateway should build");

    let ui_config = UiConfig {
        enabled: true,
        dist_path: tmpdir.to_str().unwrap().to_owned(),
    };

    let state = AppState {
        gateway: Arc::new(RwLock::new(gw)),
        audit: None,
        analytics: None,
        auth: None,
        rate_limiter: None,
        embedding: None,
        embedding_metrics: None,
        connection_registry: None,
        config: ConfigSnapshot::default(),
        ui_path: Some(ui_config.dist_path.clone()),
        ui_enabled: ui_config.enabled,
    };

    let app = acteon_server::api::router(state);

    // 3. Test root path serves index.html
    let response = app
        .clone()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("Acteon UI"));

    // 4. Test SPA fallback (unknown path should serve index.html)
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/some-random-page")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("Acteon UI"));

    // 5. Test Swagger UI is still accessible (not swallowed by UI fallback)
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
    assert!(
        html.contains("swagger"),
        "expected Swagger UI HTML, got: {html}"
    );

    // Clean up
    let _ = std::fs::remove_dir_all(&tmpdir);
}
