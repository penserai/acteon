//! Worker-queue tasks: units of work executed by external workers.
//!
//! Instead of running inside the Acteon server process, a [`WorkerTask`] is
//! enqueued on a named queue and executed by a customer-owned worker that
//! polls `POST /v1/queues/{queue}/poll`, runs the task, and reports the
//! result via `complete` / `fail`. Leases bound how long a worker may hold
//! a task without heartbeating; an expired lease re-queues the task until
//! `max_attempts` is exhausted.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Default maximum delivery attempts for a worker task.
pub const DEFAULT_TASK_MAX_ATTEMPTS: u32 = 3;
/// Default lease duration granted to a polling worker, in seconds.
pub const DEFAULT_TASK_LEASE_SECONDS: u64 = 60;
/// Maximum lease duration a worker may request, in seconds.
pub const MAX_TASK_LEASE_SECONDS: u64 = 3600;

/// Lifecycle status of a worker task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerTaskStatus {
    /// Waiting to be leased by a worker.
    Pending,
    /// Leased by a worker; the lease expires at `lease_expires_at`.
    Leased,
    /// Completed successfully.
    Completed,
    /// Failed terminally (attempts exhausted or non-retryable failure).
    Failed,
    /// Cancelled before completion.
    Cancelled,
}

impl WorkerTaskStatus {
    /// Returns `true` for statuses where the task can still make progress.
    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Pending | Self::Leased)
    }
}

/// A unit of work delivered to external workers via a named queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerTask {
    /// Unique task ID.
    pub task_id: String,
    /// Namespace the task belongs to.
    pub namespace: String,
    /// Tenant the task belongs to.
    pub tenant: String,
    /// Queue the task is routed through.
    pub queue: String,
    /// Action type for the worker's handler dispatch.
    pub action_type: String,
    /// Task payload delivered to the worker.
    pub payload: serde_json::Value,
    /// Current lifecycle status.
    pub status: WorkerTaskStatus,
    /// Delivery attempt counter (1-based once leased; 0 before first lease).
    pub attempt: u32,
    /// Maximum delivery attempts before the task fails terminally.
    pub max_attempts: u32,
    /// When the current lease expires (when `Leased`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_expires_at: Option<DateTime<Utc>>,
    /// Opaque token identifying the current lease. Heartbeat / complete /
    /// fail calls must present the matching token, so a worker whose lease
    /// expired (and was re-delivered elsewhere) cannot clobber the result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_token: Option<String>,
    /// Identifier of the worker currently holding the lease.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    /// Earliest time the task may be leased (used for retry backoff).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before: Option<DateTime<Utc>>,
    /// Result reported by the worker (when `Completed`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Error reported by the worker or the lease reaper (when `Failed`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Chain execution this task belongs to, when enqueued by a `worker`
    /// chain step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<String>,
    /// Step index within the owning chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_index: Option<usize>,
    /// Step name within the owning chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_name: Option<String>,
    /// Workflow execution this task drives, when it is a workflow task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_execution_id: Option<String>,
    /// When the task was enqueued.
    pub created_at: DateTime<Utc>,
    /// When the task was last updated.
    pub updated_at: DateTime<Utc>,
}

impl WorkerTask {
    /// Create a new pending task on a queue.
    #[must_use]
    pub fn new(
        namespace: impl Into<String>,
        tenant: impl Into<String>,
        queue: impl Into<String>,
        action_type: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        let now = Utc::now();
        Self {
            task_id: uuid::Uuid::new_v4().to_string(),
            namespace: namespace.into(),
            tenant: tenant.into(),
            queue: queue.into(),
            action_type: action_type.into(),
            payload,
            status: WorkerTaskStatus::Pending,
            attempt: 0,
            max_attempts: DEFAULT_TASK_MAX_ATTEMPTS,
            lease_expires_at: None,
            lease_token: None,
            worker_id: None,
            not_before: None,
            result: None,
            error: None,
            chain_id: None,
            step_index: None,
            step_name: None,
            workflow_execution_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Set the maximum delivery attempts.
    #[must_use]
    pub fn with_max_attempts(mut self, max_attempts: u32) -> Self {
        self.max_attempts = max_attempts.max(1);
        self
    }

    /// Link the task to a chain step.
    #[must_use]
    pub fn for_chain_step(
        mut self,
        chain_id: impl Into<String>,
        step_index: usize,
        step_name: impl Into<String>,
    ) -> Self {
        self.chain_id = Some(chain_id.into());
        self.step_index = Some(step_index);
        self.step_name = Some(step_name.into());
        self
    }

    /// Link the task to a workflow execution.
    #[must_use]
    pub fn for_workflow(mut self, execution_id: impl Into<String>) -> Self {
        self.workflow_execution_id = Some(execution_id.into());
        self
    }

    /// Returns `true` if the task's lease has expired at `now`.
    #[must_use]
    pub fn lease_expired(&self, now: DateTime<Utc>) -> bool {
        self.status == WorkerTaskStatus::Leased && self.lease_expires_at.is_some_and(|at| now >= at)
    }

    /// Returns `true` if the task is leasable at `now` (pending and past any
    /// retry backoff).
    #[must_use]
    pub fn leasable(&self, now: DateTime<Utc>) -> bool {
        self.status == WorkerTaskStatus::Pending && self.not_before.is_none_or(|nb| now >= nb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_task_is_pending_and_leasable() {
        let task = WorkerTask::new("ns", "t", "builds", "compile", serde_json::json!({}));
        assert_eq!(task.status, WorkerTaskStatus::Pending);
        assert!(task.leasable(Utc::now()));
        assert!(!task.lease_expired(Utc::now()));
        assert_eq!(task.max_attempts, DEFAULT_TASK_MAX_ATTEMPTS);
    }

    #[test]
    fn not_before_defers_leasability() {
        let mut task = WorkerTask::new("ns", "t", "q", "a", serde_json::json!({}));
        task.not_before = Some(Utc::now() + chrono::Duration::seconds(60));
        assert!(!task.leasable(Utc::now()));
        assert!(task.leasable(Utc::now() + chrono::Duration::seconds(120)));
    }

    #[test]
    fn lease_expiry_detection() {
        let mut task = WorkerTask::new("ns", "t", "q", "a", serde_json::json!({}));
        task.status = WorkerTaskStatus::Leased;
        task.lease_expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
        assert!(task.lease_expired(Utc::now()));
    }

    #[test]
    fn status_activity() {
        assert!(WorkerTaskStatus::Pending.is_active());
        assert!(WorkerTaskStatus::Leased.is_active());
        assert!(!WorkerTaskStatus::Completed.is_active());
        assert!(!WorkerTaskStatus::Failed.is_active());
        assert!(!WorkerTaskStatus::Cancelled.is_active());
    }

    #[test]
    fn serde_roundtrip() {
        let task = WorkerTask::new("ns", "t", "q", "a", serde_json::json!({"x": 1}))
            .with_max_attempts(5)
            .for_chain_step("chain-1", 2, "build");
        let json = serde_json::to_string(&task).unwrap();
        let back: WorkerTask = serde_json::from_str(&json).unwrap();
        assert_eq!(back.queue, "q");
        assert_eq!(back.max_attempts, 5);
        assert_eq!(back.chain_id.as_deref(), Some("chain-1"));
        assert_eq!(back.step_index, Some(2));
    }
}
