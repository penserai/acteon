use thiserror::Error;

#[derive(Debug, Error)]
pub enum SwarmProviderError {
    #[error("invalid swarm goal payload: {0}")]
    InvalidPayload(String),

    #[error("swarm registry is full: max_concurrent_runs={max}")]
    RegistryFull { max: usize },

    #[error("swarm run not found: {0}")]
    NotFound(String),

    #[error("swarm executor failed: {0}")]
    Executor(String),
}
