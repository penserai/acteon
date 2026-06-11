//! Checkpoint-based workflow engine.
//!
//! Workflow code runs on external workers (via the worker task queue); the
//! gateway persists execution state, records checkpoints, schedules durable
//! timers, buffers/delivers signals, and enqueues continuation tasks. See
//! [`acteon_core::workflow`] for the execution model.
//!
//! Locking: every mutation of one execution happens under the distributed
//! lock `workflow:{execution_id}`. Cross-execution effects (notifying a
//! parent of a child result, cancelling children on parent close) are
//! deferred until the local lock is released, so parent/child lock pairs
//! are never held simultaneously.

use std::collections::HashMap;
use std::time::Duration;

use chrono::Utc;
use tracing::{debug, warn};

use acteon_core::{
    CHILD_RESULT_SIGNAL_PREFIX, ExecutionEventType, ParentClosePolicy, WORKFLOW_TASK_ACTION_TYPE,
    WorkerTask, WorkerTaskStatus, WorkflowAwait, WorkflowCheckpoint, WorkflowChildRef,
    WorkflowDirective, WorkflowExecution, WorkflowStatus,
};
use acteon_state::{KeyKind, StateKey};

use crate::error::GatewayError;
use crate::gateway::Gateway;

/// State-store kind for workflow execution records.
const WORKFLOW_EXEC_KIND: &str = "workflow_exec";
/// State-store kind for due workflow timers (value = fire time in ms).
const WORKFLOW_TIMER_KIND: &str = "workflow_timer";

/// Maximum delivery attempts for workflow continuation tasks.
const WORKFLOW_TASK_MAX_ATTEMPTS: u32 = 3;
/// TTL for terminal workflow execution records.
const COMPLETED_WORKFLOW_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

/// Filters for listing workflow executions.
#[derive(Debug, Default, Clone)]
pub struct WorkflowFilter {
    /// Only executions of this workflow.
    pub workflow: Option<String>,
    /// Only executions in this status.
    pub status: Option<WorkflowStatus>,
    /// Maximum number of executions to return (default 200).
    pub limit: Option<usize>,
}

/// Cross-execution effects applied after releasing the local lock.
enum FollowUp {
    /// Deliver a child-result signal to the parent execution.
    NotifyParent {
        parent_id: String,
        child_id: String,
        payload: serde_json::Value,
    },
    /// Cancel a child execution (parent-close policy).
    CancelChild { child_id: String },
}

fn exec_key(namespace: &str, tenant: &str, execution_id: &str) -> StateKey {
    StateKey::new(
        namespace,
        tenant,
        KeyKind::Custom(WORKFLOW_EXEC_KIND.into()),
        execution_id,
    )
}

fn timer_key(namespace: &str, tenant: &str, execution_id: &str) -> StateKey {
    StateKey::new(
        namespace,
        tenant,
        KeyKind::Custom(WORKFLOW_TIMER_KIND.into()),
        execution_id,
    )
}

impl Gateway {
    /// Start a new workflow execution and enqueue its first continuation
    /// task on the worker queue.
    pub async fn start_workflow(
        &self,
        namespace: &str,
        tenant: &str,
        workflow: &str,
        queue: &str,
        input: serde_json::Value,
        search_attributes: HashMap<String, serde_json::Value>,
    ) -> Result<WorkflowExecution, GatewayError> {
        if workflow.is_empty() || queue.is_empty() {
            return Err(GatewayError::TaskQueue(
                "workflow and queue names must not be empty".into(),
            ));
        }
        let mut exec = WorkflowExecution::new(namespace, tenant, workflow, queue, input.clone());
        exec.search_attributes = search_attributes;

        self.append_execution_history(
            namespace,
            tenant,
            &exec.execution_id,
            ExecutionEventType::ExecutionStarted {
                name: workflow.to_owned(),
                version: 1,
                input,
            },
            None,
        )
        .await;

        // Persist the execution BEFORE the task becomes pollable: a fast
        // worker settling the first continuation must find the record. (The
        // reverse order silently wedges the execution; a crash between
        // persist and enqueue is surfaced as a start error instead.)
        let task = Self::build_continuation_task(&exec);
        exec.current_task_id = Some(task.task_id.clone());
        self.persist_workflow(&exec, None).await?;
        self.enqueue_worker_task(task).await?;
        debug!(
            execution_id = %exec.execution_id,
            workflow = %workflow,
            queue = %queue,
            "workflow execution started"
        );
        Ok(exec)
    }

