pub mod audit;
pub mod auth;
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

use crate::auth::middleware::AuthLayer;
use crate::auth::AuthProvider;
use crate::ratelimit::middleware::RateLimitLayer;
use crate::ratelimit::RateLimiter;

use self::openapi::ApiDoc;

/// Shared application state passed to all handlers.
#[derive(Clone)]
pub struct AppState {
    /// The gateway instance.
    pub gateway: Arc<RwLock<Gateway>>,
    /// Optional audit store (None when audit is disabled).
    pub audit: Option<Arc<dyn AuditStore>>,
    /// Optional auth provider (None when auth is disabled).
    pub auth: Option<Arc<AuthProvider>>,
    /// Optional rate limiter (None when rate limiting is disabled).
    pub rate_limiter: Option<Arc<RateLimiter>>,
}

/// Build the Axum router with all API routes, middleware, and Swagger UI.
pub fn router(state: AppState) -> Router {
    let public = Router::new()
        // Health & metrics (always public)
        .route("/health", get(health::health))
        .route("/metrics", get(health::metrics))
        // Login (must be public)
        .route("/v1/auth/login", post(auth::login));

    let protected = Router::new()
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
        // Logout (requires auth)
        .route("/v1/auth/logout", post(auth::logout))
        // Rate limiting runs after auth (so CallerIdentity is available)
        .layer(RateLimitLayer::new(state.rate_limiter.clone()))
        .layer(AuthLayer::new(state.auth.clone()));

    Router::new()
        .merge(public)
        .merge(protected)
        .with_state(state)
        // Swagger UI
        .merge(SwaggerUi::new("/swagger-ui").url("/api-doc/openapi.json", ApiDoc::openapi()))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
}
