use acteon_provider::ProviderError;
use thiserror::Error;

/// Errors specific to the Discord provider.
///
/// These are internal errors that get converted into [`ProviderError`] at the
/// public API boundary.
#[derive(Debug, Error)]
pub enum DiscordError {
    /// An HTTP-level transport error occurred.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// The Discord API returned an error response.
    #[error("Discord API error: {0}")]
    Api(String),

    /// The action payload is missing required fields or has invalid structure.
    #[error("invalid payload: {0}")]
    InvalidPayload(String),

    /// The provider received an HTTP 429 (Too Many Requests) response.
    #[error("rate limited by Discord")]
    RateLimited,
}

impl From<DiscordError> for ProviderError {
    fn from(err: DiscordError) -> Self {
        match err {
            DiscordError::Http(e) => ProviderError::Connection(e.to_string()),
            DiscordError::Api(msg) => ProviderError::ExecutionFailed(msg),
            DiscordError::InvalidPayload(msg) => ProviderError::Serialization(msg),
            DiscordError::RateLimited => ProviderError::RateLimited,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limited_maps_to_retryable() {
        let provider_err: ProviderError = DiscordError::RateLimited.into();
        assert!(provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::RateLimited));
    }

    #[test]
    fn api_error_maps_to_non_retryable() {
        let provider_err: ProviderError = DiscordError::Api("invalid webhook".into()).into();
        assert!(!provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::ExecutionFailed(_)));
    }

    #[test]
    fn invalid_payload_maps_to_serialization() {
        let provider_err: ProviderError =
            DiscordError::InvalidPayload("missing content".into()).into();
        assert!(!provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::Serialization(_)));
    }

    #[test]
    fn error_display() {
        let err = DiscordError::Api("bad request".into());
        assert_eq!(err.to_string(), "Discord API error: bad request");
    }
}
