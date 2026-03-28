use std::path::PathBuf;

/// Errors that can occur during swarm operations.
#[derive(Debug, thiserror::Error)]
pub enum SwarmError {
    #[error("plan validation failed: {0}")]
    PlanValidation(String),

    #[error("dependency cycle detected involving tasks: {}", .0.join(", "))]
    DependencyCycle(Vec<String>),

    #[error("unknown role: {0}")]
    UnknownRole(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("acteon client error: {0}")]
    Acteon(String),

    #[error("tesserai client error: {0}")]
    Tesserai(String),

    #[error("agent spawn failed: {0}")]
    AgentSpawn(String),

    #[error("agent timed out after {timeout_seconds}s: {agent_id}")]
    AgentTimeout {
        agent_id: String,
        timeout_seconds: u64,
    },

    #[error("swarm run timed out after {0} minutes")]
    RunTimeout(u64),

    #[error("workspace setup failed for {path}: {reason}")]
    WorkspaceSetup { path: PathBuf, reason: String },

    #[error("plan gathering failed: {0}")]
    PlanGathering(String),

    #[error("hook error: {0}")]
    Hook(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("toml parse error: {0}")]
    Toml(#[from] toml::de::Error),
}
