use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::identity::CallerIdentity;
use crate::auth::role::Permission;
use crate::error::ServerError;
use crate::ratelimit::config::RateLimitTier;

use super::AppState;
use super::schemas::ErrorResponse;

/// Request body for the similarity endpoint.
#[derive(Deserialize, ToSchema)]
pub struct SimilarityRequest {
    /// The text to compare.
    pub text: String,
    /// The topic to compare against.
    pub topic: String,
}

/// Response body for the similarity endpoint.
#[derive(Serialize, ToSchema)]
pub struct SimilarityResponse {
    /// Cosine similarity score in `[0.0, 1.0]`.
    pub similarity: f64,
    /// The topic that was compared.
    pub topic: String,
}

/// Tight rate limit tier for embedding similarity requests: 5 per minute.
const EMBEDDING_RATE_LIMIT: RateLimitTier = RateLimitTier {
    requests_per_window: 5,
    window_seconds: 60,
};

/// `POST /v1/embeddings/similarity` -- compute cosine similarity between text and a topic.
///
/// Returns the similarity score from the configured embedding provider.
/// Rate-limited to 5 requests per minute per caller.
#[utoipa::path(
    post,
    path = "/v1/embeddings/similarity",
    tag = "Embeddings",
    summary = "Compute embedding similarity",
    description = "Computes cosine similarity between the given text and topic using the configured embedding provider. Rate-limited to 5 requests/minute per caller.",
    request_body(content = SimilarityRequest, description = "Text and topic to compare"),
    responses(
        (status = 200, description = "Similarity computed", body = SimilarityResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse),
        (status = 404, description = "Embedding provider not configured", body = ErrorResponse),
        (status = 429, description = "Rate limit exceeded", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub async fn similarity(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Json(req): Json<SimilarityRequest>,
) -> Result<impl IntoResponse, ServerError> {
    // Check role permission (reuse Dispatch permission).
    if !identity.role.has_permission(Permission::Dispatch) {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!(ErrorResponse {
                error:
                    "insufficient permissions: embedding similarity requires admin or operator role"
                        .into(),
            })),
        ));
    }

    // Apply tight per-caller rate limit for embedding requests.
    if let Some(ref limiter) = state.rate_limiter {
        let bucket = format!("embedding:{}", identity.id);
        if let Err(e) = limiter
            .check_custom_limit(&bucket, &EMBEDDING_RATE_LIMIT)
            .await
        {
            return Err(ServerError::RateLimited {
                retry_after: e.retry_after,
            });
        }
    }

    // Check that embedding support is configured.
    let Some(ref embedding) = state.embedding else {
        return Ok((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                error: "embedding provider is not configured".into(),
            })),
        ));
    };

    // Compute similarity.
    match embedding.similarity(&req.text, &req.topic).await {
        Ok(score) => Ok((
            StatusCode::OK,
            Json(serde_json::json!(SimilarityResponse {
                similarity: score,
                topic: req.topic,
            })),
        )),
        Err(e) => Ok((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse {
                error: format!("embedding similarity failed: {e}"),
            })),
        )),
    }
}