    /// Start a child workflow from a parent execution, idempotently keyed by
    /// `checkpoint`. Returns the child execution ID (the recorded one when
    /// the checkpoint already exists).
    #[allow(clippy::too_many_arguments)]
    pub async fn start_child_workflow(
        &self,
        namespace: &str,
        tenant: &str,
        parent_id: &str,
        checkpoint: &str,
        workflow: &str,
        queue: Option<&str>,
        input: serde_json::Value,
        parent_close_policy: ParentClosePolicy,
    ) -> Result<String, GatewayError> {
        let guard = self.lock_workflow(parent_id).await?;

        let result: Result<(String, bool), GatewayError> = async {
            let mut parent = self
                .load_workflow(namespace, tenant, parent_id)
                .await?
                .ok_or_else(|| {
                    GatewayError::TaskQueue(format!("workflow execution not found: {parent_id}"))
                })?;
            if !parent.status.is_active() {
                return Err(GatewayError::TaskQueue(format!(
                    "workflow execution is not active (status: {:?})",
                    parent.status
                )));
            }
            // Idempotent replay: the child was already started.
            if let Some(existing) = parent.checkpoint(checkpoint) {
                let child_id = existing.data["child_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                return Ok((child_id, false));
            }

            let mut child = WorkflowExecution::new(
                namespace,
                tenant,
                workflow,
                queue.unwrap_or(parent.queue.as_str()),
                input.clone(),
            );
            child.parent_id = Some(parent_id.to_owned());
            let child_id = child.execution_id.clone();

            parent.record_checkpoint(checkpoint, serde_json::json!({ "child_id": child_id }));
            parent.children.push(WorkflowChildRef {
                execution_id: child_id.clone(),
                parent_close_policy,
            });
            self.persist_workflow(&parent, None).await?;
            self.append_execution_history(
                namespace,
                tenant,
                parent_id,
                ExecutionEventType::ChildStarted {
                    child_id: child_id.clone(),
                    name: workflow.to_owned(),
                },
                None,
            )
            .await;

            // Persist + enqueue the child while still under the parent lock:
            // the child is brand new, so no other actor can contend on it.
            self.append_execution_history(
                namespace,
                tenant,
                &child_id,
                ExecutionEventType::ExecutionStarted {
                    name: workflow.to_owned(),
                    version: 1,
                    input,
                },
                None,
            )
            .await;
            // Persist before enqueue, mirroring start_workflow: the parent
            // lock does not cover the child's settle path.
            let task = Self::build_continuation_task(&child);
            child.current_task_id = Some(task.task_id.clone());
            self.persist_workflow(&child, None).await?;
            self.enqueue_worker_task(task).await?;
            Ok((child_id, true))
        }
        .await;

        guard
            .release()
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))?;

