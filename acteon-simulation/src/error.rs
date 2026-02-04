//! Error types for the simulation framework.

use thiserror::Error;

/// Errors that can occur in simulation operations.
#[derive(Debug, Error)]
pub enum SimulationError {
    /// Gateway configuration or operation error.
    #[error("gateway error: {0}")]
    Gateway(String),

    /// Configuration error.
    #[error("configuration error: {0}")]
    Configuration(String),

    /// No ports available for allocation.
    #[error("no ports available for allocation")]
    PortExhausted,

    /// Node not found.
    #[error("node not found: {0}")]
    NodeNotFound(String),

    /// Provider not found.
    #[error("provider not found: {0}")]
    ProviderNotFound(String),

    /// IO error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Backend connection error.
    #[error("backend connection error: {0}")]
    BackendConnection(String),

    /// HTTP client error.
    #[error("http error: {0}")]
    Http(String),

    /// Dispatch error from server.
    #[error("dispatch error: {0}")]
    Dispatch(String),
}
