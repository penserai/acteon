use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use tokio::sync::RwLock;

use acteon_core::{Action, ActionOutcome};
use acteon_gateway::Gateway;

use super::schemas::ErrorResponse;

/// `POST /v1/dispatch` -- dispatch a single action through the gateway pipeline.
///
/// Expects a JSON body that deserializes to an [`Action`]. Returns the
/// resulting [`ActionOutcome`](acteon_core::ActionOutcome) as JSON.
#[utoipa::path(
    post,
    path = "/v1/dispatch",
    tag = "Dispatch",
    summary = "Dispatch action",
    description = "Sends a single action through the gateway pipeline (lock, rules, execute) and returns the outcome.",
    request_body(content = Action, description = "Action to dispatch"),
    responses(
        (status = 200, description = "Action dispatched successfully", body = ActionOutcome),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub async fn dispatch(
    State(gateway): State<Arc<RwLock<Gateway>>>,
    Json(action): Json<Action>,
) -> impl IntoResponse {
    let gw = gateway.read().await;
    match gw.dispatch(action).await {
        Ok(outcome) => (StatusCode::OK, Json(serde_json::json!(outcome))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse {
                error: e.to_string()
            })),
        ),
    }
}

/// `POST /v1/dispatch/batch` -- dispatch multiple actions and collect results.
///
/// Expects a JSON array of [`Action`] objects. Returns an array of results,
/// where each element is either an `ActionOutcome` or an error object.
#[utoipa::path(
    post,
    path = "/v1/dispatch/batch",
    tag = "Dispatch",
    summary = "Batch dispatch",
    description = "Dispatches multiple actions through the gateway pipeline and returns an array of outcomes or errors.",
    request_body(content = Vec<Action>, description = "Actions to dispatch"),
    responses(
        (status = 200, description = "Array of dispatch outcomes", body = Vec<serde_json::Value>)
    )
)]
pub async fn dispatch_batch(
    State(gateway): State<Arc<RwLock<Gateway>>>,
    Json(actions): Json<Vec<Action>>,
) -> impl IntoResponse {
    let gw = gateway.read().await;
    let results = gw.dispatch_batch(actions).await;

    let body: Vec<serde_json::Value> = results
        .into_iter()
        .map(|r| match r {
            Ok(outcome) => serde_json::json!(outcome),
            Err(e) => serde_json::json!(ErrorResponse {
                error: e.to_string()
            }),
        })
        .collect();

    (StatusCode::OK, Json(body))
}
