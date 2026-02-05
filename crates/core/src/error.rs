use thiserror::Error;

/// Top-level error type for the Acteon system.
#[derive(Debug, Error)]
pub enum ActeonError {
    #[error("state error: {0}")]
    State(String),

    #[error("rule error: {0}")]
    Rule(String),

    #[error("provider error: {0}")]
    Provider(String),

    #[error("executor error: {0}")]
    Executor(String),

    #[error("gateway error: {0}")]
    Gateway(String),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("configuration error: {0}")]
    Configuration(String),

    #[error("{0}")]
    Other(String),
}
