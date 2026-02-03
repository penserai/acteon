use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use thiserror::Error;

/// Errors that can occur when running the Acteon server.
#[derive(Debug, Error)]
pub enum ServerError {
    /// A configuration error.
    #[error("configuration error: {0}")]
    Config(String),

    /// An I/O error (e.g. binding the listener).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// A gateway-level error surfaced through the API.
    #[error("gateway error: {0}")]
    Gateway(#[from] acteon_gateway::GatewayError),

    /// Authentication failed (missing or invalid credentials).
    #[error("unauthorized: {0}")]
    Unauthorized(String),

    /// Caller lacks permission for the requested operation.
    #[error("forbidden: {0}")]
    Forbidden(String),

    /// Rate limit exceeded.
    #[error("rate limit exceeded")]
    RateLimited {
        /// Seconds until the caller can retry.
        retry_after: u64,
    },
}

impl IntoResponse for ServerError {
    fn into_response(self) -> Response {
        let (status, message, retry_after) = match &self {
            Self::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg.clone(), None),
            Self::Forbidden(msg) => (StatusCode::FORBIDDEN, msg.clone(), None),
            Self::Config(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone(), None),
            Self::Io(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
            Self::Gateway(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
            Self::RateLimited { retry_after } => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate limit exceeded".to_owned(),
                Some(*retry_after),
            ),
        };

        let body = if let Some(retry) = retry_after {
            serde_json::json!({ "error": message, "retry_after": retry })
        } else {
            serde_json::json!({ "error": message })
        };

        let mut response = (status, axum::Json(body)).into_response();

        if let Some(retry) = retry_after {
            response
                .headers_mut()
                .insert(axum::http::header::RETRY_AFTER, retry.into());
        }

        response
    }
}
