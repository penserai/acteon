use thiserror::Error;

/// Errors that can occur during LLM evaluation.
#[derive(Debug, Error)]
pub enum LlmEvaluatorError {
    /// HTTP request failed.
    #[error("HTTP error: {0}")]
    HttpError(String),

    /// Request timed out.
    #[error("LLM request timed out after {0}s")]
    Timeout(u64),

    /// Failed to parse LLM response.
    #[error("failed to parse LLM response: {0}")]
    ParseError(String),

    /// LLM API returned an error response.
    #[error("LLM API error: {0}")]
    ApiError(String),

    /// Configuration error.
    #[error("configuration error: {0}")]
    Configuration(String),
}
