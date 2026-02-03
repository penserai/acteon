//! Dead-letter queue API endpoints.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::AppState;
use super::schemas::ErrorResponse;

/// Response for DLQ stats endpoint.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DlqStatsResponse {
    /// Whether the DLQ is enabled.
    #[schema(example = true)]
    pub enabled: bool,
    /// Number of entries in the DLQ.
    #[schema(example = 5)]
    pub count: usize,
}

/// A single dead-letter queue entry.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DlqEntry {
    /// The failed action's unique identifier.
    #[schema(example = "act_12345")]
    pub action_id: String,
    /// Namespace the action belongs to.
    #[schema(example = "notifications")]
    pub namespace: String,
    /// Tenant that owns the action.
    #[schema(example = "tenant-1")]
    pub tenant: String,
    /// Target provider for the action.
    #[schema(example = "email")]
    pub provider: String,
    /// Action type discriminator.
    #[schema(example = "send_email")]
    pub action_type: String,
    /// Human-readable description of the final error.
    #[schema(example = "connection timeout after 3 attempts")]
    pub error: String,
    /// Number of execution attempts made.
    #[schema(example = 3)]
    pub attempts: u32,
    /// Unix timestamp (seconds) when the entry was created.
    #[schema(example = 1_706_800_000)]
    pub timestamp: u64,
}

/// Response for DLQ drain endpoint.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DlqDrainResponse {
    /// Entries drained from the DLQ.
    pub entries: Vec<DlqEntry>,
    /// Number of entries drained.
    #[schema(example = 5)]
    pub count: usize,
}

/// `GET /v1/dlq/stats` -- get dead-letter queue statistics.
#[utoipa::path(
    get,
    path = "/v1/dlq/stats",
    tag = "DLQ",
    summary = "Get DLQ statistics",
    description = "Returns the current count of entries in the dead-letter queue.",
    responses(
        (status = 200, description = "DLQ statistics", body = DlqStatsResponse)
    )
)]
pub async fn dlq_stats(State(state): State<AppState>) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let enabled = gw.dlq_enabled();
    let count = gw.dlq_len().await.unwrap_or(0);
    (StatusCode::OK, Json(DlqStatsResponse { enabled, count }))
}

/// `POST /v1/dlq/drain` -- drain all entries from the dead-letter queue.
#[utoipa::path(
    post,
    path = "/v1/dlq/drain",
    tag = "DLQ",
    summary = "Drain DLQ entries",
    description = "Removes and returns all entries from the dead-letter queue. Use this to retrieve failed actions for manual processing or resubmission.",
    responses(
        (status = 200, description = "Drained entries", body = DlqDrainResponse),
        (status = 404, description = "DLQ not enabled", body = ErrorResponse)
    )
)]
pub async fn dlq_drain(State(state): State<AppState>) -> impl IntoResponse {
    let gw = state.gateway.read().await;

    if !gw.dlq_enabled() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                error: "dead-letter queue is not enabled".into(),
            })),
        );
    }

    let entries: Vec<DlqEntry> = gw
        .dlq_drain()
        .await
        .into_iter()
        .map(|e| DlqEntry {
            action_id: e.action.id.to_string(),
            namespace: e.action.namespace.to_string(),
            tenant: e.action.tenant.to_string(),
            provider: e.action.provider.to_string(),
            action_type: e.action.action_type.clone(),
            error: e.error,
            attempts: e.attempts,
            timestamp: e
                .timestamp
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        })
        .collect();

    let count = entries.len();
    (
        StatusCode::OK,
        Json(serde_json::json!(DlqDrainResponse { entries, count })),
    )
}
