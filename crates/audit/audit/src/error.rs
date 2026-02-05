/// Errors that can occur during audit store operations.
#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    /// An error from the underlying storage backend.
    #[error("storage error: {0}")]
    Storage(String),

    /// A serialization or deserialization error.
    #[error("serialization error: {0}")]
    Serialization(String),
}
