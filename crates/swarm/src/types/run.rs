use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Tracks the overall state of a swarm execution run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmRun {
    /// Unique run identifier.
    pub id: String,
    /// ID of the plan being executed.
    pub plan_id: String,
    /// Current status.
    pub status: SwarmRunStatus,
    /// When the run started.
    pub started_at: DateTime<Utc>,
    /// When the run finished.
    #[serde(default)]
    pub finished_at: Option<DateTime<Utc>>,
    /// Per-task completion status.
    pub task_status: HashMap<String, TaskRunStatus>,
    /// Aggregate metrics.
    pub metrics: RunMetrics,
}

/// Overall run status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwarmRunStatus {
    /// Setting up (creating quotas, rules, twins).
    Initializing,
    /// Actively executing tasks.
    Running,
    /// All tasks completed successfully.
    Completed,
    /// One or more tasks failed.
    Failed,
    /// Run was cancelled.
    Cancelled,
    /// Run exceeded its time limit.
    TimedOut,
}

/// Status of a single task within a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskRunStatus {
    /// Waiting for dependencies.
    Pending,
    /// Currently executing subtasks.
    Running,
    /// All subtasks completed.
    Completed,
    /// A subtask failed.
    Failed(String),
    /// Skipped due to dependency failure.
    Skipped,
}

/// Aggregate metrics for a swarm run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunMetrics {
    /// Total actions dispatched across all agents.
    pub total_actions: u64,
    /// Actions that were blocked by rules.
    pub actions_blocked: u64,
    /// Actions that were throttled.
    pub actions_throttled: u64,
    /// Actions that were deduplicated.
    pub actions_deduped: u64,
    /// Number of agent sessions spawned.
    pub agents_spawned: u64,
    /// Number of agent sessions that completed.
    pub agents_completed: u64,
    /// Number of agent sessions that failed.
    pub agents_failed: u64,
    /// Number of plan refinements performed.
    pub refinements: u64,
    /// Semantic memories stored in `TesseraiDB`.
    pub memories_stored: u64,
}
