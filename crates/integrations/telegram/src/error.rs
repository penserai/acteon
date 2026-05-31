use acteon_provider::ProviderError;
use thiserror::Error;

/// Errors specific to the Telegram Bot provider.
///
/// These internal errors get converted into [`ProviderError`] at
/// the public API boundary. Variants deliberately mirror the
/// other on-call receiver crates so operators see consistent
/// retry semantics across providers.
#[derive(Debug, Error)]
pub enum TelegramError {
    /// An HTTP-level transport error occurred.
    #[error("HTTP error: {0}")]
    Http(reqwest::Error),

    /// The Telegram API returned a **permanent** non-success
    /// response (a 4xx that is not a rate-limit or auth failure).
    /// Surfaced as `ExecutionFailed` and not retried.
    #[error("Telegram API error: {0}")]
    Api(String),

    /// The Telegram API returned a **transient** non-success
    /// response (5xx server error or 408 Request Timeout).
    /// Surfaced as `ProviderError::Connection` so the gateway's
    /// retry logic re-queues the dispatch.
    #[error("Telegram transient error: {0}")]
    Transient(String),

    /// The action payload is missing required fields or has
    /// invalid structure.
    #[error("invalid payload: {0}")]
    InvalidPayload(String),

    /// The provider received an HTTP 429 (Too Many Requests)
    /// response.
    #[error("rate limited by Telegram")]
    RateLimited,

    /// The provider received an HTTP 401/403/404 response — for
    /// Telegram bots, `404 Not Found` typically means the bot
    /// token is unrecognized, so it is classified as an auth
    /// failure rather than a generic 4xx.
    #[error("authentication failed: {0}")]
    Unauthorized(String),

    /// The payload referenced a `chat` name that is not in the
    /// configured chats map.
    #[error("unknown Telegram chat: {0}")]
    UnknownChat(String),

    /// No `chat` was provided in the payload and no fallback
    /// applies (no default chat, and the chats map is not a
    /// single-entry map).
    #[error("no chat in payload and no default chat configured")]
    NoDefaultChat,
}

impl From<reqwest::Error> for TelegramError {
    fn from(err: reqwest::Error) -> Self {
        // Redact the request URL: for several providers it carries the bot
        // token, webhook secret, or access token, which must never reach
        // error messages, audit records, or the DLQ.
        Self::Http(err.without_url())
    }
}

impl From<TelegramError> for ProviderError {
    fn from(err: TelegramError) -> Self {
        match err {
            TelegramError::Http(e) => ProviderError::Connection(e.to_string()),
            TelegramError::Api(msg) => ProviderError::ExecutionFailed(msg),
            TelegramError::Transient(msg) => ProviderError::Connection(msg),
            TelegramError::InvalidPayload(msg) => ProviderError::Serialization(msg),
            TelegramError::RateLimited => ProviderError::RateLimited,
            TelegramError::Unauthorized(msg) => ProviderError::Configuration(msg),
            TelegramError::UnknownChat(name) => {
                ProviderError::Configuration(format!("unknown Telegram chat: {name}"))
            }
            TelegramError::NoDefaultChat => ProviderError::Configuration(
                "no chat in payload and no default chat configured".into(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limited_is_retryable() {
        let e: ProviderError = TelegramError::RateLimited.into();
        assert!(e.is_retryable());
        assert!(matches!(e, ProviderError::RateLimited));
    }

    #[test]
    fn transient_maps_to_retryable_connection() {
        let e: ProviderError = TelegramError::Transient("HTTP 503".into()).into();
        assert!(e.is_retryable());
        assert!(matches!(e, ProviderError::Connection(_)));
    }

    #[test]
    fn api_is_non_retryable() {
        let e: ProviderError = TelegramError::Api("bad request".into()).into();
        assert!(!e.is_retryable());
        assert!(matches!(e, ProviderError::ExecutionFailed(_)));
    }

    #[test]
    fn unauthorized_is_configuration() {
        let e: ProviderError = TelegramError::Unauthorized("bad token".into()).into();
        assert!(!e.is_retryable());
        assert!(matches!(e, ProviderError::Configuration(_)));
    }

    #[test]
    fn unknown_chat_is_configuration() {
        let e: ProviderError = TelegramError::UnknownChat("ops-gone".into()).into();
        assert!(!e.is_retryable());
        assert!(matches!(e, ProviderError::Configuration(_)));
    }

    #[test]
    fn invalid_payload_is_serialization() {
        let e: ProviderError = TelegramError::InvalidPayload("missing text".into()).into();
        assert!(!e.is_retryable());
        assert!(matches!(e, ProviderError::Serialization(_)));
    }

    #[test]
    fn display_messages() {
        assert_eq!(
            TelegramError::Api("bad".into()).to_string(),
            "Telegram API error: bad"
        );
        assert_eq!(
            TelegramError::Transient("503".into()).to_string(),
            "Telegram transient error: 503"
        );
        assert_eq!(
            TelegramError::RateLimited.to_string(),
            "rate limited by Telegram"
        );
        assert_eq!(
            TelegramError::UnknownChat("ops".into()).to_string(),
            "unknown Telegram chat: ops"
        );
    }

    #[tokio::test]
    async fn http_error_redacts_url() {
        // A transport failure must not leak the request URL (which for Telegram
        // carries the bot token in the path) into the converted ProviderError.
        let marker = "REDACTME-9f8a7b6c";
        let url = format!("http://127.0.0.1:1/bot{marker}/sendMessage");
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .unwrap();
        let raw = client
            .get(&url)
            .send()
            .await
            .expect_err("connecting to 127.0.0.1:1 must fail");

        // Precondition: the raw reqwest error DOES embed the URL/marker.
        assert!(
            raw.to_string().contains(marker),
            "precondition: raw reqwest error should carry the URL"
        );

        // Routing it through `From` (which calls `without_url`) strips it.
        let provider_err: ProviderError = TelegramError::from(raw).into();
        let msg = provider_err.to_string();
        assert!(
            !msg.contains(marker),
            "converted error leaked the URL secret: {msg}"
        );
        assert!(
            !msg.contains("127.0.0.1"),
            "converted error leaked the host: {msg}"
        );
    }
}
