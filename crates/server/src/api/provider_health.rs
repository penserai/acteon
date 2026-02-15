use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;

use acteon_core::{ListProviderHealthResponse, ProviderHealthStatus};

use super::AppState;

/// `GET /v1/providers/health` -- per-provider health and performance dashboard.
#[utoipa::path(
    get,
    path = "/v1/providers/health",
    tag = "Provider Health",
    summary = "Provider health dashboard",
    description = "Returns per-provider health status, circuit breaker state, execution metrics, and latency percentiles.",
    responses(
        (status = 200, description = "Provider health data", body = ListProviderHealthResponse)
    )
)]
pub async fn list_provider_health(
    State(state): State<AppState>,
    axum::Extension(_identity): axum::Extension<crate::auth::identity::CallerIdentity>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;

    // Run health checks on all registered providers.
    let health_results = gw.check_provider_health().await;

    // Take a snapshot of per-provider execution metrics.
    let metrics_map = gw.provider_metrics().snapshot();

    let mut providers = Vec::with_capacity(health_results.len());

    for result in &health_results {
        let name = &result.provider;

        // Get circuit breaker state if configured.
        let circuit_breaker_state = if let Some(registry) = gw.circuit_breakers() {
            if let Some(cb) = registry.get(name) {
                Some(cb.state().await.to_string())
            } else {
                None
            }
        } else {
            None
        };

        // Merge with execution metrics (if any requests have been made).
        let pm = metrics_map.get(name.as_str());

        providers.push(ProviderHealthStatus {
            provider: name.clone(),
            healthy: result.healthy,
            health_check_error: result.error.clone(),
            circuit_breaker_state,
            total_requests: pm.map_or(0, |s| s.total_requests),
            successes: pm.map_or(0, |s| s.successes),
            failures: pm.map_or(0, |s| s.failures),
            success_rate: pm.map_or(0.0, |s| s.success_rate),
            avg_latency_ms: pm.map_or(0.0, |s| s.avg_latency_ms),
            p50_latency_ms: pm.map_or(0.0, |s| s.p50_latency_ms),
            p95_latency_ms: pm.map_or(0.0, |s| s.p95_latency_ms),
            p99_latency_ms: pm.map_or(0.0, |s| s.p99_latency_ms),
            last_request_at: pm.and_then(|s| s.last_request_at),
            last_error: pm.and_then(|s| s.last_error.clone()),
        });
    }

    (
        StatusCode::OK,
        Json(ListProviderHealthResponse { providers }),
    )
}
