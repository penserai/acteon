pub mod a2a;
pub mod a2a_discovery;
pub mod a2a_discovery_cache;
pub mod a2a_push;
pub mod a2a_push_worker;
pub mod a2a_ssrf;
pub mod analytics;
pub mod approvals;
pub mod audit;
pub mod auth;
pub mod bus;
pub mod chains;
pub mod circuit_breakers;
pub mod compliance;
pub mod config;
pub mod dispatch;
pub mod dlq;
pub mod embeddings;
pub mod events;
pub mod groups;
pub mod health;
pub mod openapi;
pub mod plugins;
pub mod prometheus;
pub mod provider_health;
pub mod quotas;
pub mod recurring;
pub mod replay;
pub mod retention;
pub mod rules;
pub mod schemas;
pub mod signing_keys;
pub mod silences;
pub mod stream;
pub mod subscribe;
pub mod swarm;
pub mod templates;
pub mod time_intervals;
pub mod trace_context;
pub mod verify;

use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::routing::{delete, get, post, put};
use tokio::sync::RwLock;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use acteon_audit::AnalyticsStore;
use acteon_audit::store::AuditStore;
use acteon_embedding::EmbeddingMetrics;
use acteon_gateway::{Gateway, GatewayMetrics};
use acteon_rules::EmbeddingEvalSupport;

use self::stream::ConnectionRegistry;

use crate::auth::AuthProvider;
use crate::auth::middleware::AuthLayer;
use crate::config::ConfigSnapshot;
use crate::quotas_loader::StaticQuotasHandle;
use crate::ratelimit::RateLimiter;
use crate::ratelimit::middleware::RateLimitLayer;
use crate::templates_loader::StaticTemplatesHandle;

pub use self::verify::SignatureVerifier;

use self::openapi::ApiDoc;

/// Shared application state passed to all handlers.
#[derive(Clone)]
pub struct AppState {
    /// The gateway instance.
    pub gateway: Arc<RwLock<Gateway>>,
    /// Shared handle to the gateway's metric counters. Lets hot-path
    /// handlers bump counters without acquiring the gateway `RwLock`
    /// — the inner counters are `AtomicU64`, so no lock is required.
    pub metrics: Arc<GatewayMetrics>,
    /// Optional audit store (None when audit is disabled).
    pub audit: Option<Arc<dyn AuditStore>>,
    /// Optional analytics store (None when audit is disabled).
    pub analytics: Option<Arc<dyn AnalyticsStore>>,
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
    /// Short-TTL cache in front of `.well-known/agent.json` and the
    /// authenticated extended-card lookup. Caches the resolved
    /// tenant card so a tenant with thousands of registered agents
    /// doesn't pay for a full `scan_keys` on every discovery call.
    pub a2a_discovery_cache: Arc<a2a_discovery_cache::DiscoveryCache>,
    /// Semaphore for limiting concurrent dispatch operations.
    pub dispatch_semaphore: Arc<tokio::sync::Semaphore>,
    /// Sanitized configuration snapshot (secrets masked).
    pub config: ConfigSnapshot,
    /// Path to the static quotas TOML file plus a manual reload
    /// nudger. `None` when no `policies_file` is configured.
    /// `POST /v1/quotas/reload` writes through the nudger so an
    /// optional file watcher coalesces with manual reloads.
    pub static_quotas: Option<StaticQuotasHandle>,
    /// Path to the static templates TOML manifest plus a manual
    /// reload nudger. `None` when no `manifest_file` is configured.
    pub static_templates: Option<StaticTemplatesHandle>,
    /// Path to the Admin UI static files.
    pub ui_path: Option<String>,
    /// Whether the Admin UI is enabled.
    pub ui_enabled: bool,
    /// Allowed CORS origins (empty = permissive).
    pub cors_allowed_origins: Vec<String>,
    /// Optional signature verifier for Ed25519 action signing.
    pub signature_verifier: Option<Arc<SignatureVerifier>>,
    /// Replay protection config: (enabled, `ttl_seconds`).
    pub replay_protection: Option<(bool, u64)>,
    /// Swarm provider registry (None when the `swarm` feature is disabled
    /// or no `swarm`-type provider is configured).
    #[cfg(feature = "swarm")]
    pub swarm_registry: Option<std::sync::Arc<acteon_swarm_provider::SwarmRunRegistry>>,
    /// Agentic bus backend (None when the `bus` feature is disabled or
    /// `[bus].enabled = false`).
    #[cfg(feature = "bus")]
    pub bus_backend: Option<acteon_bus::SharedBackend>,
    /// Compiled-schema registry for publish-edge validation. Always
    /// constructed with the bus feature; stays empty until schemas are
    /// registered.
    #[cfg(feature = "bus")]
    pub bus_schema_validator: acteon_bus::SchemaValidator,
}

