use std::time::Duration;

use thiserror::Error;

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
}
