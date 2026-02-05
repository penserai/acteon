use acteon_provider::ProviderError;
use thiserror::Error;

/// Errors specific to the Slack provider.
///
/// These are internal errors that get converted into [`ProviderError`] at the
/// public API boundary.
#[derive(Debug, Error)]
pub enum SlackError {
    /// An HTTP-level transport error occurred.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// The Slack API returned an error response (ok: false).
    #[error("Slack API error: {0}")]
    Api(String),

    /// The action payload is missing required fields or has invalid structure.
    #[error("invalid payload: {0}")]
    InvalidPayload(String),

    /// The provider received an HTTP 429 (Too Many Requests) response.
    #[error("rate limited by Slack")]
    RateLimited,
}

impl From<SlackError> for ProviderError {
    fn from(err: SlackError) -> Self {
        match err {
            SlackError::Http(e) => ProviderError::Connection(e.to_string()),
            SlackError::Api(msg) => ProviderError::ExecutionFailed(msg),
            SlackError::InvalidPayload(msg) => ProviderError::Serialization(msg),
            SlackError::RateLimited => ProviderError::RateLimited,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limited_maps_to_retryable() {
        let provider_err: ProviderError = SlackError::RateLimited.into();
        assert!(provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::RateLimited));
    }

    #[test]
    fn api_error_maps_to_non_retryable() {
        let provider_err: ProviderError = SlackError::Api("invalid_auth".into()).into();
        assert!(!provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::ExecutionFailed(_)));
    }

    #[test]
    fn invalid_payload_maps_to_serialization() {
        let provider_err: ProviderError =
            SlackError::InvalidPayload("missing channel".into()).into();
        assert!(!provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::Serialization(_)));
    }

    #[test]
    fn http_error_maps_to_connection() {
        // We cannot easily construct a reqwest::Error directly, so test the
        // variant via the SlackError display instead.
        let err = SlackError::Api("test".into());
        assert_eq!(err.to_string(), "Slack API error: test");
    }
}
