use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A complete swarm execution plan produced by plan gathering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmPlan {
    /// Unique plan identifier.
    pub id: String,
    /// High-level objective the swarm should achieve.
    pub objective: String,
    /// Boundaries and constraints for execution.
    pub scope: SwarmScope,
    /// Conditions that define successful completion.
    pub success_criteria: Vec<String>,
    /// Ordered list of tasks to execute.
    pub tasks: Vec<SwarmTask>,
    /// Role names required by this plan.
    pub agent_roles: Vec<String>,
    /// Estimated total action count (for quota planning).
    pub estimated_actions: u64,
    /// When the plan was created.
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
    /// When the plan was approved (None if not yet approved).
    #[serde(default)]
    pub approved_at: Option<DateTime<Utc>>,
}

impl SwarmPlan {
    /// Returns true if this plan has been approved.
    pub fn is_approved(&self) -> bool {
        self.approved_at.is_some()
    }

    /// Returns all unique role names referenced by tasks.
    pub fn referenced_roles(&self) -> Vec<&str> {
        let mut roles: Vec<&str> = self
            .tasks
            .iter()
            .map(|t| t.assigned_role.as_str())
            .collect();
        roles.sort_unstable();
        roles.dedup();
        roles
    }

    /// Returns all task IDs in the plan.
    pub fn task_ids(&self) -> Vec<&str> {
        self.tasks.iter().map(|t| t.id.as_str()).collect()
    }
}

/// Scope constraints for swarm execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmScope {
    /// Root directory agents work within.
    #[serde(default = "default_working_directory")]
    pub working_directory: PathBuf,
    /// Additional paths agents may access.
    #[serde(default)]
    pub allowed_paths: Vec<PathBuf>,
    /// Regex patterns for paths/commands agents must never touch.
    #[serde(default)]
    pub forbidden_patterns: Vec<String>,
    /// Maximum concurrent agents.
    #[serde(default = "default_max_agents")]
    pub max_agents: usize,
    /// Maximum total run duration in minutes.
    #[serde(default = "default_max_duration")]
    pub max_duration_minutes: u64,
    /// Action types that require human approval before execution.
    #[serde(default)]
    pub require_approval_for: Vec<String>,
}

/// A high-level task consisting of one or more subtasks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmTask {
    /// Unique task identifier.
    pub id: String,
    /// Human-readable task name.
    pub name: String,
    /// Detailed description of what this task accomplishes.
    pub description: String,
    /// Role name assigned to execute this task.
    pub assigned_role: String,
    /// Subtasks to execute (in order within this task).
    pub subtasks: Vec<SwarmSubtask>,
    /// IDs of tasks that must complete before this one starts.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Lower priority number = earlier execution among peers.
    #[serde(default = "default_priority")]
    pub priority: u32,
}

/// An atomic unit of work assigned to a single agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmSubtask {
    /// Unique subtask identifier.
    pub id: String,
    /// Human-readable subtask name.
    pub name: String,
    /// Description of this subtask.
    pub description: String,
    /// The prompt sent to the agent.
    pub prompt: String,
    /// Override the role's default allowed tools.
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    /// Maximum execution time in seconds.
    #[serde(default = "default_subtask_timeout")]
    pub timeout_seconds: u64,
}

impl Default for SwarmScope {
    fn default() -> Self {
        Self {
            working_directory: default_working_directory(),
            allowed_paths: Vec::new(),
            forbidden_patterns: Vec::new(),
            max_agents: default_max_agents(),
            max_duration_minutes: default_max_duration(),
            require_approval_for: Vec::new(),
        }
    }
}

fn default_working_directory() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn default_max_agents() -> usize {
    5
}

fn default_max_duration() -> u64 {
    60
}

fn default_priority() -> u32 {
    10
}

fn default_subtask_timeout() -> u64 {
    900
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_plan() -> SwarmPlan {
        SwarmPlan {
            id: "plan-001".into(),
            objective: "Build a REST API".into(),
            scope: SwarmScope {
                working_directory: "/tmp/project".into(),
                allowed_paths: vec![],
                forbidden_patterns: vec![],
                max_agents: 3,
                max_duration_minutes: 30,
                require_approval_for: vec![],
            },
            success_criteria: vec!["Tests pass".into()],
            tasks: vec![
                SwarmTask {
                    id: "task-1".into(),
                    name: "Scaffold project".into(),
                    description: "Create initial project structure".into(),
                    assigned_role: "coder".into(),
                    subtasks: vec![SwarmSubtask {
                        id: "sub-1a".into(),
                        name: "Create main.rs".into(),
                        description: "Scaffold entry point".into(),
                        prompt: "Create a basic Rust project".into(),
                        allowed_tools: None,
                        timeout_seconds: 120,
                    }],
                    depends_on: vec![],
                    priority: 1,
                },
                SwarmTask {
                    id: "task-2".into(),
                    name: "Add endpoints".into(),
                    description: "Implement REST endpoints".into(),
                    assigned_role: "coder".into(),
                    subtasks: vec![],
                    depends_on: vec!["task-1".into()],
                    priority: 2,
                },
            ],
            agent_roles: vec!["coder".into()],
            estimated_actions: 50,
            created_at: Utc::now(),
            approved_at: None,
        }
    }

    #[test]
    fn test_is_approved() {
        let mut plan = sample_plan();
        assert!(!plan.is_approved());
        plan.approved_at = Some(Utc::now());
        assert!(plan.is_approved());
    }

    #[test]
    fn test_referenced_roles() {
        let plan = sample_plan();
        let roles = plan.referenced_roles();
        assert_eq!(roles, vec!["coder"]);
    }

    #[test]
    fn test_task_ids() {
        let plan = sample_plan();
        let ids = plan.task_ids();
        assert_eq!(ids, vec!["task-1", "task-2"]);
    }

    #[test]
    fn test_plan_roundtrip() {
        let plan = sample_plan();
        let json = serde_json::to_string_pretty(&plan).unwrap();
        let parsed: SwarmPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, plan.id);
        assert_eq!(parsed.tasks.len(), 2);
        assert_eq!(parsed.tasks[1].depends_on, vec!["task-1"]);
    }
}
