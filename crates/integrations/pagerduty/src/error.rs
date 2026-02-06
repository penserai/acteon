use acteon_provider::ProviderError;
use thiserror::Error;

/// Errors specific to the `PagerDuty` provider.
///
/// These are internal errors that get converted into [`ProviderError`] at the
/// public API boundary.
#[derive(Debug, Error)]
pub enum PagerDutyError {
    /// An HTTP-level transport error occurred.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// The `PagerDuty` API returned a non-success response.
    #[error("PagerDuty API error: {0}")]
    Api(String),

    /// The action payload is missing required fields or has invalid structure.
    #[error("invalid payload: {0}")]
    InvalidPayload(String),

    /// The provider received an HTTP 429 (Too Many Requests) response.
    #[error("rate limited by PagerDuty")]
    RateLimited,
}

impl From<PagerDutyError> for ProviderError {
    fn from(err: PagerDutyError) -> Self {
        match err {
            PagerDutyError::Http(e) => ProviderError::Connection(e.to_string()),
            PagerDutyError::Api(msg) => ProviderError::ExecutionFailed(msg),
            PagerDutyError::InvalidPayload(msg) => ProviderError::Serialization(msg),
            PagerDutyError::RateLimited => ProviderError::RateLimited,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limited_maps_to_retryable() {
        let provider_err: ProviderError = PagerDutyError::RateLimited.into();
        assert!(provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::RateLimited));
    }

    #[test]
    fn api_error_maps_to_non_retryable() {
        let provider_err: ProviderError = PagerDutyError::Api("bad request".into()).into();
        assert!(!provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::ExecutionFailed(_)));
    }

    #[test]
    fn invalid_payload_maps_to_serialization() {
        let provider_err: ProviderError =
            PagerDutyError::InvalidPayload("missing summary".into()).into();
        assert!(!provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::Serialization(_)));
    }

    #[test]
    fn display_messages() {
        let err = PagerDutyError::Api("invalid routing key".into());
        assert_eq!(err.to_string(), "PagerDuty API error: invalid routing key");

        let err = PagerDutyError::InvalidPayload("missing dedup_key".into());
        assert_eq!(err.to_string(), "invalid payload: missing dedup_key");

        let err = PagerDutyError::RateLimited;
        assert_eq!(err.to_string(), "rate limited by PagerDuty");
    }
}
