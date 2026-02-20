use thiserror::Error;

/// Errors that can occur during blob storage operations.
#[derive(Debug, Error)]
pub enum BlobError {
    /// The requested blob was not found.
    #[error("blob not found: {0}")]
    NotFound(String),

    /// The requested blob has expired.
    #[error("blob expired: {0}")]
    Expired(String),

    /// The blob exceeds the maximum allowed size.
    #[error("blob too large: {size} bytes exceeds limit of {limit} bytes")]
    TooLarge {
        /// Actual size.
        size: u64,
        /// Maximum allowed size.
        limit: u64,
    },

    /// A storage backend error occurred.
    #[error("blob storage error: {0}")]
    Storage(String),

    /// The content type is invalid or not allowed.
    #[error("invalid content type: {0}")]
    InvalidContentType(String),
}
