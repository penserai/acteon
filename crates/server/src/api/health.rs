use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;

use super::AppState;
use super::schemas::{EmbeddingMetricsResponse, HealthResponse, MetricsResponse};

fn build_metrics_response(
    state: &AppState,
    snap: &acteon_gateway::MetricsSnapshot,
) -> MetricsResponse {
    let embedding = state.embedding_metrics.as_ref().map(|m| {
        let s = m.snapshot();
        EmbeddingMetricsResponse {
            topic_cache_hits: s.topic_cache_hits,
            topic_cache_misses: s.topic_cache_misses,
            text_cache_hits: s.text_cache_hits,
            text_cache_misses: s.text_cache_misses,
            errors: s.errors,
            fail_open_count: s.fail_open_count,
        }
    });

    MetricsResponse {
        dispatched: snap.dispatched,
        executed: snap.executed,
        deduplicated: snap.deduplicated,
        suppressed: snap.suppressed,
        rerouted: snap.rerouted,
        throttled: snap.throttled,
        failed: snap.failed,
        llm_guardrail_allowed: snap.llm_guardrail_allowed,
        llm_guardrail_denied: snap.llm_guardrail_denied,
        llm_guardrail_errors: snap.llm_guardrail_errors,
        chains_started: snap.chains_started,
        chains_completed: snap.chains_completed,
        chains_failed: snap.chains_failed,
        chains_cancelled: snap.chains_cancelled,
        pending_approval: snap.pending_approval,
        circuit_open: snap.circuit_open,
        circuit_transitions: snap.circuit_transitions,
        circuit_fallbacks: snap.circuit_fallbacks,
        scheduled: snap.scheduled,
        recurring_dispatched: snap.recurring_dispatched,
        recurring_errors: snap.recurring_errors,
        recurring_skipped: snap.recurring_skipped,
        quota_exceeded: snap.quota_exceeded,
        quota_warned: snap.quota_warned,
        quota_degraded: snap.quota_degraded,
        quota_notified: snap.quota_notified,
        retention_deleted_state: snap.retention_deleted_state,
        retention_skipped_compliance: snap.retention_skipped_compliance,
        retention_errors: snap.retention_errors,
        wasm_invocations: snap.wasm_invocations,
        wasm_errors: snap.wasm_errors,
        embedding,
    }
}

/// `GET /health` -- returns service status together with a metrics snapshot.
#[utoipa::path(
    get,
    path = "/health",
    tag = "Health",
    summary = "Health check",
    description = "Returns service status and a snapshot of gateway dispatch metrics.",
    responses(
        (status = 200, description = "Service is healthy", body = HealthResponse)
    )
)]
pub async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let snap = gw.metrics().snapshot();

    let body = HealthResponse {
        status: "ok".into(),
        metrics: build_metrics_response(&state, &snap),
    };

    (StatusCode::OK, Json(body))
}

/// `GET /metrics` -- returns gateway metrics as JSON.
#[utoipa::path(
    get,
    path = "/metrics",
    tag = "Health",
    summary = "Gateway metrics",
    description = "Returns current dispatch counters for monitoring.",
    responses(
        (status = 200, description = "Current metric counters", body = MetricsResponse)
    )
)]
pub async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let snap = gw.metrics().snapshot();
    let body = build_metrics_response(&state, &snap);
    (StatusCode::OK, Json(body))
}
