use acteon_provider::ProviderError;
use thiserror::Error;

/// Errors specific to AWS provider operations.
#[derive(Debug, Error)]
pub enum AwsProviderError {
    /// The AWS SDK returned an error from the service.
    #[error("AWS service error: {0}")]
    ServiceError(String),

    /// The request was throttled by the AWS service.
    #[error("AWS request throttled")]
    Throttled,

    /// A network or connection error occurred communicating with AWS.
    #[error("AWS connection error: {0}")]
    Connection(String),

    /// The request timed out.
    #[error("AWS request timed out")]
    Timeout,

    /// The action payload was invalid or missing required fields.
    #[error("invalid payload: {0}")]
    InvalidPayload(String),

    /// AWS credential resolution failed.
    #[error("credential error: {0}")]
    CredentialError(String),

    /// Configuration is invalid.
    #[error("invalid configuration: {0}")]
    Configuration(String),
}

impl From<AwsProviderError> for ProviderError {
    fn from(err: AwsProviderError) -> Self {
        match err {
            AwsProviderError::ServiceError(msg) => ProviderError::ExecutionFailed(msg),
            AwsProviderError::Throttled => ProviderError::RateLimited,
            AwsProviderError::Connection(msg) => ProviderError::Connection(msg),
            AwsProviderError::Timeout => ProviderError::Timeout(std::time::Duration::from_secs(30)),
            AwsProviderError::InvalidPayload(msg) => ProviderError::Serialization(msg),
            AwsProviderError::CredentialError(msg) | AwsProviderError::Configuration(msg) => {
                ProviderError::Configuration(msg)
            }
        }
    }
}

/// Classify an AWS SDK error string into the appropriate [`AwsProviderError`].
///
/// This helper inspects the error message for common patterns (throttling,
/// timeout, connection) and maps them to the correct variant.
pub fn classify_sdk_error(error_str: &str) -> AwsProviderError {
    let lower = error_str.to_lowercase();
    if lower.contains("throttl") || lower.contains("rate exceed") || lower.contains("too many") {
        AwsProviderError::Throttled
    } else if lower.contains("timeout") || lower.contains("timed out") {
        AwsProviderError::Timeout
    } else if lower.contains("connection")
        || lower.contains("connect")
        || lower.contains("dns")
        || lower.contains("network")
    {
        AwsProviderError::Connection(error_str.to_owned())
    } else {
        AwsProviderError::ServiceError(error_str.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttled_maps_to_rate_limited() {
        let err: ProviderError = AwsProviderError::Throttled.into();
        assert!(matches!(err, ProviderError::RateLimited));
        assert!(err.is_retryable());
    }

    #[test]
    fn timeout_maps_to_timeout() {
        let err: ProviderError = AwsProviderError::Timeout.into();
        assert!(matches!(err, ProviderError::Timeout(_)));
        assert!(err.is_retryable());
    }

    #[test]
    fn connection_maps_to_connection() {
        let err: ProviderError = AwsProviderError::Connection("reset".into()).into();
        assert!(matches!(err, ProviderError::Connection(_)));
        assert!(err.is_retryable());
    }

    #[test]
    fn service_error_maps_to_execution_failed() {
        let err: ProviderError = AwsProviderError::ServiceError("topic not found".into()).into();
        assert!(matches!(err, ProviderError::ExecutionFailed(_)));
        assert!(!err.is_retryable());
    }

    #[test]
    fn invalid_payload_maps_to_serialization() {
        let err: ProviderError = AwsProviderError::InvalidPayload("missing field".into()).into();
        assert!(matches!(err, ProviderError::Serialization(_)));
        assert!(!err.is_retryable());
    }

    #[test]
    fn credential_error_maps_to_configuration() {
        let err: ProviderError = AwsProviderError::CredentialError("no credentials".into()).into();
        assert!(matches!(err, ProviderError::Configuration(_)));
    }

    #[test]
    fn classify_throttled() {
        let err = classify_sdk_error("Throttling: Rate exceeded");
        assert!(matches!(err, AwsProviderError::Throttled));
    }

    #[test]
    fn classify_timeout() {
        let err = classify_sdk_error("Request timed out after 30s");
        assert!(matches!(err, AwsProviderError::Timeout));
    }

    #[test]
    fn classify_connection() {
        let err = classify_sdk_error("Connection refused: localhost:4566");
        assert!(matches!(err, AwsProviderError::Connection(_)));
    }

    #[test]
    fn classify_generic_service_error() {
        let err = classify_sdk_error("TopicNotFoundException: Topic does not exist");
        assert!(matches!(err, AwsProviderError::ServiceError(_)));
    }

    #[test]
    fn error_display() {
        assert_eq!(
            AwsProviderError::Throttled.to_string(),
            "AWS request throttled"
        );
        assert_eq!(
            AwsProviderError::Timeout.to_string(),
            "AWS request timed out"
        );
        assert_eq!(
            AwsProviderError::ServiceError("bad".into()).to_string(),
            "AWS service error: bad"
        );
    }
}
