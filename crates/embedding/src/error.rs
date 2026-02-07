use thiserror::Error;

/// Errors that can occur during embedding operations.
#[derive(Debug, Error)]
pub enum EmbeddingError {
    /// An HTTP request failed.
    #[error("HTTP error: {0}")]
    HttpError(String),

    /// The request timed out.
    #[error("embedding request timed out")]
    Timeout,

    /// Failed to parse the API response.
    #[error("parse error: {0}")]
    ParseError(String),

    /// The API returned an error.
    #[error("API error: {0}")]
    ApiError(String),
}
