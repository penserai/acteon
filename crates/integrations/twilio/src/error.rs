use acteon_provider::ProviderError;
use thiserror::Error;

/// Errors specific to the Twilio provider.
///
/// These are internal errors that get converted into [`ProviderError`] at the
/// public API boundary.
#[derive(Debug, Error)]
pub enum TwilioError {
    /// An HTTP-level transport error occurred.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// The Twilio API returned an error response.
    #[error("Twilio API error: {0}")]
    Api(String),

    /// The action payload is missing required fields or has invalid structure.
    #[error("invalid payload: {0}")]
    InvalidPayload(String),

    /// The provider received an HTTP 429 (Too Many Requests) response.
    #[error("rate limited by Twilio")]
    RateLimited,
}

impl From<TwilioError> for ProviderError {
    fn from(err: TwilioError) -> Self {
        match err {
            TwilioError::Http(e) => ProviderError::Connection(e.to_string()),
            TwilioError::Api(msg) => ProviderError::ExecutionFailed(msg),
            TwilioError::InvalidPayload(msg) => ProviderError::Serialization(msg),
            TwilioError::RateLimited => ProviderError::RateLimited,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limited_maps_to_retryable() {
        let provider_err: ProviderError = TwilioError::RateLimited.into();
        assert!(provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::RateLimited));
    }

    #[test]
    fn api_error_maps_to_non_retryable() {
        let provider_err: ProviderError = TwilioError::Api("invalid_auth".into()).into();
        assert!(!provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::ExecutionFailed(_)));
    }

    #[test]
    fn invalid_payload_maps_to_serialization() {
        let provider_err: ProviderError =
            TwilioError::InvalidPayload("missing 'to' field".into()).into();
        assert!(!provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::Serialization(_)));
    }

    #[test]
    fn error_display() {
        let err = TwilioError::Api("21211".into());
        assert_eq!(err.to_string(), "Twilio API error: 21211");
    }
}
