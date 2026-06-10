//! Workflow executions: checkpoint-based durable workflows-as-code.
//!
//! A workflow execution runs customer code on external workers via the
//! worker task queue. The server never executes workflow logic — it
//! persists *checkpoints* (named, recorded results of completed steps) and
//! schedules continuation tasks. On every resume the worker re-runs the
//! workflow function from the top; the SDK's context replays recorded
//! checkpoints instantly (returning the stored result instead of
//! re-executing), so the function deterministically reaches the first
//! un-checkpointed operation and continues from there. Unlike replay-based
//! engines there is no determinism sandbox: only the order and names of
//! checkpointed operations must be stable across resumes.
//!
//! Suspension points (durable timers, signal waits, child workflows) are
//! expressed as *directives* the worker returns when completing a workflow
//! task — see [`WorkflowDirective`].

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Action type used for workflow continuation tasks on worker queues.
pub const WORKFLOW_TASK_ACTION_TYPE: &str = "__workflow__";

/// Signal-name prefix used to deliver child-workflow results to parents.
/// `ctx.wait_for_child(child_id)` awaits the signal `__child:{child_id}`.
pub const CHILD_RESULT_SIGNAL_PREFIX: &str = "__child:";

/// Lifecycle status of a workflow execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStatus {
    /// A continuation task is queued or running on a worker.
    Running,
    /// Paused on a durable timer.
    WaitingTimer,
    /// Paused waiting for an external signal.
    WaitingSignal,
    /// Completed successfully.
    Completed,
    /// Failed terminally.
    Failed,
    /// Cancelled.
    Cancelled,
}

impl WorkflowStatus {
    /// Returns `true` while the execution can still make progress.
    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Running | Self::WaitingTimer | Self::WaitingSignal
        )
    }
}

/// What happens to running children when a parent workflow reaches a
/// terminal state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParentClosePolicy {
    /// Children keep running independently.
    #[default]
    Abandon,
    /// Children are cancelled when the parent closes.
    Cancel,
}

/// A recorded checkpoint: the durable result of one completed operation
/// (step, fired timer, received signal, started child).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowCheckpoint {
    /// Dense 1-based sequence number.
    pub seq: u64,
    /// Unique checkpoint name within the execution (the SDK derives it from
    /// the step name and call order).
    pub name: String,
    /// Recorded payload, returned verbatim on replay.
    pub data: serde_json::Value,
    /// When the checkpoint was recorded.
    pub recorded_at: DateTime<Utc>,
}

/// What a suspended workflow execution is waiting on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkflowAwait {
    /// A durable timer; on fire the checkpoint is recorded and a
    /// continuation task is enqueued.
    Timer {
        /// Checkpoint recorded when the timer fires.
        checkpoint: String,
        /// When the timer fires.
        fire_at: DateTime<Utc>,
    },
    /// An external signal (optionally with a timeout).
    Signal {
        /// Checkpoint recorded when the signal (or timeout) resolves.
        checkpoint: String,
        /// Name of the awaited signal.
        signal_name: String,
        /// When the wait times out, if configured.
        timeout_at: Option<DateTime<Utc>>,
    },
}

/// Reference to a child workflow started by this execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowChildRef {
    /// Child execution ID.
    pub execution_id: String,
    /// What happens to the child when this execution closes.
    #[serde(default)]
    pub parent_close_policy: ParentClosePolicy,
}

/// A buffered signal that arrived before the workflow awaited it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BufferedSignal {
    /// Signal name.
    pub name: String,
    /// Signal payload.
    pub payload: serde_json::Value,
    /// When the signal arrived.
    pub received_at: DateTime<Utc>,
}

/// Persistent state of one workflow execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowExecution {
    /// Unique execution ID.
    pub execution_id: String,
    /// Workflow name (matched to a handler registered on the worker).
    pub workflow: String,
    /// Worker queue continuation tasks are routed through.
    pub queue: String,
    /// Namespace the execution belongs to.
    pub namespace: String,
    /// Tenant the execution belongs to.
    pub tenant: String,
    /// Current lifecycle status.
    pub status: WorkflowStatus,
    /// Input the execution started with.
    pub input: serde_json::Value,
    /// Result reported on completion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Error reported on terminal failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Recorded checkpoints in order.
    #[serde(default)]
    pub checkpoints: Vec<WorkflowCheckpoint>,
    /// Signals received but not yet consumed by an await.
    #[serde(default)]
    pub buffered_signals: Vec<BufferedSignal>,
    /// What the execution is waiting on, when suspended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awaiting: Option<WorkflowAwait>,
    /// Parent execution ID for child workflows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// Children started by this execution.
    #[serde(default)]
    pub children: Vec<WorkflowChildRef>,
    /// User-defined, queryable attributes.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub search_attributes: HashMap<String, serde_json::Value>,
    /// ID of the in-flight continuation task, when `Running`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_task_id: Option<String>,
    /// When the execution started.
    pub created_at: DateTime<Utc>,
    /// When the execution was last updated.
    pub updated_at: DateTime<Utc>,
}

