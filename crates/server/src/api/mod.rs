pub mod approvals;
pub mod audit;
pub mod auth;
pub mod chains;
pub mod dispatch;
pub mod dlq;
pub mod embeddings;
pub mod events;
pub mod groups;
pub mod health;
pub mod openapi;
pub mod rules;
pub mod schemas;

use std::sync::Arc;

use axum::Router;
use axum::routing::{delete, get, post, put};
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use acteon_audit::store::AuditStore;
use acteon_embedding::EmbeddingMetrics;
use acteon_gateway::Gateway;
use acteon_rules::EmbeddingEvalSupport;

use crate::auth::AuthProvider;
use crate::auth::middleware::AuthLayer;
use crate::ratelimit::RateLimiter;
use crate::ratelimit::middleware::RateLimitLayer;

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
    /// Optional embedding support for similarity testing (None when embedding is disabled).
    pub embedding: Option<Arc<dyn EmbeddingEvalSupport>>,
    /// Optional embedding metrics handle (None when embedding is disabled).
    pub embedding_metrics: Option<Arc<EmbeddingMetrics>>,
}

/// Build the Axum router with all API routes, middleware, and Swagger UI.
pub fn router(state: AppState) -> Router {
    let public = Router::new()
        // Health & metrics (always public)
        .route("/health", get(health::health))
        .route("/metrics", get(health::metrics))
        // Login (must be public)
        .route("/v1/auth/login", post(auth::login))
        // Approvals (public, HMAC-authenticated via query signature)
        .route(
            "/v1/approvals/{namespace}/{tenant}/{id}/approve",
            post(approvals::approve),
        )
        .route(
            "/v1/approvals/{namespace}/{tenant}/{id}/reject",
            post(approvals::reject),
        )
        .route(
            "/v1/approvals/{namespace}/{tenant}/{id}",
            get(approvals::get_approval),
        );

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
        // Dead-letter queue
        .route("/v1/dlq/stats", get(dlq::dlq_stats))
        .route("/v1/dlq/drain", post(dlq::dlq_drain))
        // Events (state machine lifecycle)
        .route("/v1/events", get(events::list_events))
        .route("/v1/events/{fingerprint}", get(events::get_event))
        .route(
            "/v1/events/{fingerprint}/transition",
            put(events::transition_event),
        )
        // Groups (event batching)
        .route("/v1/groups", get(groups::list_groups))
        .route("/v1/groups/{group_key}", get(groups::get_group))
        .route("/v1/groups/{group_key}", delete(groups::flush_group))
        // Chains (task chain orchestration)
        .route("/v1/chains", get(chains::list_chains))
        .route("/v1/chains/{chain_id}", get(chains::get_chain))
        .route("/v1/chains/{chain_id}/cancel", post(chains::cancel_chain))
        // Embeddings
        .route("/v1/embeddings/similarity", post(embeddings::similarity))
        // Approvals (list requires auth)
        .route("/v1/approvals", get(approvals::list_approvals))
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
