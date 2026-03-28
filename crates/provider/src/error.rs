use std::time::Duration;

use thiserror::Error;

/// Maximum number of bytes from an upstream HTTP response body to include in
/// error messages. Response bodies can contain internal hostnames, stack
/// traces, or partial credentials -- truncating prevents information leakage
/// through the dispatch API and audit trail.
const MAX_ERROR_BODY_BYTES: usize = 256;

/// Truncate an HTTP response body for safe inclusion in error messages.
///
/// Returns at most [`MAX_ERROR_BODY_BYTES`] bytes of the input, appending
/// `"...[truncated]"` when the input exceeds the limit. Truncation is
/// performed at a UTF-8 character boundary to avoid producing invalid strings.
pub fn truncate_error_body(body: &str) -> String {
    if body.len() <= MAX_ERROR_BODY_BYTES {
        return body.to_owned();
    }
    // Find a valid char boundary at or before the limit.
    let mut end = MAX_ERROR_BODY_BYTES;
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...[truncated]", &body[..end])
}

/// Errors that can occur during provider operations.
#[derive(Debug, Error)]
pub enum ProviderError {
    /// The requested provider was not found in the registry.
    #[error("provider not found: {0}")]
    NotFound(String),

    /// The provider failed to execute the action.
    #[error("execution failed: {0}")]
    ExecutionFailed(String),

    /// The provider did not respond within the allowed duration.
    #[error("timeout after {0:?}")]
    Timeout(Duration),

    /// A network or transport-level error occurred.
    #[error("connection error: {0}")]
    Connection(String),

    /// The provider was given invalid configuration.
    #[error("invalid configuration: {0}")]
    Configuration(String),

    /// The provider rejected the request due to rate limiting.
    #[error("rate limited")]
    RateLimited,

    /// A serialization or deserialization error occurred.
    #[error("serialization error: {0}")]
    Serialization(String),
}

impl ProviderError {
    /// Returns `true` if the error is transient and the operation may succeed
    /// on retry.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Timeout(_) | Self::Connection(_) | Self::RateLimited
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_errors() {
        assert!(ProviderError::Timeout(Duration::from_secs(5)).is_retryable());
        assert!(ProviderError::Connection("reset".into()).is_retryable());
        assert!(ProviderError::RateLimited.is_retryable());
    }

    #[test]
    fn non_retryable_errors() {
        assert!(!ProviderError::NotFound("x".into()).is_retryable());
        assert!(!ProviderError::ExecutionFailed("x".into()).is_retryable());
        assert!(!ProviderError::Configuration("x".into()).is_retryable());
        assert!(!ProviderError::Serialization("x".into()).is_retryable());
    }

    #[test]
    fn error_display() {
        let err = ProviderError::NotFound("email".into());
        assert_eq!(err.to_string(), "provider not found: email");

        let err = ProviderError::Timeout(Duration::from_millis(500));
        assert_eq!(err.to_string(), "timeout after 500ms");

        let err = ProviderError::RateLimited;
        assert_eq!(err.to_string(), "rate limited");
    }

    #[test]
    fn truncate_error_body_short_passes_through() {
        let short = "bad request";
        assert_eq!(truncate_error_body(short), short);
    }

    #[test]
    fn truncate_error_body_at_limit_passes_through() {
        let exact = "x".repeat(MAX_ERROR_BODY_BYTES);
        assert_eq!(truncate_error_body(&exact), exact);
    }

    #[test]
    fn truncate_error_body_long_is_truncated() {
        let long = "a".repeat(512);
        let result = truncate_error_body(&long);
        assert!(result.len() < long.len());
        assert!(result.ends_with("...[truncated]"));
        // Verify the prefix is correct.
        assert!(result.starts_with(&"a".repeat(MAX_ERROR_BODY_BYTES)));
    }

    #[test]
    fn truncate_error_body_respects_utf8_boundary() {
        // Multi-byte UTF-8: each char is 4 bytes. Place boundary mid-character.
        let emoji = "😀".repeat(100); // 400 bytes
        let result = truncate_error_body(&emoji);
        assert!(result.ends_with("...[truncated]"));
        // Must be valid UTF-8 (this would panic if not).
        let _ = result.as_bytes();
    }
}