        let (child_id, started) = result?;
        if started {
            debug!(parent_id = %parent_id, child_id = %child_id, "child workflow started");
        }
        Ok(child_id)
    }

    /// Load a workflow execution.
    pub async fn get_workflow_execution(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: &str,
    ) -> Result<Option<WorkflowExecution>, GatewayError> {
        self.load_workflow(namespace, tenant, execution_id).await
    }

    /// List workflow executions, most recently created first.
    pub async fn list_workflow_executions(
        &self,
        namespace: &str,
        tenant: &str,
        filter: &WorkflowFilter,
    ) -> Result<Vec<WorkflowExecution>, GatewayError> {
        let entries = self
            .state
            .scan_keys(
                namespace,
                tenant,
                KeyKind::Custom(WORKFLOW_EXEC_KIND.into()),
                None,
            )
            .await?;
        let mut executions = Vec::new();
        for (_, raw) in entries {
            let Ok(json) = self.decrypt_state_value(&raw) else {
                continue;
            };
            let Ok(exec) = serde_json::from_str::<WorkflowExecution>(&json) else {
                continue;
            };
            if filter
                .workflow
                .as_deref()
                .is_some_and(|w| exec.workflow != w)
            {
                continue;
            }
            if filter.status.is_some_and(|s| exec.status != s) {
                continue;
            }
            executions.push(exec);
        }
        executions.sort_by_key(|e| std::cmp::Reverse(e.created_at));
        executions.truncate(filter.limit.unwrap_or(200));
        Ok(executions)
    }

    /// Record a checkpoint on a running execution (called by the worker SDK
    /// after each completed step). Idempotent by checkpoint name.
    pub async fn record_workflow_checkpoint(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: &str,
        name: &str,
        data: serde_json::Value,
    ) -> Result<WorkflowCheckpoint, GatewayError> {
        if name.is_empty() {
            return Err(GatewayError::TaskQueue(
                "checkpoint name must not be empty".into(),
            ));
        }
        let guard = self.lock_workflow(execution_id).await?;
        let result: Result<WorkflowCheckpoint, GatewayError> = async {
            let mut exec = self
                .load_workflow(namespace, tenant, execution_id)
                .await?
                .ok_or_else(|| {
                    GatewayError::TaskQueue(format!("workflow execution not found: {execution_id}"))
                })?;
            if !exec.status.is_active() {
                return Err(GatewayError::TaskQueue(format!(
                    "workflow execution is not active (status: {:?})",
                    exec.status
                )));
            }
            let already = exec.checkpoint(name).is_some();
            let checkpoint = exec.record_checkpoint(name, data);
            if !already {
                self.persist_workflow(&exec, None).await?;
                self.append_execution_history(
                    namespace,
                    tenant,
                    execution_id,
                    ExecutionEventType::CheckpointRecorded {
                        name: checkpoint.name.clone(),
                        seq: checkpoint.seq,
                    },
                    None,
                )
                .await;
            }
            Ok(checkpoint)
        }
        .await;
        guard
            .release()
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))?;
        result
    }

    /// Deliver an external signal to a workflow execution. If the execution
    /// is awaiting this signal it resumes immediately; otherwise the signal
    /// is buffered and consumed by the next matching await.
    pub async fn signal_workflow(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: &str,
        signal_name: &str,
        payload: serde_json::Value,
    ) -> Result<(), GatewayError> {
        if signal_name.is_empty() {
            return Err(GatewayError::TaskQueue(
                "signal name must not be empty".into(),
            ));
        }
        let guard = self.lock_workflow(execution_id).await?;
        let result: Result<(), GatewayError> = async {
            let mut exec = self
                .load_workflow(namespace, tenant, execution_id)
                .await?
                .ok_or_else(|| {
                    GatewayError::TaskQueue(format!("workflow execution not found: {execution_id}"))
                })?;
            if !exec.status.is_active() {
                return Err(GatewayError::TaskQueue(format!(
                    "workflow execution is not active (status: {:?})",
                    exec.status
                )));
            }

            self.append_execution_history(
                namespace,
                tenant,
                execution_id,
                ExecutionEventType::SignalReceived {
                    signal_name: signal_name.to_owned(),
                    payload: payload.clone(),
                },
                None,
            )
            .await;

            let awaiting_this = matches!(
                &exec.awaiting,
                Some(WorkflowAwait::Signal { signal_name: awaited, .. }) if awaited == signal_name
            );
            if awaiting_this {
                let Some(WorkflowAwait::Signal { checkpoint, .. }) = exec.awaiting.take() else {
                    unreachable!()
                };
                exec.record_checkpoint(&checkpoint, payload);
                exec.status = WorkflowStatus::Running;
                let _ = self
                    .state
                    .remove_timeout_index(&timer_key(namespace, tenant, execution_id))
                    .await;
                let task_id = self.enqueue_workflow_continuation(&exec).await?;
                exec.current_task_id = Some(task_id);
                self.persist_workflow(&exec, None).await?;
                debug!(
                    execution_id,
                    signal_name, "workflow signal consumed; resumed"
                );
            } else {
                exec.buffered_signals.push(acteon_core::BufferedSignal {
                    name: signal_name.to_owned(),
                    payload,
                    received_at: Utc::now(),
                });
                exec.updated_at = Utc::now();
                self.persist_workflow(&exec, None).await?;
                debug!(execution_id, signal_name, "workflow signal buffered");
            }
            Ok(())
        }
        .await;
        guard
            .release()
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))?;
        result
    }

    /// Cancel a workflow execution. Cancels the in-flight continuation task
    /// (best-effort), notifies the parent, and applies parent-close policy
    /// to children.
    pub async fn cancel_workflow(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: &str,
        reason: Option<String>,
    ) -> Result<WorkflowExecution, GatewayError> {
        let guard = self.lock_workflow(execution_id).await?;
        let result: Result<(WorkflowExecution, Vec<FollowUp>), GatewayError> = async {
            let mut exec = self
                .load_workflow(namespace, tenant, execution_id)
                .await?
                .ok_or_else(|| {
                    GatewayError::TaskQueue(format!("workflow execution not found: {execution_id}"))
                })?;
            if !exec.status.is_active() {
                return Err(GatewayError::TaskQueue(format!(
                    "workflow execution is not active (status: {:?})",
                    exec.status
                )));
            }
            exec.status = WorkflowStatus::Cancelled;
            exec.error.clone_from(&reason);
            exec.awaiting = None;
            exec.updated_at = Utc::now();
            let _ = self
                .state
                .remove_timeout_index(&timer_key(namespace, tenant, execution_id))
                .await;
            self.persist_workflow(&exec, Some(COMPLETED_WORKFLOW_TTL))
                .await?;
            self.append_execution_history(
                namespace,
                tenant,
                execution_id,
                ExecutionEventType::ExecutionCancelled { reason },
                Some(COMPLETED_WORKFLOW_TTL),
            )
            .await;

            if let Some(task_id) = exec.current_task_id.clone() {
                let _ = self.cancel_worker_task(namespace, tenant, &task_id).await;
            }
            let follow_ups =
                Self::close_follow_ups(&exec, serde_json::json!({ "status": "cancelled" }));
            Ok((exec, follow_ups))
        }
        .await;
        guard
            .release()
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))?;

        let (exec, follow_ups) = result?;
        self.run_follow_ups(namespace, tenant, follow_ups).await;
        Ok(exec)
    }

    /// Apply the outcome of a settled continuation task to its workflow
    /// execution. Called by the task queue when a workflow task completes
    /// or fails terminally. Best-effort: errors are logged, never returned
    /// to the worker that settled the task.
    pub(crate) async fn settle_workflow_task(&self, task: &WorkerTask) {
        let Some(execution_id) = task.workflow_execution_id.clone() else {
            return;
        };
        let directive = match task.status {
            WorkerTaskStatus::Completed => {
                let result = task.result.clone().unwrap_or_default();
                match WorkflowDirective::from_task_result(&result) {
                    Some(directive) => directive,
                    // A `directive` key that failed to parse must not be
                    // mistaken for a workflow result — fail loudly instead
                    // of silently completing (e.g. a float `seconds`).
                    None if result.get("directive").is_some() => WorkflowDirective::Fail {
                        error: format!("malformed workflow directive: {result}"),
                    },
                    // A plain (non-directive) result means the workflow
                    // function returned it directly.
                    None => WorkflowDirective::Complete { result },
                }
            }
            WorkerTaskStatus::Failed | WorkerTaskStatus::Cancelled => WorkflowDirective::Fail {
                error: task
                    .error
                    .clone()
                    .unwrap_or_else(|| "workflow task failed".into()),
            },
            _ => return,
        };
        if let Err(e) = self
            .apply_workflow_directive(
                &task.namespace,
                &task.tenant,
                &execution_id,
                &task.task_id,
                directive,
            )
            .await
        {
            warn!(
                execution_id = %execution_id,
                task_id = %task.task_id,
                error = %e,
                "failed to apply workflow directive"
            );
        }
    }

    /// Apply a directive from a settled continuation task.
    #[allow(clippy::too_many_lines)]
    async fn apply_workflow_directive(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: &str,
        task_id: &str,
        directive: WorkflowDirective,
    ) -> Result<(), GatewayError> {
        let guard = self.lock_workflow(execution_id).await?;
        let result: Result<Vec<FollowUp>, GatewayError> = async {
            let Some(mut exec) = self.load_workflow(namespace, tenant, execution_id).await? else {
                return Ok(Vec::new());
            };
            if !exec.status.is_active() {
                return Ok(Vec::new());
            }
            // Ignore directives from stale tasks (e.g. a lease expired,
            // the task was re-delivered, and the original worker finished
            // anyway — the queue's lease-token check already blocks most
            // of this; this guards the workflow side).
            if exec.current_task_id.as_deref() != Some(task_id) {
                debug!(
                    execution_id,
                    task_id, "stale workflow task settled; directive ignored"
                );
                return Ok(Vec::new());
            }
            exec.current_task_id = None;

            match directive {
                WorkflowDirective::Complete { result } => {
                    exec.status = WorkflowStatus::Completed;
                    exec.result = Some(result.clone());
                    exec.awaiting = None;
                    exec.updated_at = Utc::now();
                    self.persist_workflow(&exec, Some(COMPLETED_WORKFLOW_TTL))
                        .await?;
                    self.append_execution_history(
                        namespace,
                        tenant,
                        execution_id,
                        ExecutionEventType::ExecutionCompleted,
                        Some(COMPLETED_WORKFLOW_TTL),
                    )
                    .await;
                    debug!(execution_id, "workflow completed");
                    Ok(Self::close_follow_ups(
                        &exec,
                        serde_json::json!({ "status": "completed", "result": result }),
                    ))
                }
                WorkflowDirective::Fail { error } => {
                    exec.status = WorkflowStatus::Failed;
                    exec.error = Some(error.clone());
                    exec.awaiting = None;
                    exec.updated_at = Utc::now();
                    self.persist_workflow(&exec, Some(COMPLETED_WORKFLOW_TTL))
                        .await?;
                    self.append_execution_history(
                        namespace,
                        tenant,
                        execution_id,
                        ExecutionEventType::ExecutionFailed {
                            error: error.clone(),
                        },
                        Some(COMPLETED_WORKFLOW_TTL),
                    )
                    .await;
                    warn!(execution_id, error = %error, "workflow failed");
                    Ok(Self::close_follow_ups(
                        &exec,
                        serde_json::json!({ "status": "failed", "error": error }),
                    ))
                }
                WorkflowDirective::Sleep {
                    checkpoint,
                    seconds,
                } => {
                    if exec.checkpoint(&checkpoint).is_some() {
                        // Already slept (replayed suspend); continue at once.
                        let next = self.enqueue_workflow_continuation(&exec).await?;
                        exec.current_task_id = Some(next);
                        self.persist_workflow(&exec, None).await?;
                        return Ok(Vec::new());
                    }
                    #[allow(clippy::cast_possible_wrap)]
                    let fire_at = Utc::now() + chrono::Duration::seconds(seconds.max(1) as i64);
                    exec.awaiting = Some(WorkflowAwait::Timer {
                        checkpoint: checkpoint.clone(),
                        fire_at,
                    });
                    exec.status = WorkflowStatus::WaitingTimer;
                    exec.updated_at = Utc::now();
                    self.persist_workflow(&exec, None).await?;
                    self.state
                        .index_timeout(
                            &timer_key(namespace, tenant, execution_id),
                            fire_at.timestamp_millis(),
                        )
                        .await?;
                    self.append_execution_history(
                        namespace,
                        tenant,
                        execution_id,
                        ExecutionEventType::TimerStarted {
                            step_name: checkpoint,
                            fire_at,
                        },
                        None,
                    )
                    .await;
                    debug!(execution_id, %fire_at, "workflow sleeping");
                    Ok(Vec::new())
                }
                WorkflowDirective::AwaitSignal {
                    checkpoint,
                    name,
                    timeout_seconds,
                } => {
                    if exec.checkpoint(&checkpoint).is_some() {
                        let next = self.enqueue_workflow_continuation(&exec).await?;
                        exec.current_task_id = Some(next);
                        self.persist_workflow(&exec, None).await?;
                        return Ok(Vec::new());
                    }
                    // A buffered signal satisfies the await immediately.
                    if let Some(buffered) = exec.take_buffered_signal(&name) {
                        exec.record_checkpoint(&checkpoint, buffered.payload);
                        exec.status = WorkflowStatus::Running;
                        let next = self.enqueue_workflow_continuation(&exec).await?;
                        exec.current_task_id = Some(next);
                        self.persist_workflow(&exec, None).await?;
                        self.append_execution_history(
                            namespace,
                            tenant,
                            execution_id,
                            ExecutionEventType::CheckpointRecorded {
                                name: checkpoint,
                                seq: exec.checkpoints.len() as u64,
                            },
                            None,
                        )
                        .await;
                        return Ok(Vec::new());
                    }
                    #[allow(clippy::cast_possible_wrap)]
                    let timeout_at = timeout_seconds
                        .map(|s| Utc::now() + chrono::Duration::seconds(s.max(1) as i64));
                    exec.awaiting = Some(WorkflowAwait::Signal {
                        checkpoint: checkpoint.clone(),
                        signal_name: name.clone(),
                        timeout_at,
                    });
                    exec.status = WorkflowStatus::WaitingSignal;
                    exec.updated_at = Utc::now();
                    self.persist_workflow(&exec, None).await?;
                    if let Some(t) = timeout_at {
                        self.state
                            .index_timeout(
                                &timer_key(namespace, tenant, execution_id),
                                t.timestamp_millis(),
                            )
                            .await?;
                    }
                    self.append_execution_history(
                        namespace,
                        tenant,
                        execution_id,
                        ExecutionEventType::SignalAwaited {
                            step_name: checkpoint,
                            signal_name: name,
                            timeout_at,
                        },
                        None,
                    )
                    .await;
                    debug!(execution_id, "workflow awaiting signal");
                    Ok(Vec::new())
                }
            }
        }
        .await;

        guard
            .release()
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))?;

        let follow_ups = result?;
        self.run_follow_ups(namespace, tenant, follow_ups).await;
        Ok(())
    }

    /// Fire due workflow timers (durable sleeps and signal-wait timeouts).
    /// Driven by the background processor on the chain-advance tick.
    pub async fn process_due_workflow_timers(&self) -> Result<usize, GatewayError> {
        let now = Utc::now();
        // The shared timeout index returns only due entries (O(log N + M));
        // other consumers of the feed filter by their own key kind, as here.
        let expired = self
            .state
            .get_expired_timeouts(now.timestamp_millis())
            .await?;

        let mut fired = 0;
        for key in expired {
            if !key.contains(":workflow_timer:") {
                continue;
            }
            // Canonical key: {namespace}:{tenant}:workflow_timer:{execution_id}
            let parts: Vec<&str> = key.splitn(4, ':').collect();
            if parts.len() < 4 {
                continue;
            }
            let (namespace, tenant, execution_id) = (parts[0], parts[1], parts[3]);
            match self
                .fire_workflow_timer(namespace, tenant, execution_id)
                .await
            {
                Ok(true) => fired += 1,
                Ok(false) => {}
                Err(e) => {
                    warn!(execution_id, error = %e, "failed to fire workflow timer");
                }
            }
        }
        Ok(fired)
    }

    /// Fire one due timer. Returns `true` when the execution was resumed.
    async fn fire_workflow_timer(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: &str,
    ) -> Result<bool, GatewayError> {
        let guard = self.lock_workflow(execution_id).await?;
        let result: Result<bool, GatewayError> = async {
            let timer = timer_key(namespace, tenant, execution_id);
            let Some(mut exec) = self.load_workflow(namespace, tenant, execution_id).await? else {
                let _ = self.state.remove_timeout_index(&timer).await;
                return Ok(false);
            };

            let now = Utc::now();
            match exec.awaiting.clone() {
                Some(WorkflowAwait::Timer {
                    checkpoint,
                    fire_at,
                }) if now >= fire_at => {
                    exec.record_checkpoint(
                        &checkpoint,
                        serde_json::json!({ "fired_at": now.to_rfc3339() }),
                    );
                    exec.awaiting = None;
                    exec.status = WorkflowStatus::Running;
                    let _ = self.state.remove_timeout_index(&timer).await;
                    let next = self.enqueue_workflow_continuation(&exec).await?;
                    exec.current_task_id = Some(next);
                    self.persist_workflow(&exec, None).await?;
                    self.append_execution_history(
                        namespace,
                        tenant,
                        execution_id,
                        ExecutionEventType::TimerFired {
                            step_name: checkpoint,
                        },
                        None,
                    )
                    .await;
                    Ok(true)
                }
                Some(WorkflowAwait::Signal {
                    checkpoint,
                    signal_name,
                    timeout_at: Some(timeout_at),
                }) if now >= timeout_at => {
                    exec.record_checkpoint(&checkpoint, serde_json::json!({ "timed_out": true }));
                    exec.awaiting = None;
                    exec.status = WorkflowStatus::Running;
                    let _ = self.state.remove_timeout_index(&timer).await;
                    let next = self.enqueue_workflow_continuation(&exec).await?;
                    exec.current_task_id = Some(next);
                    self.persist_workflow(&exec, None).await?;
                    self.append_execution_history(
                        namespace,
                        tenant,
                        execution_id,
                        ExecutionEventType::SignalTimedOut {
                            step_name: checkpoint,
                            signal_name,
                        },
                        None,
                    )
                    .await;
                    Ok(true)
                }
                _ => {
                    // Stale timer entry (the await already resolved).
                    let _ = self.state.remove_timeout_index(&timer).await;
                    Ok(false)
                }
            }
        }
        .await;
        guard
            .release()
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))?;
        result
    }

    /// Compute the cross-execution effects of a terminal transition.
    fn close_follow_ups(
        exec: &WorkflowExecution,
        parent_payload: serde_json::Value,
    ) -> Vec<FollowUp> {
        let mut follow_ups = Vec::new();
        if let Some(parent_id) = exec.parent_id.clone() {
            follow_ups.push(FollowUp::NotifyParent {
                parent_id,
                child_id: exec.execution_id.clone(),
                payload: parent_payload,
            });
        }
        for child in &exec.children {
            if child.parent_close_policy == ParentClosePolicy::Cancel {
                follow_ups.push(FollowUp::CancelChild {
                    child_id: child.execution_id.clone(),
                });
            }
        }
        follow_ups
    }

    /// Apply cross-execution effects after the local lock is released.
    async fn run_follow_ups(&self, namespace: &str, tenant: &str, follow_ups: Vec<FollowUp>) {
        for follow_up in follow_ups {
            match follow_up {
                FollowUp::NotifyParent {
                    parent_id,
                    child_id,
                    payload,
                } => {
                    let signal = format!("{CHILD_RESULT_SIGNAL_PREFIX}{child_id}");
                    if let Err(e) = self
                        .signal_workflow(namespace, tenant, &parent_id, &signal, payload)
                        .await
                    {
                        // The parent may itself already be terminal.
                        debug!(parent_id, child_id, error = %e, "child-result signal not delivered");
                    }
                }
                FollowUp::CancelChild { child_id } => {
                    if let Err(e) = Box::pin(self.cancel_workflow(
                        namespace,
                        tenant,
                        &child_id,
                        Some("parent workflow closed".into()),
                    ))
                    .await
                    {
                        debug!(child_id, error = %e, "child not cancelled (may be terminal)");
                    }
                }
            }
        }
    }

    /// Build a slim continuation task referencing the execution. The worker
    /// resolves input + recorded checkpoints from the execution record (one
    /// GET per continuation), so the queued payload stays O(1) instead of
    /// re-serializing an ever-growing checkpoint snapshot on every resume.
    fn build_continuation_task(exec: &WorkflowExecution) -> WorkerTask {
        let payload = serde_json::json!({
            "execution_id": exec.execution_id,
            "workflow": exec.workflow,
        });
        WorkerTask::new(
            exec.namespace.as_str(),
            exec.tenant.as_str(),
            exec.queue.as_str(),
            WORKFLOW_TASK_ACTION_TYPE,
            payload,
        )
        .with_max_attempts(WORKFLOW_TASK_MAX_ATTEMPTS)
        .for_workflow(exec.execution_id.clone())
    }

    /// Enqueue a continuation task for an execution mutation made under the
    /// workflow lock (settle paths take the same lock, so enqueue-then-
    /// persist is race-free there).
    async fn enqueue_workflow_continuation(
        &self,
        exec: &WorkflowExecution,
    ) -> Result<String, GatewayError> {
        let task = Self::build_continuation_task(exec);
        let task_id = task.task_id.clone();
        self.enqueue_worker_task(task).await?;
        Ok(task_id)
    }

    async fn lock_workflow(
        &self,
        execution_id: &str,
    ) -> Result<Box<dyn acteon_state::LockGuard>, GatewayError> {
        self.lock
            .acquire(
                &format!("workflow:{execution_id}"),
                Duration::from_secs(30),
                Duration::from_secs(5),
            )
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))
    }

    async fn load_workflow(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: &str,
    ) -> Result<Option<WorkflowExecution>, GatewayError> {
        match self
            .state
            .get(&exec_key(namespace, tenant, execution_id))
            .await?
        {
            Some(raw) => {
                let json = self.decrypt_state_value(&raw)?;
                serde_json::from_str(&json).map(Some).map_err(|e| {
                    GatewayError::TaskQueue(format!(
                        "failed to deserialize workflow execution: {e}"
                    ))
                })
            }
            None => Ok(None),
        }
    }

    async fn persist_workflow(
        &self,
        exec: &WorkflowExecution,
        ttl: Option<Duration>,
    ) -> Result<(), GatewayError> {
        let json = serde_json::to_string(exec).map_err(|e| {
            GatewayError::TaskQueue(format!("failed to serialize workflow execution: {e}"))
        })?;
        let stored = self.encrypt_state_value(&json)?;
        self.state
            .set(
                &exec_key(&exec.namespace, &exec.tenant, &exec.execution_id),
                &stored,
                ttl,
            )
            .await?;
        Ok(())
    }
}
