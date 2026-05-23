use acteon_provider::ProviderError;
use thiserror::Error;

/// Errors specific to the `WeChat` Work provider.
///
/// Variants mirror the other receiver crates so operators see
/// consistent retry semantics across providers.
/// The [`Self::TokenExpired`] variant is `WeChat`-specific: it
/// drives the in-band refresh-and-retry logic inside the provider
/// and is **not** surfaced to the gateway — operators will see a
/// successful retry (or a different terminal error) rather than
/// this variant.
#[derive(Debug, Error)]
pub enum WeChatError {
    /// An HTTP-level transport error occurred.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// The `WeChat` API returned a **permanent** failure
    /// (`errcode != 0` that is neither rate-limit, auth, nor
    /// a known transient class).
    #[error("WeChat API error: {0}")]
    Api(String),

    /// The `WeChat` API returned a **transient** server-busy
    /// class error (e.g. `errcode: -1` system busy, `errcode:
    /// 50001` / `50002` temporary failures, or HTTP 5xx / 408).
    /// Surfaced as `ProviderError::Connection` so the gateway's
    /// retry logic re-queues the dispatch.
    #[error("WeChat transient error: {0}")]
    Transient(String),

    /// The action payload is missing required fields or has
    /// invalid structure.
    #[error("invalid payload: {0}")]
    InvalidPayload(String),

    /// The `WeChat` API returned `errcode: 45009` (interface
    /// call limit exceeded).
    #[error("rate limited by WeChat")]
    RateLimited,

    /// Auth / authorization failure.
    ///
    /// Includes `errcode: 40001` (invalid credential on the
    /// `gettoken` endpoint, typically a bad `corp_secret`),
    /// `errcode: 40013` (invalid `corpid`), and HTTP 401/403
    /// on either endpoint. Surfaced as `Configuration` because
    /// it indicates an operator/credential problem, not a
    /// transient one.
    #[error("authentication failed: {0}")]
    Unauthorized(String),

    /// Internal signal used by the provider's retry loop.
    ///
    /// Returned when the send endpoint responds with
    /// `errcode: 42001` (`access_token` expired) or `40014`
    /// (invalid `access_token`). The provider catches this
    /// variant, invalidates its token cache, refreshes, and
    /// retries the send **exactly once**. If the retry produces
    /// the same error, it is converted to [`Self::Unauthorized`]
    /// so the caller sees a terminal credential problem.
    ///
    /// This variant is never surfaced to the `From<WeChatError>`
    /// conversion for [`ProviderError`]; seeing it in a
    /// `ProviderError` would be a provider bug.
    #[error("WeChat access token expired or invalid (internal retry signal)")]
    TokenExpired,
}

impl From<WeChatError> for ProviderError {
    fn from(err: WeChatError) -> Self {
        match err {
            WeChatError::Http(e) => ProviderError::Connection(e.to_string()),
            WeChatError::Api(msg) => ProviderError::ExecutionFailed(msg),
            WeChatError::Transient(msg) => ProviderError::Connection(msg),
            WeChatError::InvalidPayload(msg) => ProviderError::Serialization(msg),
            WeChatError::RateLimited => ProviderError::RateLimited,
            WeChatError::Unauthorized(msg) => ProviderError::Configuration(msg),
            // TokenExpired should never escape the provider's
            // retry loop. If it does, treat it as an auth
            // failure so the operator at least sees "credential
            // problem" instead of a confusing internal label.
            WeChatError::TokenExpired => ProviderError::Configuration(
                "WeChat access_token refresh failed after retry".into(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limited_is_retryable() {
        let e: ProviderError = WeChatError::RateLimited.into();
        assert!(e.is_retryable());
        assert!(matches!(e, ProviderError::RateLimited));
    }

    #[test]
    fn transient_maps_to_retryable_connection() {
        let e: ProviderError = WeChatError::Transient("errcode -1 system busy".into()).into();
        assert!(e.is_retryable());
        assert!(matches!(e, ProviderError::Connection(_)));
    }

    #[test]
    fn api_is_non_retryable() {
        let e: ProviderError = WeChatError::Api("errcode 40001 invalid credential".into()).into();
        assert!(!e.is_retryable());
        assert!(matches!(e, ProviderError::ExecutionFailed(_)));
    }

    #[test]
    fn unauthorized_is_configuration() {
        let e: ProviderError = WeChatError::Unauthorized("bad corp_secret".into()).into();
        assert!(!e.is_retryable());
        assert!(matches!(e, ProviderError::Configuration(_)));
    }

    #[test]
    fn invalid_payload_is_serialization() {
        let e: ProviderError = WeChatError::InvalidPayload("missing content".into()).into();
        assert!(!e.is_retryable());
        assert!(matches!(e, ProviderError::Serialization(_)));
    }

    #[test]
    fn token_expired_escape_maps_to_configuration() {
        // TokenExpired should never escape the retry loop, but
        // if it does we surface it as a terminal credential
        // problem rather than a confusing internal label.
        let e: ProviderError = WeChatError::TokenExpired.into();
        assert!(!e.is_retryable());
        assert!(matches!(e, ProviderError::Configuration(_)));
    }

    #[test]
    fn display_messages() {
        assert_eq!(
            WeChatError::Api("bad".into()).to_string(),
            "WeChat API error: bad"
        );
        assert_eq!(
            WeChatError::Transient("-1".into()).to_string(),
            "WeChat transient error: -1"
        );
        assert_eq!(
            WeChatError::RateLimited.to_string(),
            "rate limited by WeChat"
        );
        assert_eq!(
            WeChatError::TokenExpired.to_string(),
            "WeChat access token expired or invalid (internal retry signal)"
        );
    }
}
