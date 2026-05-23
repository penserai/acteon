//! Error types for the operations layer.

use thiserror::Error;

/// Errors from the operations layer.
#[derive(Debug, Error)]
pub enum OpsError {
    /// Configuration error.
    #[error("configuration error: {0}")]
    Configuration(String),

    /// Error from the underlying HTTP client.
    #[error(transparent)]
    Client(#[from] acteon_client::Error),
}
