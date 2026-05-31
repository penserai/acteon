use acteon_provider::ProviderError;
use thiserror::Error;

/// Errors specific to the Microsoft Teams provider.
///
/// These are internal errors that get converted into [`ProviderError`] at the
/// public API boundary.
#[derive(Debug, Error)]
pub enum TeamsError {
    /// An HTTP-level transport error occurred.
    #[error("HTTP error: {0}")]
    Http(reqwest::Error),

    /// The Teams webhook returned an error response.
    #[error("Teams webhook error: {0}")]
    Api(String),

    /// The action payload is missing required fields or has invalid structure.
    #[error("invalid payload: {0}")]
    InvalidPayload(String),

    /// The provider received an HTTP 429 (Too Many Requests) response.
    #[error("rate limited by Teams")]
    RateLimited,

    /// 5xx server error or 408 Request Timeout — the request was fine, the
    /// server was temporarily unable to handle it. Surfaced as
    /// `ProviderError::Connection` so the gateway retries instead of dropping.
    #[error("Teams transient error: {0}")]
    Transient(String),
}

impl From<reqwest::Error> for TeamsError {
    fn from(err: reqwest::Error) -> Self {
        // Redact the request URL: for several providers it carries the bot
        // token, webhook secret, or access token, which must never reach
        // error messages, audit records, or the DLQ.
        Self::Http(err.without_url())
    }
}

impl From<TeamsError> for ProviderError {
    fn from(err: TeamsError) -> Self {
        match err {
            TeamsError::Http(e) => ProviderError::Connection(e.to_string()),
            TeamsError::Api(msg) => ProviderError::ExecutionFailed(msg),
            TeamsError::InvalidPayload(msg) => ProviderError::Serialization(msg),
            TeamsError::RateLimited => ProviderError::RateLimited,
            TeamsError::Transient(msg) => ProviderError::Connection(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limited_maps_to_retryable() {
        let provider_err: ProviderError = TeamsError::RateLimited.into();
        assert!(provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::RateLimited));
    }

    #[test]
    fn transient_maps_to_retryable() {
        let provider_err: ProviderError =
            TeamsError::Transient("HTTP 503: service unavailable".into()).into();
        assert!(provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::Connection(_)));
    }

    #[test]
    fn api_error_maps_to_non_retryable() {
        let provider_err: ProviderError = TeamsError::Api("webhook returned error".into()).into();
        assert!(!provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::ExecutionFailed(_)));
    }

    #[test]
    fn invalid_payload_maps_to_serialization() {
        let provider_err: ProviderError =
            TeamsError::InvalidPayload("missing 'text' field".into()).into();
        assert!(!provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::Serialization(_)));
    }

    #[test]
    fn error_display() {
        let err = TeamsError::Api("bad request".into());
        assert_eq!(err.to_string(), "Teams webhook error: bad request");
    }
}
