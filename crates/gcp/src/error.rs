use acteon_provider::ProviderError;
use thiserror::Error;

/// Errors specific to GCP provider operations.
#[derive(Debug, Error)]
pub enum GcpProviderError {
    /// The GCP service returned an error.
    #[error("GCP service error: {0}")]
    ServiceError(String),

    /// The request was throttled by the GCP service.
    #[error("GCP request throttled")]
    Throttled,

    /// A network or connection error occurred communicating with GCP.
    #[error("GCP connection error: {0}")]
    Connection(String),

    /// The request timed out.
    #[error("GCP request timed out")]
    Timeout,

    /// The action payload was invalid or missing required fields.
    #[error("invalid payload: {0}")]
    InvalidPayload(String),

    /// GCP credential resolution failed.
    #[error("credential error: {0}")]
    CredentialError(String),

    /// Configuration is invalid.
    #[error("invalid configuration: {0}")]
    Configuration(String),
}

impl From<GcpProviderError> for ProviderError {
    fn from(err: GcpProviderError) -> Self {
        match err {
            GcpProviderError::ServiceError(msg) => ProviderError::ExecutionFailed(msg),
            GcpProviderError::Throttled => ProviderError::RateLimited,
            GcpProviderError::Connection(msg) => ProviderError::Connection(msg),
            GcpProviderError::Timeout => ProviderError::Timeout(std::time::Duration::from_secs(30)),
            GcpProviderError::InvalidPayload(msg) => ProviderError::Serialization(msg),
            GcpProviderError::CredentialError(msg) | GcpProviderError::Configuration(msg) => {
                ProviderError::Configuration(msg)
            }
        }
    }
}

/// Classify a GCP error string into the appropriate [`GcpProviderError`].
///
/// Inspects the error message for common patterns (throttling, timeout,
/// connection) and maps them to the correct variant.
pub fn classify_gcp_error(error_str: &str) -> GcpProviderError {
    let lower = error_str.to_lowercase();
    if lower.contains("429")
        || lower.contains("throttl")
        || lower.contains("rate exceed")
        || lower.contains("too many")
        || lower.contains("resource_exhausted")
    {
        GcpProviderError::Throttled
    } else if lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("deadline_exceeded")
    {
        GcpProviderError::Timeout
    } else if lower.contains("connection")
        || lower.contains("connect")
        || lower.contains("dns")
        || lower.contains("network")
        || lower.contains("unavailable")
    {
        GcpProviderError::Connection(error_str.to_owned())
    } else {
        GcpProviderError::ServiceError(error_str.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttled_maps_to_rate_limited() {
        let err: ProviderError = GcpProviderError::Throttled.into();
        assert!(matches!(err, ProviderError::RateLimited));
        assert!(err.is_retryable());
    }

    #[test]
    fn timeout_maps_to_timeout() {
        let err: ProviderError = GcpProviderError::Timeout.into();
        assert!(matches!(err, ProviderError::Timeout(_)));
        assert!(err.is_retryable());
    }

    #[test]
    fn connection_maps_to_connection() {
        let err: ProviderError = GcpProviderError::Connection("reset".into()).into();
        assert!(matches!(err, ProviderError::Connection(_)));
        assert!(err.is_retryable());
    }

    #[test]
    fn service_error_maps_to_execution_failed() {
        let err: ProviderError = GcpProviderError::ServiceError("object not found".into()).into();
        assert!(matches!(err, ProviderError::ExecutionFailed(_)));
        assert!(!err.is_retryable());
    }

    #[test]
    fn invalid_payload_maps_to_serialization() {
        let err: ProviderError = GcpProviderError::InvalidPayload("missing field".into()).into();
        assert!(matches!(err, ProviderError::Serialization(_)));
        assert!(!err.is_retryable());
    }

    #[test]
    fn credential_error_maps_to_configuration() {
        let err: ProviderError = GcpProviderError::CredentialError("no credentials".into()).into();
        assert!(matches!(err, ProviderError::Configuration(_)));
    }

    #[test]
    fn classify_throttled_429() {
        let err = classify_gcp_error("HTTP 429: Too Many Requests");
        assert!(matches!(err, GcpProviderError::Throttled));
    }

    #[test]
    fn classify_resource_exhausted() {
        let err = classify_gcp_error("RESOURCE_EXHAUSTED: quota exceeded");
        assert!(matches!(err, GcpProviderError::Throttled));
    }

    #[test]
    fn classify_timeout() {
        let err = classify_gcp_error("Request timed out after 30s");
        assert!(matches!(err, GcpProviderError::Timeout));
    }

    #[test]
    fn classify_deadline_exceeded() {
        let err = classify_gcp_error("DEADLINE_EXCEEDED: operation took too long");
        assert!(matches!(err, GcpProviderError::Timeout));
    }

    #[test]
    fn classify_connection() {
        let err = classify_gcp_error("Connection refused: 10.0.0.1:443");
        assert!(matches!(err, GcpProviderError::Connection(_)));
    }

    #[test]
    fn classify_unavailable() {
        let err = classify_gcp_error("UNAVAILABLE: service is not reachable");
        assert!(matches!(err, GcpProviderError::Connection(_)));
    }

    #[test]
    fn classify_generic_service_error() {
        let err = classify_gcp_error("NOT_FOUND: The specified object does not exist");
        assert!(matches!(err, GcpProviderError::ServiceError(_)));
    }

    #[test]
    fn error_display() {
        assert_eq!(
            GcpProviderError::Throttled.to_string(),
            "GCP request throttled"
        );
        assert_eq!(
            GcpProviderError::Timeout.to_string(),
            "GCP request timed out"
        );
        assert_eq!(
            GcpProviderError::ServiceError("bad".into()).to_string(),
            "GCP service error: bad"
        );
    }
}
