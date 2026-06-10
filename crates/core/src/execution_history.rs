//! Durable, per-execution event history.
//!
//! Every long-running execution (task chain or workflow) appends an ordered,
//! append-only log of [`ExecutionEvent`]s describing each state transition:
//! steps completing, timers firing, signals arriving, and the terminal
//! outcome. The history is the source of truth for the execution timeline —
//! the UI renders it and `GET /v1/executions/{id}/history` exposes it.
//!
//! Events are stored per execution under the state store and capped at
//! [`MAX_HISTORY_EVENTS`] to bound unbounded growth; once the cap is reached
//! only terminal events are still recorded.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Maximum number of events retained per execution history.
///
/// Once the cap is reached, non-terminal events are dropped (with a warning
/// at the append site) so the terminal outcome is always recorded.
pub const MAX_HISTORY_EVENTS: usize = 5000;

/// A single entry in an execution's history log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionEvent {
    /// Monotonic 1-based sequence number within the execution.
    pub event_id: u64,
    /// When the event was recorded.
    pub timestamp: DateTime<Utc>,
    /// The event payload.
    #[serde(flatten)]
    pub event: ExecutionEventType,
}

/// The kind of state transition an [`ExecutionEvent`] records.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum ExecutionEventType {
    /// The execution started.
    ExecutionStarted {
        /// Chain or workflow name.
        name: String,
        /// Definition version pinned by this execution.
        version: u64,
        /// Input payload the execution started with.
        input: serde_json::Value,
    },
    /// A step completed successfully.
    StepCompleted {
        step_name: String,
        step_index: usize,
        /// 1-based attempt that produced the result (0 = unknown).
        attempt: u32,
    },
    /// A step failed terminally (after exhausting retries).
    StepFailed {
        step_name: String,
        step_index: usize,
        attempt: u32,
        error: String,
    },
    /// A step failed and a retry was scheduled.
    StepRetrying {
        step_name: String,
        step_index: usize,
        attempt: u32,
        error: String,
    },
    /// A step failed and was skipped per its failure policy.
    StepSkipped {
        step_name: String,
        step_index: usize,
        error: Option<String>,
    },
    /// A durable timer started; the execution sleeps until `fire_at`.
    TimerStarted {
        step_name: String,
        fire_at: DateTime<Utc>,
    },
    /// A durable timer fired and the execution resumed.
    TimerFired { step_name: String },
    /// The execution paused waiting for an external signal.
    SignalAwaited {
        step_name: String,
        signal_name: String,
        /// When the wait gives up, if a timeout is configured.
        timeout_at: Option<DateTime<Utc>>,
    },
    /// An external signal was delivered to the execution.
    SignalReceived {
        signal_name: String,
        payload: serde_json::Value,
    },
    /// A wait-for-signal step timed out before the signal arrived.
    SignalTimedOut {
        step_name: String,
        signal_name: String,
    },
    /// A task was enqueued on a worker queue for external execution.
    TaskEnqueued {
        step_name: String,
        task_id: String,
        queue: String,
    },
    /// A worker completed an enqueued task.
    TaskCompleted {
        step_name: String,
        task_id: String,
        attempt: u32,
    },
    /// A worker task failed terminally.
    TaskFailed {
        step_name: String,
        task_id: String,
        attempt: u32,
        error: String,
    },
    /// A child execution (sub-chain or child workflow) was started.
    ChildStarted { child_id: String, name: String },
    /// A workflow checkpoint was recorded (workflow executions only).
    CheckpointRecorded { name: String, seq: u64 },
    /// Search attributes were added or updated on the execution.
    SearchAttributesUpserted {
        attributes: HashMap<String, serde_json::Value>,
    },
    /// The execution completed successfully.
    ExecutionCompleted,
    /// The execution failed.
    ExecutionFailed { error: String },
    /// The execution was cancelled.
    ExecutionCancelled { reason: Option<String> },
    /// The execution exceeded its timeout.
    ExecutionTimedOut,
}

impl ExecutionEventType {
    /// Returns `true` for events that record a terminal execution outcome.
    ///
    /// Terminal events are always appended even when the history is at the
    /// [`MAX_HISTORY_EVENTS`] cap.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::ExecutionCompleted
                | Self::ExecutionFailed { .. }
                | Self::ExecutionCancelled { .. }
                | Self::ExecutionTimedOut
        )
    }
}

/// The full history log for one execution.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionHistory {
    /// Ordered list of events; `event_id` is 1-based and dense.
    pub events: Vec<ExecutionEvent>,
}

impl ExecutionHistory {
    /// Append an event, assigning the next sequence number.
    ///
    /// Returns `false` (and drops the event) when the history is at the
    /// [`MAX_HISTORY_EVENTS`] cap and the event is not terminal.
    pub fn append(&mut self, event: ExecutionEventType) -> bool {
        if self.events.len() >= MAX_HISTORY_EVENTS && !event.is_terminal() {
            return false;
        }
        let event_id = self.events.last().map_or(1, |e| e.event_id + 1);
        self.events.push(ExecutionEvent {
            event_id,
            timestamp: Utc::now(),
            event,
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_assigns_dense_event_ids() {
        let mut history = ExecutionHistory::default();
        assert!(history.append(ExecutionEventType::ExecutionStarted {
            name: "c".into(),
            version: 1,
            input: serde_json::json!({}),
        }));
        assert!(history.append(ExecutionEventType::StepCompleted {
            step_name: "s1".into(),
            step_index: 0,
            attempt: 1,
        }));
        assert!(history.append(ExecutionEventType::ExecutionCompleted));
        let ids: Vec<u64> = history.events.iter().map(|e| e.event_id).collect();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn append_drops_non_terminal_events_at_cap() {
        let mut history = ExecutionHistory::default();
        for i in 0..MAX_HISTORY_EVENTS {
            assert!(history.append(ExecutionEventType::StepCompleted {
                step_name: format!("s{i}"),
                step_index: i,
                attempt: 1,
            }));
        }
        // Non-terminal event past the cap is dropped.
        assert!(!history.append(ExecutionEventType::TimerFired {
            step_name: "t".into()
        }));
        assert_eq!(history.events.len(), MAX_HISTORY_EVENTS);
        // Terminal event is still recorded.
        assert!(history.append(ExecutionEventType::ExecutionCompleted));
        assert_eq!(history.events.len(), MAX_HISTORY_EVENTS + 1);
    }

    #[test]
    fn event_serde_roundtrip_with_tag() {
        let mut history = ExecutionHistory::default();
        history.append(ExecutionEventType::SignalAwaited {
            step_name: "wait".into(),
            signal_name: "approved".into(),
            timeout_at: None,
        });
        let json = serde_json::to_string(&history).unwrap();
        assert!(json.contains("\"event_type\":\"signal_awaited\""));
        let back: ExecutionHistory = serde_json::from_str(&json).unwrap();
        assert_eq!(back.events.len(), 1);
        assert!(matches!(
            back.events[0].event,
            ExecutionEventType::SignalAwaited { .. }
        ));
    }

    #[test]
    fn terminal_classification() {
        assert!(ExecutionEventType::ExecutionCompleted.is_terminal());
        assert!(ExecutionEventType::ExecutionFailed { error: "e".into() }.is_terminal());
        assert!(ExecutionEventType::ExecutionCancelled { reason: None }.is_terminal());
        assert!(ExecutionEventType::ExecutionTimedOut.is_terminal());
        assert!(
            !ExecutionEventType::TimerFired {
                step_name: "t".into()
            }
            .is_terminal()
        );
    }
}
