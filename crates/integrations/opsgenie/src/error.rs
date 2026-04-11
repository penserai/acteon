use acteon_provider::ProviderError;
use thiserror::Error;

/// Errors specific to the `OpsGenie` provider.
///
/// These are internal errors that get converted into [`ProviderError`] at the
/// public API boundary.
#[derive(Debug, Error)]
pub enum OpsGenieError {
    /// An HTTP-level transport error occurred.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// The `OpsGenie` API returned a **permanent** non-success response
    /// (a 4xx that is not a rate-limit or auth failure). These are
    /// surfaced as `ExecutionFailed` and are **not** retried — retrying
    /// a malformed request will never succeed.
    #[error("OpsGenie API error: {0}")]
    Api(String),

    /// The `OpsGenie` API returned a **transient** non-success response
    /// (5xx server error or 408 Request Timeout). The request body was
    /// fine; the server was temporarily unable to handle it.
    ///
    /// Surfaced as `ProviderError::Connection` so the gateway's retry
    /// logic re-queues the dispatch instead of dropping the alert on
    /// the floor during a brief `OpsGenie` outage.
    #[error("OpsGenie transient error: {0}")]
    Transient(String),

    /// The action payload is missing required fields or has invalid structure.
    #[error("invalid payload: {0}")]
    InvalidPayload(String),

    /// The provider received an HTTP 429 (Too Many Requests) response.
    #[error("rate limited by OpsGenie")]
    RateLimited,

    /// The provider received an HTTP 401/403 response — typically a bad or
    /// revoked API key.
    #[error("authentication failed: {0}")]
    Unauthorized(String),
}

impl From<OpsGenieError> for ProviderError {
    fn from(err: OpsGenieError) -> Self {
        match err {
            OpsGenieError::Http(e) => ProviderError::Connection(e.to_string()),
            OpsGenieError::Api(msg) => ProviderError::ExecutionFailed(msg),
            OpsGenieError::Transient(msg) => ProviderError::Connection(msg),
            OpsGenieError::InvalidPayload(msg) => ProviderError::Serialization(msg),
            OpsGenieError::RateLimited => ProviderError::RateLimited,
            OpsGenieError::Unauthorized(msg) => ProviderError::Configuration(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limited_maps_to_retryable() {
        let provider_err: ProviderError = OpsGenieError::RateLimited.into();
        assert!(provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::RateLimited));
    }

    #[test]
    fn api_error_maps_to_non_retryable() {
        let provider_err: ProviderError = OpsGenieError::Api("bad request".into()).into();
        assert!(!provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::ExecutionFailed(_)));
    }

    #[test]
    fn transient_error_maps_to_retryable_connection() {
        // 5xx / 408 live-blip errors must be retried instead of
        // dropping the alert on the floor.
        let provider_err: ProviderError =
            OpsGenieError::Transient("HTTP 503: service unavailable".into()).into();
        assert!(
            provider_err.is_retryable(),
            "transient errors must be retryable"
        );
        assert!(matches!(provider_err, ProviderError::Connection(_)));
    }

    #[test]
    fn invalid_payload_maps_to_serialization() {
        let provider_err: ProviderError =
            OpsGenieError::InvalidPayload("missing message".into()).into();
        assert!(!provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::Serialization(_)));
    }

    #[test]
    fn unauthorized_maps_to_configuration() {
        let provider_err: ProviderError =
            OpsGenieError::Unauthorized("invalid API key".into()).into();
        assert!(!provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::Configuration(_)));
    }

    #[test]
    fn display_messages() {
        let err = OpsGenieError::Api("invalid request".into());
        assert_eq!(err.to_string(), "OpsGenie API error: invalid request");

        let err = OpsGenieError::InvalidPayload("missing alias".into());
        assert_eq!(err.to_string(), "invalid payload: missing alias");

        let err = OpsGenieError::RateLimited;
        assert_eq!(err.to_string(), "rate limited by OpsGenie");

        let err = OpsGenieError::Unauthorized("token revoked".into());
        assert_eq!(err.to_string(), "authentication failed: token revoked");
    }
}
