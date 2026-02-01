use thiserror::Error;

/// Errors that can occur when running the Acteon server.
#[derive(Debug, Error)]
pub enum ServerError {
    /// A configuration error.
    #[error("configuration error: {0}")]
    Config(String),

    /// An I/O error (e.g. binding the listener).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// A gateway-level error surfaced through the API.
    #[error("gateway error: {0}")]
    Gateway(#[from] acteon_gateway::GatewayError),
}
