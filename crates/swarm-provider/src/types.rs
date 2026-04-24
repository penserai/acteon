use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use acteon_swarm::types::run::{RunMetrics, SwarmRunStatus as InnerSwarmRunStatus};
use acteon_swarm::{SwarmPlan, SwarmRun};

/// Payload shape the `swarm` provider expects on an incoming `Action`.
///
/// Callers dispatch a goal by sending an `Action` with `provider = "swarm"`
/// and this struct serialized as its `payload`. A pre-built plan is required
/// — natural-language planning is a deliberate V2 extension so the synchronous
/// dispatch call stays fast.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct GoalRequest {
    /// Human-readable objective. Mirrors `plan.objective` for logging and
    /// stream events.
    pub objective: String,
    /// Pre-built swarm plan to execute. Validated and required at V1.
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub plan: SwarmPlan,
    /// Optional idempotency key. When set, repeated dispatches with the same
    /// key collapse to the same run (caller sees the existing `run_id`).
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

/// Response body the provider returns immediately after accepting a goal.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SwarmGoalAccepted {
    /// Unique identifier for the background run.
    pub run_id: String,
    /// Plan ID from the goal.
    pub plan_id: String,
    /// When the background run was scheduled.
    pub started_at: DateTime<Utc>,
    /// Objective, echoed from the request for operator visibility.
    pub objective: String,
}

/// Status of a swarm run tracked by the registry.
///
/// Mirrors [`acteon_swarm::types::run::SwarmRunStatus`] but adds states that
/// exist only from the server's perspective (`Accepted`, `Cancelling`). Kept
/// as a separate enum so operational semantics can evolve without forcing
/// changes on the swarm crate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum SwarmRunStatus {
    Accepted,
    Running,
    Adversarial,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    Cancelling,
}

impl SwarmRunStatus {
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::TimedOut
        )
    }
}

impl From<InnerSwarmRunStatus> for SwarmRunStatus {
    fn from(value: InnerSwarmRunStatus) -> Self {
        match value {
            InnerSwarmRunStatus::Initializing => Self::Accepted,
            InnerSwarmRunStatus::Running => Self::Running,
            InnerSwarmRunStatus::Adversarial => Self::Adversarial,
            InnerSwarmRunStatus::Completed => Self::Completed,
            InnerSwarmRunStatus::Failed => Self::Failed,
            InnerSwarmRunStatus::Cancelled => Self::Cancelled,
            InnerSwarmRunStatus::TimedOut => Self::TimedOut,
        }
    }
}

/// Snapshot of a swarm run at a point in time, safe to return from the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SwarmRunSnapshot {
    pub run_id: String,
    pub plan_id: String,
    pub objective: String,
    pub status: SwarmRunStatus,
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<Object>))]
    pub metrics: Option<RunMetrics>,
    #[serde(default)]
    pub error: Option<String>,
    /// The originating action's namespace/tenant (for tenant-scoped listing).
    pub namespace: String,
    pub tenant: String,
}

impl SwarmRunSnapshot {
    #[must_use]
    pub fn apply_run(mut self, run: &SwarmRun) -> Self {
        self.status = run.status.clone().into();
        self.finished_at = run.finished_at;
        self.metrics = Some(run.metrics.clone());
        self
    }
}

/// Filter for listing swarm runs through the API.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SwarmRunFilter {
    pub namespace: Option<String>,
    pub tenant: Option<String>,
    pub status: Option<SwarmRunStatus>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}
