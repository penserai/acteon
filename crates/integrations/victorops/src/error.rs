use acteon_provider::ProviderError;
use thiserror::Error;

/// Errors specific to the `VictorOps` provider.
///
/// These are internal errors that get converted into [`ProviderError`]
/// at the public API boundary. The variants deliberately mirror
/// `acteon-opsgenie` so operators see the same retry semantics across
/// on-call receivers.
#[derive(Debug, Error)]
pub enum VictorOpsError {
    /// An HTTP-level transport error occurred.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// The `VictorOps` API returned a **permanent** non-success
    /// response (a 4xx that is not a rate-limit or auth failure).
    /// Surfaced as `ExecutionFailed` and **not** retried — retrying
    /// a malformed request will never succeed.
    #[error("VictorOps API error: {0}")]
    Api(String),

    /// The `VictorOps` API returned a **transient** non-success
    /// response (5xx server error or 408 Request Timeout). The
    /// request body was fine; the server was temporarily unable to
    /// handle it.
    ///
    /// Surfaced as `ProviderError::Connection` so the gateway's
    /// retry logic re-queues the dispatch instead of dropping the
    /// alert on the floor during a brief `VictorOps` outage.
    #[error("VictorOps transient error: {0}")]
    Transient(String),

    /// The action payload is missing required fields or has invalid structure.
    #[error("invalid payload: {0}")]
    InvalidPayload(String),

    /// The provider received an HTTP 429 (Too Many Requests) response.
    #[error("rate limited by VictorOps")]
    RateLimited,

    /// The provider received an HTTP 401/403 response — typically a
    /// bad or revoked api key / routing key.
    #[error("authentication failed: {0}")]
    Unauthorized(String),

    /// The payload referenced a routing key that is not present in
    /// the configured routing-key map.
    #[error("unknown VictorOps routing key: {0}")]
    UnknownRoutingKey(String),

    /// No routing key was provided in the payload and none of the
    /// config's fallbacks apply (no default routing key, and the
    /// routing-key map is not a single-entry map).
    #[error("no routing_key in payload and no default routing key configured")]
    NoDefaultRoutingKey,
}

impl From<VictorOpsError> for ProviderError {
    fn from(err: VictorOpsError) -> Self {
        match err {
            VictorOpsError::Http(e) => ProviderError::Connection(e.to_string()),
            VictorOpsError::Api(msg) => ProviderError::ExecutionFailed(msg),
            VictorOpsError::Transient(msg) => ProviderError::Connection(msg),
            VictorOpsError::InvalidPayload(msg) => ProviderError::Serialization(msg),
            VictorOpsError::RateLimited => ProviderError::RateLimited,
            VictorOpsError::Unauthorized(msg) => ProviderError::Configuration(msg),
            VictorOpsError::UnknownRoutingKey(name) => {
                ProviderError::Configuration(format!("unknown VictorOps routing key: {name}"))
            }
            VictorOpsError::NoDefaultRoutingKey => ProviderError::Configuration(
                "no routing_key in payload and no default routing key configured".into(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limited_maps_to_retryable() {
        let provider_err: ProviderError = VictorOpsError::RateLimited.into();
        assert!(provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::RateLimited));
    }

    #[test]
    fn api_error_maps_to_non_retryable() {
        let provider_err: ProviderError = VictorOpsError::Api("bad request".into()).into();
        assert!(!provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::ExecutionFailed(_)));
    }

    #[test]
    fn transient_error_maps_to_retryable_connection() {
        // 5xx/408 live-blip errors must be retried instead of
        // dropping the alert on the floor.
        let provider_err: ProviderError =
            VictorOpsError::Transient("HTTP 503: service unavailable".into()).into();
        assert!(provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::Connection(_)));
    }

    #[test]
    fn invalid_payload_maps_to_serialization() {
        let provider_err: ProviderError =
            VictorOpsError::InvalidPayload("missing entity_id".into()).into();
        assert!(!provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::Serialization(_)));
    }

    #[test]
    fn unauthorized_maps_to_configuration() {
        let provider_err: ProviderError = VictorOpsError::Unauthorized("bad key".into()).into();
        assert!(!provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::Configuration(_)));
    }

    #[test]
    fn unknown_routing_key_maps_to_configuration() {
        let provider_err: ProviderError =
            VictorOpsError::UnknownRoutingKey("team-gone".into()).into();
        assert!(!provider_err.is_retryable());
        assert!(matches!(provider_err, ProviderError::Configuration(_)));
    }

    #[test]
    fn display_messages() {
        assert_eq!(
            VictorOpsError::Api("bad".into()).to_string(),
            "VictorOps API error: bad"
        );
        assert_eq!(
            VictorOpsError::Transient("503".into()).to_string(),
            "VictorOps transient error: 503"
        );
        assert_eq!(
            VictorOpsError::RateLimited.to_string(),
            "rate limited by VictorOps"
        );
        assert_eq!(
            VictorOpsError::UnknownRoutingKey("team-x".into()).to_string(),
            "unknown VictorOps routing key: team-x"
        );
        assert_eq!(
            VictorOpsError::NoDefaultRoutingKey.to_string(),
            "no routing_key in payload and no default routing key configured"
        );
    }
}
