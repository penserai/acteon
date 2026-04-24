use thiserror::Error;

/// All errors returned by bus backends.
///
/// Kept as a flat enum so callers can match on specific failures
/// without knowing whether they came from Kafka or the in-memory
/// backend.
#[derive(Debug, Error)]
pub enum BusError {
    #[error("bus backend not configured")]
    NotConfigured,

    #[error("topic already exists: {0}")]
    TopicAlreadyExists(String),

    #[error("topic not found: {0}")]
    TopicNotFound(String),

    #[error("invalid topic name: {0}")]
    InvalidTopic(String),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("timed out waiting for broker")]
    Timeout,

    #[error("transport error: {0}")]
    Transport(String),
}
