use acteon_provider::ProviderError;
use thiserror::Error;

/// Errors specific to Azure provider operations.
#[derive(Debug, Error)]
pub enum AzureProviderError {
    /// The Azure service returned an error.
    #[error("Azure service error: {0}")]
    ServiceError(String),

    /// The request was throttled by the Azure service.
    #[error("Azure request throttled")]
    Throttled,

    /// A network or connection error occurred communicating with Azure.
    #[error("Azure connection error: {0}")]
    Connection(String),

    /// The request timed out.
    #[error("Azure request timed out")]
    Timeout,

    /// The action payload was invalid or missing required fields.
    #[error("invalid payload: {0}")]
    InvalidPayload(String),

    /// Azure credential resolution failed.
    #[error("credential error: {0}")]
    CredentialError(String),

    /// Configuration is invalid.
    #[error("invalid configuration: {0}")]
    Configuration(String),
}

impl From<AzureProviderError> for ProviderError {
    fn from(err: AzureProviderError) -> Self {
        match err {
            AzureProviderError::ServiceError(msg) => ProviderError::ExecutionFailed(msg),
            AzureProviderError::Throttled => ProviderError::RateLimited,
            AzureProviderError::Connection(msg) => ProviderError::Connection(msg),
            AzureProviderError::Timeout => {
                ProviderError::Timeout(std::time::Duration::from_secs(30))
            }
            AzureProviderError::InvalidPayload(msg) => ProviderError::Serialization(msg),
            AzureProviderError::CredentialError(msg) | AzureProviderError::Configuration(msg) => {
                ProviderError::Configuration(msg)
            }
        }
    }
}

/// Classify an Azure SDK error string into the appropriate [`AzureProviderError`].
///
/// Inspects the error message for common patterns (throttling, timeout,
/// connection) and maps them to the correct variant.
pub fn classify_azure_error(error_str: &str) -> AzureProviderError {
    let lower = error_str.to_lowercase();
    if lower.contains("429")
        || lower.contains("throttl")
        || lower.contains("rate exceed")
        || lower.contains("too many")
    {
        AzureProviderError::Throttled
    } else if lower.contains("timeout") || lower.contains("timed out") {
        AzureProviderError::Timeout
    } else if lower.contains("connection")
        || lower.contains("connect")
        || lower.contains("dns")
        || lower.contains("network")
    {
        AzureProviderError::Connection(error_str.to_owned())
    } else {
        AzureProviderError::ServiceError(error_str.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttled_maps_to_rate_limited() {
        let err: ProviderError = AzureProviderError::Throttled.into();
        assert!(matches!(err, ProviderError::RateLimited));
        assert!(err.is_retryable());
    }

    #[test]
    fn timeout_maps_to_timeout() {
        let err: ProviderError = AzureProviderError::Timeout.into();
        assert!(matches!(err, ProviderError::Timeout(_)));
        assert!(err.is_retryable());
    }

    #[test]
    fn connection_maps_to_connection() {
        let err: ProviderError = AzureProviderError::Connection("reset".into()).into();
        assert!(matches!(err, ProviderError::Connection(_)));
        assert!(err.is_retryable());
    }

    #[test]
    fn service_error_maps_to_execution_failed() {
        let err: ProviderError =
            AzureProviderError::ServiceError("container not found".into()).into();
        assert!(matches!(err, ProviderError::ExecutionFailed(_)));
        assert!(!err.is_retryable());
    }

    #[test]
    fn invalid_payload_maps_to_serialization() {
        let err: ProviderError = AzureProviderError::InvalidPayload("missing field".into()).into();
        assert!(matches!(err, ProviderError::Serialization(_)));
        assert!(!err.is_retryable());
    }

    #[test]
    fn credential_error_maps_to_configuration() {
        let err: ProviderError =
            AzureProviderError::CredentialError("no credentials".into()).into();
        assert!(matches!(err, ProviderError::Configuration(_)));
    }

    #[test]
    fn classify_throttled_429() {
        let err = classify_azure_error("HTTP 429: Too Many Requests");
        assert!(matches!(err, AzureProviderError::Throttled));
    }

    #[test]
    fn classify_throttled_keyword() {
        let err = classify_azure_error("Throttling: Rate exceeded");
        assert!(matches!(err, AzureProviderError::Throttled));
    }

    #[test]
    fn classify_timeout() {
        let err = classify_azure_error("Request timed out after 30s");
        assert!(matches!(err, AzureProviderError::Timeout));
    }

    #[test]
    fn classify_connection() {
        let err = classify_azure_error("Connection refused: 10.0.0.1:443");
        assert!(matches!(err, AzureProviderError::Connection(_)));
    }

    #[test]
    fn classify_generic_service_error() {
        let err = classify_azure_error("ContainerNotFound: The specified container does not exist");
        assert!(matches!(err, AzureProviderError::ServiceError(_)));
    }

    #[test]
    fn error_display() {
        assert_eq!(
            AzureProviderError::Throttled.to_string(),
            "Azure request throttled"
        );
        assert_eq!(
            AzureProviderError::Timeout.to_string(),
            "Azure request timed out"
        );
        assert_eq!(
            AzureProviderError::ServiceError("bad".into()).to_string(),
            "Azure service error: bad"
        );
    }
}
