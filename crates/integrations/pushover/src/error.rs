use acteon_provider::ProviderError;
use thiserror::Error;

/// Errors specific to the `Pushover` provider.
///
/// These are internal errors that get converted into [`ProviderError`]
/// at the public API boundary. The variants deliberately mirror the
/// other on-call receiver crates so operators see the same retry
/// semantics across the Alertmanager parity set.
#[derive(Debug, Error)]
pub enum PushoverError {
    /// An HTTP-level transport error occurred.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// The Pushover API returned a **permanent** non-success response
    /// (a 4xx that is not a rate-limit or auth failure).
    #[error("Pushover API error: {0}")]
    Api(String),

    /// The Pushover API returned a **transient** non-success response
    /// (5xx server error or 408 Request Timeout). Surfaced as
    /// `ProviderError::Connection` so the gateway's retry logic
    /// re-queues the dispatch instead of dropping the notification.
    #[error("Pushover transient error: {0}")]
    Transient(String),

    /// The action payload is missing required fields or has invalid structure.
    #[error("invalid payload: {0}")]
    InvalidPayload(String),

    /// The provider received an HTTP 429 (Too Many Requests) response.
    #[error("rate limited by Pushover")]
    RateLimited,

    /// The provider received an HTTP 401/403 response — typically a bad
    /// app token or user key.
    #[error("authentication failed: {0}")]
    Unauthorized(String),

    /// The payload referenced a `user_key` that is not present in the
    /// configured recipients map.
    #[error("unknown Pushover recipient: {0}")]
    UnknownRecipient(String),

    /// No `user_key` was provided in the payload and no fallback applies
    /// (no default recipient, and the recipient map is not a single-entry map).
    #[error("no user_key in payload and no default recipient configured")]
    NoDefaultRecipient,
}

impl From<PushoverError> for ProviderError {
    fn from(err: PushoverError) -> Self {
        match err {
            PushoverError::Http(e) => ProviderError::Connection(e.to_string()),
            PushoverError::Api(msg) => ProviderError::ExecutionFailed(msg),
            PushoverError::Transient(msg) => ProviderError::Connection(msg),
            PushoverError::InvalidPayload(msg) => ProviderError::Serialization(msg),
            PushoverError::RateLimited => ProviderError::RateLimited,
            PushoverError::Unauthorized(msg) => ProviderError::Configuration(msg),
            PushoverError::UnknownRecipient(name) => {
                ProviderError::Configuration(format!("unknown Pushover recipient: {name}"))
            }
            PushoverError::NoDefaultRecipient => ProviderError::Configuration(
                "no user_key in payload and no default recipient configured".into(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limited_is_retryable() {
        let e: ProviderError = PushoverError::RateLimited.into();
        assert!(e.is_retryable());
        assert!(matches!(e, ProviderError::RateLimited));
    }

    #[test]
    fn transient_maps_to_retryable_connection() {
        let e: ProviderError =
            PushoverError::Transient("HTTP 503: Service Unavailable".into()).into();
        assert!(e.is_retryable());
        assert!(matches!(e, ProviderError::Connection(_)));
    }

    #[test]
    fn api_is_non_retryable() {
        let e: ProviderError = PushoverError::Api("bad request".into()).into();
        assert!(!e.is_retryable());
        assert!(matches!(e, ProviderError::ExecutionFailed(_)));
    }

    #[test]
    fn unauthorized_is_configuration() {
        let e: ProviderError = PushoverError::Unauthorized("bad token".into()).into();
        assert!(!e.is_retryable());
        assert!(matches!(e, ProviderError::Configuration(_)));
    }

    #[test]
    fn unknown_recipient_is_configuration() {
        let e: ProviderError = PushoverError::UnknownRecipient("ops-gone".into()).into();
        assert!(!e.is_retryable());
        assert!(matches!(e, ProviderError::Configuration(_)));
    }

    #[test]
    fn invalid_payload_is_serialization() {
        let e: ProviderError = PushoverError::InvalidPayload("missing message".into()).into();
        assert!(!e.is_retryable());
        assert!(matches!(e, ProviderError::Serialization(_)));
    }

    #[test]
    fn display_messages() {
        assert_eq!(
            PushoverError::Api("bad".into()).to_string(),
            "Pushover API error: bad"
        );
        assert_eq!(
            PushoverError::Transient("503".into()).to_string(),
            "Pushover transient error: 503"
        );
        assert_eq!(
            PushoverError::RateLimited.to_string(),
            "rate limited by Pushover"
        );
        assert_eq!(
            PushoverError::UnknownRecipient("ops".into()).to_string(),
            "unknown Pushover recipient: ops"
        );
    }
}
