use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use tokio::sync::RwLock;

use acteon_gateway::Gateway;

use super::schemas::{HealthResponse, MetricsResponse};

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
pub async fn health(State(gateway): State<Arc<RwLock<Gateway>>>) -> impl IntoResponse {
    let gw = gateway.read().await;
    let snap = gw.metrics().snapshot();

    let body = HealthResponse {
        status: "ok".into(),
        metrics: MetricsResponse {
            dispatched: snap.dispatched,
            executed: snap.executed,
            deduplicated: snap.deduplicated,
            suppressed: snap.suppressed,
            rerouted: snap.rerouted,
            throttled: snap.throttled,
            failed: snap.failed,
        },
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
pub async fn metrics(State(gateway): State<Arc<RwLock<Gateway>>>) -> impl IntoResponse {
    let gw = gateway.read().await;
    let snap = gw.metrics().snapshot();

    let body = MetricsResponse {
        dispatched: snap.dispatched,
        executed: snap.executed,
        deduplicated: snap.deduplicated,
        suppressed: snap.suppressed,
        rerouted: snap.rerouted,
        throttled: snap.throttled,
        failed: snap.failed,
    };

    (StatusCode::OK, Json(body))
}
