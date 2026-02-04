//! Error types for the Acteon client.

use thiserror::Error;

/// Errors that can occur when using the Acteon client.
#[derive(Debug, Error)]
pub enum Error {
    /// Connection error (network failure, DNS resolution, etc.).
    #[error("connection error: {0}")]
    Connection(String),

    /// HTTP error with status code.
    #[error("HTTP {status}: {message}")]
    Http {
        /// HTTP status code.
        status: u16,
        /// Error message.
        message: String,
    },

    /// API error returned by the server.
    #[error("API error [{code}]: {message}")]
    Api {
        /// Error code.
        code: String,
        /// Error message.
        message: String,
        /// Whether the request can be retried.
        retryable: bool,
    },

    /// Response deserialization error.
    #[error("failed to deserialize response: {0}")]
    Deserialization(String),

    /// Client configuration error.
    #[error("configuration error: {0}")]
    Configuration(String),
}

impl Error {
    /// Returns `true` if this error is retryable.
    ///
    /// Connection errors and API errors marked as retryable return `true`.
    /// HTTP 5xx errors also return `true`.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Connection(_) => true,
            Self::Http { status, .. } => *status >= 500,
            Self::Api { retryable, .. } => *retryable,
            Self::Deserialization(_) => false,
            Self::Configuration(_) => false,
        }
    }

    /// Returns `true` if this is a connection error.
    pub fn is_connection_error(&self) -> bool {
        matches!(self, Self::Connection(_))
    }

    /// Returns `true` if this is an API error.
    pub fn is_api_error(&self) -> bool {
        matches!(self, Self::Api { .. })
    }

    /// Returns the API error code if this is an API error.
    pub fn api_code(&self) -> Option<&str> {
        match self {
            Self::Api { code, .. } => Some(code),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_error_is_retryable() {
        let err = Error::Connection("timeout".to_string());
        assert!(err.is_retryable());
        assert!(err.is_connection_error());
    }

    #[test]
    fn http_5xx_is_retryable() {
        let err = Error::Http {
            status: 500,
            message: "Internal Server Error".to_string(),
        };
        assert!(err.is_retryable());

        let err = Error::Http {
            status: 503,
            message: "Service Unavailable".to_string(),
        };
        assert!(err.is_retryable());
    }

    #[test]
    fn http_4xx_is_not_retryable() {
        let err = Error::Http {
            status: 400,
            message: "Bad Request".to_string(),
        };
        assert!(!err.is_retryable());

        let err = Error::Http {
            status: 404,
            message: "Not Found".to_string(),
        };
        assert!(!err.is_retryable());
    }

    #[test]
    fn api_error_retryable_flag() {
        let retryable = Error::Api {
            code: "RATE_LIMITED".to_string(),
            message: "Too many requests".to_string(),
            retryable: true,
        };
        assert!(retryable.is_retryable());
        assert!(retryable.is_api_error());
        assert_eq!(retryable.api_code(), Some("RATE_LIMITED"));

        let not_retryable = Error::Api {
            code: "INVALID_INPUT".to_string(),
            message: "Invalid action".to_string(),
            retryable: false,
        };
        assert!(!not_retryable.is_retryable());
    }

    #[test]
    fn deserialization_error_not_retryable() {
        let err = Error::Deserialization("invalid JSON".to_string());
        assert!(!err.is_retryable());
    }
}
