pub mod audit;
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

use acteon_audit::store::AuditStore;
use acteon_gateway::Gateway;

use self::openapi::ApiDoc;

/// Shared application state passed to all handlers.
#[derive(Clone)]
pub struct AppState {
    /// The gateway instance.
    pub gateway: Arc<RwLock<Gateway>>,
    /// Optional audit store (None when audit is disabled).
    pub audit: Option<Arc<dyn AuditStore>>,
}

/// Build the Axum router with all API routes, middleware, and Swagger UI.
pub fn router(state: AppState) -> Router {
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
        // Audit
        .route("/v1/audit", get(audit::query_audit))
        .route("/v1/audit/{action_id}", get(audit::get_audit_by_action))
        .with_state(state)
        // Swagger UI
        .merge(SwaggerUi::new("/swagger-ui").url("/api-doc/openapi.json", ApiDoc::openapi()))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
}