impl WorkflowExecution {
    /// Create a new running execution.
    #[must_use]
    pub fn new(
        namespace: impl Into<String>,
        tenant: impl Into<String>,
        workflow: impl Into<String>,
        queue: impl Into<String>,
        input: serde_json::Value,
    ) -> Self {
        let now = Utc::now();
        Self {
            execution_id: uuid::Uuid::new_v4().to_string(),
            workflow: workflow.into(),
            queue: queue.into(),
            namespace: namespace.into(),
            tenant: tenant.into(),
            status: WorkflowStatus::Running,
            input,
            result: None,
            error: None,
            checkpoints: Vec::new(),
            buffered_signals: Vec::new(),
            awaiting: None,
            parent_id: None,
            children: Vec::new(),
            search_attributes: HashMap::new(),
            current_task_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Look up a checkpoint by name.
    #[must_use]
    pub fn checkpoint(&self, name: &str) -> Option<&WorkflowCheckpoint> {
        self.checkpoints.iter().find(|c| c.name == name)
    }

    /// Record a checkpoint, returning the existing one when the name was
    /// already recorded (idempotent replays).
    pub fn record_checkpoint(
        &mut self,
        name: impl Into<String>,
        data: serde_json::Value,
    ) -> WorkflowCheckpoint {
        let name = name.into();
        if let Some(existing) = self.checkpoint(&name) {
            return existing.clone();
        }
        let checkpoint = WorkflowCheckpoint {
            seq: self.checkpoints.len() as u64 + 1,
            name,
            data,
            recorded_at: Utc::now(),
        };
        self.checkpoints.push(checkpoint.clone());
        self.updated_at = checkpoint.recorded_at;
        checkpoint
    }

    /// Remove and return the oldest buffered signal with the given name.
    pub fn take_buffered_signal(&mut self, name: &str) -> Option<BufferedSignal> {
        let idx = self.buffered_signals.iter().position(|s| s.name == name)?;
        Some(self.buffered_signals.remove(idx))
    }
}

/// Directive returned by a worker when settling a workflow continuation
/// task: either the workflow finished (complete/fail) or it suspends until
/// a timer, signal, or child resolves.
///
/// Serialized as the worker task's `result` payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "directive", rename_all = "snake_case")]
pub enum WorkflowDirective {
    /// The workflow function returned: the execution completes.
    Complete {
        /// Workflow result.
        result: serde_json::Value,
    },
    /// The workflow function raised a terminal error.
    Fail {
        /// Error message.
        error: String,
    },
    /// Suspend on a durable timer.
    Sleep {
        /// Checkpoint to record when the timer fires.
        checkpoint: String,
        /// Seconds to sleep.
        seconds: u64,
    },
    /// Suspend until an external signal arrives.
    AwaitSignal {
        /// Checkpoint to record when the signal (or timeout) resolves.
        checkpoint: String,
        /// Name of the awaited signal.
        name: String,
        /// Optional timeout in seconds; on expiry the checkpoint records
        /// `{"timed_out": true}` and the workflow resumes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_seconds: Option<u64>,
    },
}

impl WorkflowDirective {
    /// Parse a directive from a worker task result payload.
    ///
    /// Returns `None` when the payload is not a directive object.
    #[must_use]
    pub fn from_task_result(result: &serde_json::Value) -> Option<Self> {
        serde_json::from_value(result.clone()).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_recording_is_idempotent() {
        let mut exec = WorkflowExecution::new("ns", "t", "wf", "q", serde_json::json!({}));
        let first = exec.record_checkpoint("step-1", serde_json::json!({"v": 1}));
        let replay = exec.record_checkpoint("step-1", serde_json::json!({"v": 999}));
        assert_eq!(first.seq, 1);
        assert_eq!(replay.data, serde_json::json!({"v": 1}));
        assert_eq!(exec.checkpoints.len(), 1);

        let second = exec.record_checkpoint("step-2", serde_json::json!({}));
        assert_eq!(second.seq, 2);
    }

    #[test]
    fn buffered_signals_consumed_fifo_by_name() {
        let mut exec = WorkflowExecution::new("ns", "t", "wf", "q", serde_json::json!({}));
        exec.buffered_signals.push(BufferedSignal {
            name: "go".into(),
            payload: serde_json::json!(1),
            received_at: Utc::now(),
        });
        exec.buffered_signals.push(BufferedSignal {
            name: "go".into(),
            payload: serde_json::json!(2),
            received_at: Utc::now(),
        });
        assert_eq!(
            exec.take_buffered_signal("go").unwrap().payload,
            serde_json::json!(1)
        );
        assert_eq!(
            exec.take_buffered_signal("go").unwrap().payload,
            serde_json::json!(2)
        );
        assert!(exec.take_buffered_signal("go").is_none());
    }

    #[test]
    fn directive_serde() {
        let directive = WorkflowDirective::AwaitSignal {
            checkpoint: "sig-1".into(),
            name: "approved".into(),
            timeout_seconds: Some(3600),
        };
        let json = serde_json::to_value(&directive).unwrap();
        assert_eq!(json["directive"], "await_signal");
        let back = WorkflowDirective::from_task_result(&json).unwrap();
        assert_eq!(back, directive);

        assert!(WorkflowDirective::from_task_result(&serde_json::json!({"plain": true})).is_none());
    }

    #[test]
    fn status_activity() {
        assert!(WorkflowStatus::Running.is_active());
        assert!(WorkflowStatus::WaitingTimer.is_active());
        assert!(WorkflowStatus::WaitingSignal.is_active());
        assert!(!WorkflowStatus::Completed.is_active());
        assert!(!WorkflowStatus::Failed.is_active());
        assert!(!WorkflowStatus::Cancelled.is_active());
    }
}