/// Build the Axum router with all API routes, middleware, and Swagger UI.
#[allow(clippy::too_many_lines)]
pub fn router(state: AppState) -> Router {
    let public = Router::new()
        // Health & metrics (always public)
        .route("/health", get(health::health))
        .route("/metrics", get(health::metrics))
        .route("/metrics/prometheus", get(prometheus::prometheus_metrics))
        // JWKS-style discovery for action signing keys (public; only
        // exposes public key material, never private keys)
        .route(
            "/.well-known/acteon-signing-keys",
            get(signing_keys::discover_signing_keys),
        )
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
        )
        // A2A Discovery — public, unauthenticated per A2A spec
        .route(
            "/a2a/{namespace}/{tenant}/.well-known/agent.json",
            get(a2a_discovery::discover_agent),
        );

    let protected = Router::new()
        // Dispatch
        .route("/v1/dispatch", post(dispatch::dispatch))
        .route("/v1/dispatch/batch", post(dispatch::dispatch_batch))
        // A2A protocol — JSON-RPC 2.0 + REST binding (Phase 2). The
        // body-reading routes carry an explicit A2A body-size cap; the
        // REST cancel verb shares the `tasks/{id}` path (its `{id}`
        // segment carries the `:cancel` suffix, split in-handler).
        .route(
            "/a2a/{namespace}/{tenant}",
            post(a2a::a2a_rpc).layer(DefaultBodyLimit::max(a2a::A2A_MAX_BODY_BYTES)),
        )
        .route(
            "/a2a/{namespace}/{tenant}/v1/message:send",
            post(a2a::a2a_rest_message_send).layer(DefaultBodyLimit::max(a2a::A2A_MAX_BODY_BYTES)),
        )
        .route(
            "/a2a/{namespace}/{tenant}/v1/tasks/{id}",
            get(a2a::a2a_rest_task_get).post(a2a::a2a_rest_task_cancel),
        )
        // SSE stream of task lifecycle events (Phase 3.2). No body
        // limit needed — GET only — and the per-tenant connection cap
        // is enforced inside the handler.
        .route(
            "/a2a/{namespace}/{tenant}/v1/tasks/{id}/events",
            get(a2a::a2a_task_events),
        )
        // Push-notification config CRUD (Phase 4.1). The collection
        // endpoint registers `POST` (set) + `GET` (list); the item
        // endpoint registers `GET` (get) + `DELETE` (delete).
        .route(
            "/a2a/{namespace}/{tenant}/v1/tasks/{id}/pushNotificationConfigs",
            post(a2a_push::rest_set_push_config).get(a2a_push::rest_list_push_configs),
        )
        .route(
            "/a2a/{namespace}/{tenant}/v1/tasks/{id}/pushNotificationConfigs/{cfgId}",
            get(a2a_push::rest_get_push_config).delete(a2a_push::rest_delete_push_config),
        )
        // A2A push delivery DLQ (operator surface). Not part of the
        // A2A protocol — lives under /v1/a2a/... and uses the
        // standard A2A dispatch grant.
        .route(
            "/v1/a2a/{namespace}/{tenant}/push-dlq",
            get(a2a_push::rest_list_push_dlq),
        )
        .route(
            "/v1/a2a/{namespace}/{tenant}/push-dlq/{entryId}",
            get(a2a_push::rest_get_push_dlq).delete(a2a_push::rest_delete_push_dlq),
        )
        // Rules management
        .route("/v1/rules", get(rules::list_rules))
        .route("/v1/rules/coverage", get(rules::rule_coverage))
        .route("/v1/rules/reload", post(rules::reload_rules))
        .route("/v1/rules/{name}/enabled", put(rules::set_rule_enabled))
        .route("/v1/rules/evaluate", post(rules::evaluate_rules))
        // Analytics
        .route("/v1/analytics", get(analytics::query_analytics))
        // Audit
        .route("/v1/audit", get(audit::query_audit))
        .route("/v1/audit/replay", post(replay::replay_audit))
        .route("/v1/audit/{action_id}", get(audit::get_audit_by_action))
        .route("/v1/audit/{action_id}/replay", post(replay::replay_action))
        // Action signature verification
        .route("/v1/actions/{id}/verify", get(verify::verify_action))
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
        .route("/v1/chains/{chain_id}/dag", get(chains::get_chain_dag))
        .route(
            "/v1/chains/{chain_id}/history",
            get(chains::get_chain_history),
        )
        // Chain definitions CRUD
        .route("/v1/chains/definitions", get(chains::list_definitions))
        .route(
            "/v1/chains/definitions/{name}/dag",
            get(chains::get_chain_definition_dag),
        )
        .route(
            "/v1/chains/definitions/{name}",
            get(chains::get_definition)
                .put(chains::put_definition)
                .delete(chains::delete_definition),
        )
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
        // Quotas
        .route(
            "/v1/quotas",
            get(quotas::list_quotas).post(quotas::create_quota),
        )
        .route("/v1/quotas/reload", post(quotas::reload_static_quotas))
        .route(
            "/v1/quotas/{id}",
            get(quotas::get_quota)
                .put(quotas::update_quota)
                .delete(quotas::delete_quota),
        )
        .route("/v1/quotas/{id}/usage", get(quotas::get_quota_usage))
        // Silences
        .route(
            "/v1/silences",
            get(silences::list_silences).post(silences::create_silence),
        )
        .route(
            "/v1/silences/{id}",
            get(silences::get_silence)
                .put(silences::update_silence)
                .delete(silences::delete_silence),
        )
        // Time intervals
        .route(
            "/v1/time-intervals",
            get(time_intervals::list_time_intervals).post(time_intervals::create_time_interval),
        )
        .route(
            "/v1/time-intervals/{namespace}/{tenant}/{name}",
            get(time_intervals::get_time_interval)
                .put(time_intervals::update_time_interval)
                .delete(time_intervals::delete_time_interval),
        )
        // Retention policies
        .route(
            "/v1/retention",
            get(retention::list_retention).post(retention::create_retention),
        )
        .route(
            "/v1/retention/{id}",
            get(retention::get_retention)
                .put(retention::update_retention)
                .delete(retention::delete_retention),
        )
        // Compliance
        .route(
            "/v1/compliance/status",
            get(compliance::get_compliance_status),
        )
        .route("/v1/audit/verify", post(compliance::verify_audit_chain))
        // Embeddings
        .route("/v1/embeddings/similarity", post(embeddings::similarity))
        // Approvals (list requires auth)
        .route("/v1/approvals", get(approvals::list_approvals))
        // WASM plugins
        .route("/v1/plugins", get(plugins::list_plugins))
        .route("/v1/plugins/{name}", delete(plugins::unregister_plugin))
        // Templates (non-parameterized routes BEFORE parameterized)
        .route(
            "/v1/templates/profiles",
            get(templates::list_profiles).post(templates::create_profile),
        )
        .route(
            "/v1/templates/profiles/{id}",
            get(templates::get_profile)
                .put(templates::update_profile)
                .delete(templates::delete_profile),
        )
        .route("/v1/templates/render", post(templates::render_preview))
        .route(
            "/v1/templates/reload",
            post(templates::reload_static_templates),
        )
        .route(
            "/v1/templates",
            get(templates::list_templates).post(templates::create_template),
        )
        .route(
            "/v1/templates/{id}",
            get(templates::get_template)
                .put(templates::update_template)
                .delete(templates::delete_template),
        )
        // Provider health dashboard
        .route(
            "/v1/providers/health",
            get(provider_health::list_provider_health),
        )
        // Bus (Phase 1 + 2)
        .route(
            "/v1/bus/topics",
            get(bus::list_topics).post(bus::create_topic),
        )
        .route("/v1/bus/topics/{kafka_name}", delete(bus::delete_topic))
        .route("/v1/bus/publish", post(bus::publish))
        .route("/v1/bus/subscribe/{subscription_id}", get(bus::subscribe))
        // Phase 2: durable subscriptions + ack + lag + DLQ. Per-subscription
        // endpoints include the (namespace, tenant) in the path so each
        // operation does an O(1) StateKey lookup instead of scanning.
        .route(
            "/v1/bus/subscriptions",
            get(bus::list_subscriptions).post(bus::create_subscription),
        )
        .route(
            "/v1/bus/subscriptions/{namespace}/{tenant}/{id}",
            delete(bus::delete_subscription),
        )
        .route(
            "/v1/bus/subscriptions/{namespace}/{tenant}/{id}/ack",
            post(bus::ack_subscription),
        )
        .route(
            "/v1/bus/subscriptions/{namespace}/{tenant}/{id}/lag",
            get(bus::subscription_lag),
        )
        .route(
            "/v1/bus/subscriptions/{namespace}/{tenant}/{id}/deadletter",
            post(bus::deadletter_subscription),
        )
        // Phase 3: JSON-Schema registry + topic binding. Tenant-scoped
        // URLs keep state lookups O(1) and make authorization surfaces
        // explicit, matching topics and subscriptions.
        .route(
            "/v1/bus/schemas",
            get(bus::list_schemas).post(bus::create_schema),
        )
        .route(
            "/v1/bus/schemas/{namespace}/{tenant}/{subject}",
            get(bus::get_subject_versions),
        )
        .route(
            "/v1/bus/schemas/{namespace}/{tenant}/{subject}/{version}",
            get(bus::get_schema_version).delete(bus::delete_schema_version),
        )
        .route(
            "/v1/bus/topics/{namespace}/{tenant}/{name}/schema",
            put(bus::bind_topic_schema).delete(bus::unbind_topic_schema),
        )
        // Phase 4: Agent identity + heartbeat + send-to-agent. Shared
        // inbox topic `{ns}.{tenant}.agents-inbox` is auto-created on
        // first registration.
        .route(
            "/v1/bus/agents",
            get(bus::list_agents).post(bus::register_agent),
        )
        .route(
            "/v1/bus/agents/{namespace}/{tenant}/{agent_id}",
            get(bus::get_agent)
                .put(bus::update_agent)
                .delete(bus::delete_agent),
        )
        .route(
            "/v1/bus/agents/{namespace}/{tenant}/{agent_id}/heartbeat",
            post(bus::heartbeat_agent),
        )
        // A2A AgentCard CRUD — populates the discovery surface
        // (`/a2a/{ns}/{tenant}/.well-known/agent.json`).
        .route(
            "/v1/bus/agents/{namespace}/{tenant}/{agent_id}/card",
            put(a2a_discovery::put_agent_card)
                .get(a2a_discovery::get_agent_card)
                .delete(a2a_discovery::delete_agent_card),
        )
        .route(
            "/v1/bus/agents/{namespace}/{tenant}/{agent_id}/send",
            post(bus::send_to_agent),
        )
        // Agent admin lifecycle — operator sets Active / Suspended /
        // Banned. Distinct from the derived liveness `status`; see
        // `AgentAdminState`.
        .route(
            "/v1/bus/agents/{namespace}/{tenant}/{agent_id}/admin-state",
            put(bus::set_agent_admin_state),
        )
        // Phase 5: Conversations — multi-agent threads on a shared
        // events topic. Messages are keyed by conversation_id for
        // per-thread Kafka FIFO; replay filters on the
        // `acteon.conversation.id` header.
        .route(
            "/v1/bus/conversations",
            get(bus::list_conversations).post(bus::register_conversation),
        )
        .route(
            "/v1/bus/conversations/{namespace}/{tenant}/{conversation_id}",
            get(bus::get_conversation)
                .put(bus::update_conversation)
                .delete(bus::delete_conversation),
        )
        .route(
            "/v1/bus/conversations/{namespace}/{tenant}/{conversation_id}/transition",
            post(bus::transition_conversation),
        )
        .route(
            "/v1/bus/conversations/{namespace}/{tenant}/{conversation_id}/messages",
            get(bus::replay_conversation_messages).post(bus::append_conversation_message),
        )
        // Phase 6a: tool-call envelopes (ride on top of conversation
        // events; server stamps `acteon.envelope.kind`,
        // `acteon.tool.call_id`, `acteon.correlation_id`,
        // `acteon.reply_to`).
        .route(
            "/v1/bus/conversations/{namespace}/{tenant}/{conversation_id}/tool-calls",
            post(bus::post_tool_call),
        )
        .route(
            "/v1/bus/conversations/{namespace}/{tenant}/{conversation_id}/tool-results",
            post(bus::post_tool_result),
        )
        .route(
            "/v1/bus/tool-calls/{namespace}/{tenant}/{call_id}/result",
            get(bus::lookup_tool_result),
        )
        // Phase 6b: streaming chunks (server stamps
        // `acteon.envelope.kind ∈ stream_chunk|stream_end`,
        // `acteon.stream.id`, `acteon.stream.seq`).
        .route(
            "/v1/bus/conversations/{namespace}/{tenant}/{conversation_id}/stream-chunks",
            post(bus::post_stream_chunk),
        )
        .route(
            "/v1/bus/conversations/{namespace}/{tenant}/{conversation_id}/stream-end",
            post(bus::post_stream_end),
        )
        .route(
            "/v1/bus/streams/{namespace}/{tenant}/{conversation_id}/{stream_id}",
            get(bus::consume_stream),
        )
        // Phase 6c: pre-publish HITL approvals for tool-calls.
        .route(
            "/v1/bus/approvals/{namespace}/{tenant}",
            get(bus::list_bus_approvals),
        )
        .route(
            "/v1/bus/approvals/{namespace}/{tenant}/{approval_id}",
            get(bus::get_bus_approval),
        )
        .route(
            "/v1/bus/approvals/{namespace}/{tenant}/{approval_id}/approve",
            post(bus::approve_bus_approval),
        )
        .route(
            "/v1/bus/approvals/{namespace}/{tenant}/{approval_id}/reject",
            post(bus::reject_bus_approval),
        )
        // Swarm runs
        .route("/v1/swarm/runs", get(swarm::list_swarm_runs))
        .route("/v1/swarm/runs/{run_id}", get(swarm::get_swarm_run))
        .route(
            "/v1/swarm/runs/{run_id}/cancel",
            post(swarm::cancel_swarm_run),
        )
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

    let cors = if state.cors_allowed_origins.is_empty() {
        CorsLayer::permissive()
    } else {
        let origins: Vec<axum::http::HeaderValue> = state
            .cors_allowed_origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods(tower_http::cors::Any)
            .allow_headers(tower_http::cors::Any)
    };

    router
        .with_state(state)
        // W3C Trace Context propagation (extracts traceparent/tracestate from
        // incoming requests so OTel can link server spans to the caller's trace).
        .layer(middleware::from_fn(trace_context::propagate_trace_context))
        .layer(TraceLayer::new_for_http())
        .layer(cors)
}
