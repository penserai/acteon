use acteon_provider::ProviderError;
use thiserror::Error;

/// Errors specific to the webhook provider.
///
/// These are internal errors that get converted into [`ProviderError`] at the
/// public API boundary.
#[derive(Debug, Error)]
pub enum WebhookError {
    /// An HTTP-level transport error occurred.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// The remote endpoint returned an unexpected status code.
    #[error("unexpected status {status}: {body}")]
    UnexpectedStatus { status: u16, body: String },

    /// The action payload could not be serialized for the request body.
    #[error("invalid payload: {0}")]
    InvalidPayload(String),

    /// The provider received an HTTP 429 (Too Many Requests) response.
    #[error("rate limited by remote endpoint")]
    RateLimited,

    /// HMAC signature computation failed.
    #[error("HMAC signing error: {0}")]
    SigningError(String),
}

impl From<WebhookError> for ProviderError {
    fn from(err: WebhookError) -> Self {
        match err {
            WebhookError::Http(e) => {
                if e.is_timeout() {
                    ProviderError::Timeout(std::time::Duration::from_secs(0))
                } else {
                    ProviderError::Connection(e.to_string())
                }
            }
            WebhookError::UnexpectedStatus { status, body } => {
                if status == 429 {
                    ProviderError::RateLimited
                } else if (500..600).contains(&status) {
                    // Server errors are retryable
                    ProviderError::Connection(format!("HTTP {status}: {body}"))
                } else {
                    ProviderError::ExecutionFailed(format!("HTTP {status}: {body}"))
                }
            }
            WebhookError::InvalidPayload(msg) => ProviderError::Serialization(msg),
            WebhookError::RateLimited => ProviderError::RateLimited,
            WebhookError::SigningError(msg) => ProviderError::Configuration(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limited_maps_to_retryable() {
        let provider_err: ProviderError = WebhookError::RateLimited.into();
        assert!(provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::RateLimited));
    }

    #[test]
    fn unexpected_status_429_maps_to_rate_limited() {
        let provider_err: ProviderError = WebhookError::UnexpectedStatus {
            status: 429,
            body: "too many requests".into(),
        }
        .into();
        assert!(provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::RateLimited));
    }

    #[test]
    fn unexpected_status_500_maps_to_retryable_connection() {
        let provider_err: ProviderError = WebhookError::UnexpectedStatus {
            status: 500,
            body: "internal server error".into(),
        }
        .into();
        assert!(provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::Connection(_)));
    }

    #[test]
    fn unexpected_status_400_maps_to_non_retryable() {
        let provider_err: ProviderError = WebhookError::UnexpectedStatus {
            status: 400,
            body: "bad request".into(),
        }
        .into();
        assert!(!provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::ExecutionFailed(_)));
    }

    #[test]
    fn invalid_payload_maps_to_serialization() {
        let provider_err: ProviderError =
            WebhookError::InvalidPayload("missing field".into()).into();
        assert!(!provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::Serialization(_)));
    }

    #[test]
    fn signing_error_maps_to_configuration() {
        let provider_err: ProviderError = WebhookError::SigningError("bad secret".into()).into();
        assert!(!provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::Configuration(_)));
    }

    #[test]
    fn error_display() {
        let err = WebhookError::RateLimited;
        assert_eq!(err.to_string(), "rate limited by remote endpoint");

        let err = WebhookError::UnexpectedStatus {
            status: 503,
            body: "unavailable".into(),
        };
        assert_eq!(err.to_string(), "unexpected status 503: unavailable");

        let err = WebhookError::InvalidPayload("bad json".into());
        assert_eq!(err.to_string(), "invalid payload: bad json");
    }
}
