use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Definition of an agent role with its capabilities and constraints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRole {
    /// Unique role name (e.g., "coder", "researcher").
    pub name: String,
    /// Human-readable description of this role.
    pub description: String,
    /// `MiniJinja` template for the agent's system prompt.
    pub system_prompt_template: String,
    /// Claude Code tool names this role is allowed to use.
    pub allowed_tools: Vec<String>,
    /// Other role names this role can delegate subtasks to.
    #[serde(default)]
    pub can_delegate_to: Vec<String>,
    /// Maximum number of concurrent instances of this role.
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_instances: usize,
}

/// Runtime state of an active agent session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSession {
    /// Unique session identifier.
    pub id: String,
    /// Role this agent is executing as.
    pub role: String,
    /// Task this session belongs to.
    pub task_id: String,
    /// Subtask this session is executing.
    pub subtask_id: String,
    /// OS process ID of the claude subprocess.
    #[serde(default)]
    pub pid: Option<u32>,
    /// Working directory for this agent.
    pub workspace: PathBuf,
    /// Current execution status.
    pub status: AgentSessionStatus,
    /// When the session started.
    pub started_at: DateTime<Utc>,
    /// When the session finished (None if still running).
    #[serde(default)]
    pub finished_at: Option<DateTime<Utc>>,
    /// Number of actions dispatched through Acteon.
    #[serde(default)]
    pub actions_dispatched: u64,
    /// Number of actions blocked by rules.
    #[serde(default)]
    pub actions_blocked: u64,
}

/// Agent session lifecycle status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentSessionStatus {
    /// Waiting to be spawned.
    Pending,
    /// Currently executing.
    Running,
    /// Blocked waiting for human approval.
    WaitingApproval,
    /// Successfully completed.
    Completed,
    /// Failed with an error message.
    Failed(String),
    /// Exceeded timeout.
    TimedOut,
    /// Cancelled by the orchestrator.
    Cancelled,
}

impl AgentSessionStatus {
    /// Returns true if this is a terminal status.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed(_) | Self::TimedOut | Self::Cancelled
        )
    }
}

fn default_max_concurrent() -> usize {
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terminal_status() {
        assert!(!AgentSessionStatus::Pending.is_terminal());
        assert!(!AgentSessionStatus::Running.is_terminal());
        assert!(!AgentSessionStatus::WaitingApproval.is_terminal());
        assert!(AgentSessionStatus::Completed.is_terminal());
        assert!(AgentSessionStatus::Failed("err".into()).is_terminal());
        assert!(AgentSessionStatus::TimedOut.is_terminal());
        assert!(AgentSessionStatus::Cancelled.is_terminal());
    }

    #[test]
    fn test_agent_role_serde() {
        let role = AgentRole {
            name: "coder".into(),
            description: "Writes code".into(),
            system_prompt_template: "You are a coder. Task: {{ task }}".into(),
            allowed_tools: vec!["Read".into(), "Write".into(), "Edit".into(), "Bash".into()],
            can_delegate_to: vec!["researcher".into()],
            max_concurrent_instances: 3,
        };
        let json = serde_json::to_string(&role).unwrap();
        let parsed: AgentRole = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "coder");
        assert_eq!(parsed.allowed_tools.len(), 4);
    }
}
