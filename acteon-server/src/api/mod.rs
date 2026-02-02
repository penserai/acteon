pub mod dispatch;
pub mod health;
pub mod openapi;
pub mod rules;
pub mod schemas;

use std::sync::Arc;

use axum::routing::{get, post, put};
use axum::Router;
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use acteon_gateway::Gateway;

use self::openapi::ApiDoc;

/// Build the Axum router with all API routes, middleware, and Swagger UI.
pub fn router(gateway: Arc<RwLock<Gateway>>) -> Router {
    Router::new()
        // Health & metrics
        .route("/health", get(health::health))
        .route("/metrics", get(health::metrics))
        // Dispatch
        .route("/v1/dispatch", post(dispatch::dispatch))
        .route("/v1/dispatch/batch", post(dispatch::dispatch_batch))
        // Rules management
        .route("/v1/rules", get(rules::list_rules))
        .route("/v1/rules/reload", post(rules::reload_rules))
        .route("/v1/rules/{name}/enabled", put(rules::set_rule_enabled))
        .with_state(gateway)
        // Swagger UI
        .merge(SwaggerUi::new("/swagger-ui").url("/api-doc/openapi.json", ApiDoc::openapi()))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
}
