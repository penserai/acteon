use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use tokio::sync::RwLock;

use acteon_gateway::Gateway;

/// `GET /health` -- returns `{"status": "ok"}` together with a metrics snapshot.
pub async fn health(State(gateway): State<Arc<RwLock<Gateway>>>) -> impl IntoResponse {
    let gw = gateway.read().await;
    let snap = gw.metrics().snapshot();

    let body = serde_json::json!({
        "status": "ok",
        "metrics": {
            "dispatched": snap.dispatched,
            "executed": snap.executed,
            "deduplicated": snap.deduplicated,
            "suppressed": snap.suppressed,
            "rerouted": snap.rerouted,
            "throttled": snap.throttled,
            "failed": snap.failed,
        }
    });

    (StatusCode::OK, Json(body))
}

/// `GET /metrics` -- returns gateway metrics as JSON.
pub async fn metrics(State(gateway): State<Arc<RwLock<Gateway>>>) -> impl IntoResponse {
    let gw = gateway.read().await;
    let snap = gw.metrics().snapshot();

    let body = serde_json::json!({
        "dispatched": snap.dispatched,
        "executed": snap.executed,
        "deduplicated": snap.deduplicated,
        "suppressed": snap.suppressed,
        "rerouted": snap.rerouted,
        "throttled": snap.throttled,
        "failed": snap.failed,
    });

    (StatusCode::OK, Json(body))
}
