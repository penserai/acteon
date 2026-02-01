pub mod dispatch;
pub mod health;
pub mod rules;

use std::sync::Arc;

use axum::routing::{get, post, put};
use axum::Router;
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use acteon_gateway::Gateway;

/// Build the Axum router with all API routes and middleware.
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
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
}
