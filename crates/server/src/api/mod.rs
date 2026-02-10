pub mod approvals;
pub mod audit;
pub mod auth;
pub mod chains;
pub mod circuit_breakers;
pub mod config;
pub mod dispatch;
pub mod dlq;
pub mod embeddings;
pub mod events;
pub mod groups;
pub mod health;
pub mod openapi;
pub mod recurring;
pub mod replay;
pub mod rules;
pub mod schemas;
pub mod stream;
pub mod subscribe;
pub mod trace_context;

use std::sync::Arc;

use axum::Router;
use axum::middleware;
use axum::routing::{delete, get, post, put};
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use acteon_audit::store::AuditStore;
use acteon_embedding::EmbeddingMetrics;
use acteon_gateway::Gateway;
use acteon_rules::EmbeddingEvalSupport;

use self::stream::ConnectionRegistry;

use crate::auth::AuthProvider;
use crate::auth::middleware::AuthLayer;
use crate::config::ConfigSnapshot;
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
    /// Per-tenant SSE connection limit registry.
    pub connection_registry: Option<Arc<ConnectionRegistry>>,
    /// Sanitized configuration snapshot (secrets masked).
    pub config: ConfigSnapshot,
    /// Path to the Admin UI static files.
    pub ui_path: Option<String>,
    /// Whether the Admin UI is enabled.
    pub ui_enabled: bool,
}

/// Build the Axum router with all API routes, middleware, and Swagger UI.
#[allow(clippy::too_many_lines)]
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
        .route("/v1/audit/replay", post(replay::replay_audit))
        .route("/v1/audit/{action_id}", get(audit::get_audit_by_action))
        .route("/v1/audit/{action_id}/replay", post(replay::replay_action))
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
        // Recurring actions
        .route(
            "/v1/recurring",
            get(recurring::list_recurring).post(recurring::create_recurring),
        )
        .route(
            "/v1/recurring/{id}",
            get(recurring::get_recurring)
                .put(recurring::update_recurring)
                .delete(recurring::delete_recurring),
        )
        .route("/v1/recurring/{id}/pause", post(recurring::pause_recurring))
        .route(
            "/v1/recurring/{id}/resume",
            post(recurring::resume_recurring),
        )
        // Embeddings
        .route("/v1/embeddings/similarity", post(embeddings::similarity))
        // Approvals (list requires auth)
        .route("/v1/approvals", get(approvals::list_approvals))
        // Circuit breaker admin
        .route(
            "/admin/circuit-breakers",
            get(circuit_breakers::list_circuit_breakers),
        )
        .route(
            "/admin/circuit-breakers/{provider}/trip",
            post(circuit_breakers::trip_circuit_breaker),
        )
        .route(
            "/admin/circuit-breakers/{provider}/reset",
            post(circuit_breakers::reset_circuit_breaker),
        )
        // Admin config
        .route("/admin/config", get(config::get_config))
        // SSE event streaming
        .route("/v1/stream", get(stream::stream))
        // Entity-specific SSE subscription
        .route(
            "/v1/subscribe/{entity_type}/{entity_id}",
            get(subscribe::subscribe),
        )
        // Logout (requires auth)
        .route("/v1/auth/logout", post(auth::logout))
        // Rate limiting runs after auth (so CallerIdentity is available)
        .layer(RateLimitLayer::new(state.rate_limiter.clone()))
        .layer(AuthLayer::new(state.auth.clone()));

    let mut router = Router::new()
        .merge(public)
        .merge(protected)
        // Swagger UI must be merged BEFORE the UI fallback, otherwise the fallback
        // will swallow /swagger-ui requests.
        .merge(SwaggerUi::new("/swagger-ui").url("/api-doc/openapi.json", ApiDoc::openapi()));

    // Serve Admin UI static files if enabled and path is provided.
    if let Some(path_str) = state.ui_path.as_ref().filter(|_| state.ui_enabled) {
        let path = std::path::PathBuf::from(path_str);
        if path.exists() {
            let index_path = path.join("index.html");
            router = router.fallback_service(
                ServeDir::new(path).fallback(tower_http::services::ServeFile::new(index_path)),
            );
        } else {
            tracing::warn!(
                path = %path.display(),
                "Admin UI directory not found, UI will not be served"
            );
        }
    }

    router
        .with_state(state)
        // W3C Trace Context propagation (extracts traceparent/tracestate from
        // incoming requests so OTel can link server spans to the caller's trace).
        .layer(middleware::from_fn(trace_context::propagate_trace_context))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
}
