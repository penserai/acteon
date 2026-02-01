use thiserror::Error;

/// Errors from state store and distributed lock operations.
#[derive(Debug, Error)]
pub enum StateError {
    #[error("connection error: {0}")]
    Connection(String),

    #[error("key not found: {0}")]
    NotFound(String),

    #[error("lock contention: {0}")]
    LockContention(String),

    #[error("lock expired: {0}")]
    LockExpired(String),

    #[error("CAS conflict: expected version {expected}, found {found}")]
    CasConflict { expected: u64, found: u64 },

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("backend error: {0}")]
    Backend(String),

    #[error("operation timed out after {0:?}")]
    Timeout(std::time::Duration),
}
