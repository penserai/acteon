use thiserror::Error;

/// Errors that can occur during gateway dispatch operations.
#[derive(Debug, Error)]
pub enum GatewayError {
    /// An error occurred in the state store.
    #[error("state error: {0}")]
    State(#[from] acteon_state::StateError),

    /// An error occurred during rule evaluation.
    #[error("rule error: {0}")]
    Rule(#[from] acteon_rules::RuleError),

    /// An error from a provider operation.
    #[error("provider error: {0}")]
    Provider(#[from] acteon_provider::ProviderError),

    /// The requested provider was not found in the registry.
    #[error("provider not found: {0}")]
    ProviderNotFound(String),

    /// Failed to acquire a distributed lock.
    #[error("lock acquisition failed: {0}")]
    LockFailed(String),

    /// The gateway was misconfigured (e.g. missing required components).
    #[error("configuration error: {0}")]
    Configuration(String),
}
