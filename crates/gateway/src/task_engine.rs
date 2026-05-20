//! A2A Task Engine — Phase 2 of the A2A protocol.
//!
//! Owns the persistence and mutation lifecycle for [`Task`] rows.
//! Every Task lives at a [`KeyKind::A2aTask`] row keyed by
//! `(namespace, tenant, task_id)`. Mutations use the same
//! optimistic-locking pattern as the bus's `cas_update` helper:
//! load-with-version, mutate, `compare_and_swap`, retry on conflict.
//!
//! ## Idempotency
//!
//! A2A clients submit messages with a stable `messageId`. The engine
//! deduplicates by writing a marker at
//! [`KeyKind::A2aMessageDedup`] before applying the message. A
//! re-submission returns the already-committed task without
//! reapplying the mutation.
//!
//! ## Reference-graph validation (graph-bomb defense)
//!
//! A2A `Message`s carry `referenceTaskIds`, so Task A's history can
//! cite Task B, whose history cites Task C, and so on. A malicious or
//! buggy producer could build a cycle (A → B → A) or a pathologically
//! deep / wide graph that exhausts the gateway when traversed.
//!
//! The engine walks the reference graph **at write time** — inside
//! [`TaskEngine::create_task`] and [`TaskEngine::append_history`],
//! before the row is persisted — rather than at read time, so a
//! graph bomb is refused before it ever lands. The walk is bounded
//! three ways: a cycle back to the root is rejected, a path deeper
//! than [`acteon_core::MAX_REFERENCE_DEPTH`] is rejected, and a graph
//! wider than [`MAX_REFERENCE_GRAPH_NODES`] distinct nodes is
//! rejected. See the `check_reference_graph` method.
//!
//! ## Audit integration
//!
//! When an [`AuditStore`] is attached via [`TaskEngine::with_audit`],
//! every successful mutation emits an
//! [`AuditEventKind::A2aTaskTransition`](acteon_audit::AuditEventKind)
//! record — creation, state transition, history append, artifact
//! update, heartbeat, pending-approval stamp, and stale-task reap.
//! The record rides the same [`AuditStore`] the gateway uses, so it
//! inherits hash-chaining and compliance decorators automatically. An
//! audit write failure is logged but never fails the mutation: the
//! task transition is the source of truth, the audit record a
//! best-effort projection of it.
//!
//! ## Human-in-the-loop pauses
//!
//! [`TaskEngine::pause_for_human`] pauses a Task on a human: it
//! transitions the Task to [`TaskState::AuthRequired`] /
//! [`TaskState::InputRequired`] and creates the matching
//! [`acteon_core::BusApproval`] row (a [`PauseKind::UserAuth`] /
//! [`PauseKind::UserInput`] kind), stamping the approval id onto
//! `Task.pending_approval_id`. `BusApproval` is the single
//! "waiting on a human" record — the same row type the bus's
//! operator-approval gate uses. The Task transition itself rides the
//! audit integration above.
//!
//! ## Out of scope for this PR (deferred to follow-ups)
//!
//! - **Protocol codecs**: A2A JSON-RPC 2.0 + REST framing lives in
//!   the server layer, not here. The engine speaks pure Rust types.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::stream::{self, StreamExt, TryStreamExt};
use tracing::{debug, warn};

use acteon_audit::store::AuditStore;
use acteon_core::{
    Artifact, BusApproval, BusApprovalValidationError, DEFAULT_APPROVAL_TTL_MS,
    MAX_APPROVAL_TTL_MS, MAX_REFERENCE_DEPTH, PauseKind, Task, TaskArtifactUpdateEvent,
    TaskMessage, TaskRole, TaskState, TaskValidationError,
};
use acteon_state::{CasResult, KeyKind, StateError, StateKey, StateStore};

use crate::audit_helpers::build_task_audit_record;

/// Max number of CAS retry attempts before declaring contention
/// exhausted. Matches the bus's
/// `crates/server/src/api/bus.rs::MAX_CAS_RETRY_ATTEMPTS`.
pub const MAX_CAS_RETRY_ATTEMPTS: u32 = 8;

/// TTL on A2A `messageId` deduplication markers
/// ([`KeyKind::A2aMessageDedup`]). Without a TTL the dedup keyspace
/// grows unboundedly — a long-running gateway eventually pays linear
/// storage cost in total lifetime traffic.
///
/// 24h is chosen to comfortably exceed any realistic A2A client
/// retry window (clients that haven't given up after a day aren't
/// retrying anyway). After expiry, a *theoretical* very-late retry
/// would re-apply the message rather than dedup. This is the safe
/// failure mode — an erroneous append is recoverable (operator
/// inspects and corrects); a memory leak is not.
pub const MESSAGE_DEDUP_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Max distinct tasks the reference-graph walker visits before
/// declaring the graph too large. [`MAX_REFERENCE_DEPTH`] bounds how
/// *deep* a reference chain may go; this bounds how *wide* it may fan
/// out. Without it, one task referencing tens of thousands of others
/// (`MAX_REFERENCE_TASK_IDS` × `MAX_HISTORY_LEN`) would force that
/// many state-store reads on every write that touches the graph.
///
/// 256 is generous for legitimate citation (a task cites a handful of
/// prior tasks) and tight enough to refuse a fan-out bomb cheaply.
pub const MAX_REFERENCE_GRAPH_NODES: usize = 256;

/// Max state-store reads issued concurrently while fetching one
/// breadth-first level of the reference graph. The walker fetches a
/// whole level at once; this caps how many of those reads are in
/// flight so a wide (near-abusive) frontier cannot open hundreds of
/// simultaneous connections. Legitimate citation graphs are far
/// narrower than this — they complete in a single batch, so the walk
/// costs O(depth) round-trips rather than O(nodes).
const REFERENCE_GRAPH_FETCH_CONCURRENCY: usize = 32;

/// Tenant scoping for a Task. Mirrors the
/// `(namespace, tenant)` pair used by every other bus primitive so
/// the state-store key derivation is mechanical.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskScope {
    pub namespace: String,
    pub tenant: String,
}

impl TaskScope {
    /// Construct a scope.
    #[must_use]
    pub fn new(namespace: impl Into<String>, tenant: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            tenant: tenant.into(),
        }
    }

    fn task_key(&self, task_id: &str) -> StateKey {
        StateKey::new(
            self.namespace.clone(),
            self.tenant.clone(),
            KeyKind::A2aTask,
            task_id.to_string(),
        )
    }

    fn dedup_key(&self, message_id: &str) -> StateKey {
        StateKey::new(
            self.namespace.clone(),
            self.tenant.clone(),
            KeyKind::A2aMessageDedup,
            message_id.to_string(),
        )
    }
}

/// A2A Task lifecycle manager.
///
/// Stateless wrt the in-memory cache — every operation hits the
/// state store. Tasks are not on the same per-request hot path as
/// agents/groups; persistence reads are the source of truth.
#[derive(Clone)]
pub struct TaskEngine {
    state: Arc<dyn StateStore>,
    /// Optional audit sink. When set, every successful mutation emits
    /// an A2A task-transition record. `None` disables audit emission
    /// entirely (the engine still functions; transitions just aren't
    /// projected into the audit trail).
    audit: Option<Arc<dyn AuditStore>>,
    /// Optional SSE event broadcast sender. When set, every successful
    /// mutation emits a [`StreamEvent`] with the Task-specific event
    /// type, so an A2A streaming subscriber (e.g. `tasks/resubscribe`)
    /// observes the change. `None` disables stream emission — the
    /// engine still functions; transitions just don't surface to SSE
    /// consumers.
    stream_tx: Option<tokio::sync::broadcast::Sender<acteon_core::StreamEvent>>,
}

impl std::fmt::Debug for TaskEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskEngine").finish_non_exhaustive()
    }
}

impl TaskEngine {
    /// Construct a Task Engine backed by the given state store, with
    /// audit emission disabled. Use [`TaskEngine::with_audit`] to
    /// attach an audit sink.
    #[must_use]
    pub fn new(state: Arc<dyn StateStore>) -> Self {
        Self {
            state,
            audit: None,
            stream_tx: None,
        }
    }

    /// Attach an audit sink so every successful mutation emits an A2A
    /// task-transition record. Pass the same (compliance-decorated)
    /// [`AuditStore`] the gateway uses so task records share the
    /// hash chain with action records.
    #[must_use]
    pub fn with_audit(mut self, audit: Arc<dyn AuditStore>) -> Self {
        self.audit = Some(audit);
        self
    }

    /// Attach an SSE event broadcast sender so every successful
    /// mutation emits an A2A Task stream event. Pass the same channel
    /// the gateway uses (`Gateway::stream_tx()`) so SSE subscribers
    /// share one bus with the rest of the system.
    #[must_use]
    pub fn with_stream_tx(
        mut self,
        tx: tokio::sync::broadcast::Sender<acteon_core::StreamEvent>,
    ) -> Self {
        self.stream_tx = Some(tx);
        self
    }

    /// Emit a best-effort A2A Task stream event. `broadcast::send`
    /// returns `Err` only when there are no subscribers — that's a
    /// no-op for us, never an error to surface. The event carries
    /// `action_type = "a2a.task"` so SSE filtering can route by type;
    /// `action_id` is the task id so a per-task subscriber filters
    /// precisely.
    fn emit_stream(
        &self,
        namespace: &str,
        tenant: &str,
        task_id: &str,
        event_type: acteon_core::StreamEventType,
    ) {
        if let Some(tx) = &self.stream_tx {
            let evt = acteon_core::StreamEvent {
                id: uuid::Uuid::now_v7().to_string(),
                timestamp: Utc::now(),
                event_type,
                namespace: namespace.to_string(),
                tenant: tenant.to_string(),
                action_type: Some("a2a.task".to_string()),
                action_id: Some(task_id.to_string()),
            };
            let _ = tx.send(evt);
        }
    }

    /// Emit a best-effort A2A task-transition audit record. A write
    /// failure is logged, never propagated — the persisted task is the
    /// source of truth and must not be rolled back over an audit miss.
    async fn emit_audit(&self, task: &Task, operation: &str, from_state: Option<TaskState>) {
        let Some(audit) = &self.audit else {
            return;
        };
        let record = build_task_audit_record(task, operation, from_state, Utc::now(), None);
        if let Err(e) = audit.record(record).await {
            warn!(
                error = %e,
                task_id = %task.id,
                operation,
                "A2A task audit write failed"
            );
        }
    }

    /// Create a new task. Fails with [`TaskEngineError::AlreadyExists`]
    /// if a task with the same id already exists in the same scope.
    ///
    /// If the task's history carries `referenceTaskIds`, the reference
    /// graph is walked first — see the `check_reference_graph` method.
    pub async fn create_task(&self, task: Task) -> Result<Task, TaskEngineError> {
        task.validate()?;
        let scope = TaskScope::new(&task.namespace, &task.tenant);
        let seed: Vec<&str> = task
            .history
            .iter()
            .flat_map(|m| m.reference_task_ids.iter().map(String::as_str))
            .collect();
        self.check_reference_graph(&scope, &task.id, &seed).await?;
        let key = scope.task_key(&task.id);
        let payload = serde_json::to_string(&task)?;
        let inserted = self.state.check_and_set(&key, &payload, None).await?;
        if !inserted {
            return Err(TaskEngineError::AlreadyExists(task.id));
        }
        debug!(task_id = %task.id, namespace = %task.namespace, tenant = %task.tenant, "task created");
        self.emit_audit(&task, "create", None).await;
        Ok(task)
    }

    /// Fetch a task by scope + id.
    pub async fn get_task(
        &self,
        scope: &TaskScope,
        task_id: &str,
    ) -> Result<Option<Task>, TaskEngineError> {
        let key = scope.task_key(task_id);
        let Some(raw) = self.state.get(&key).await? else {
            return Ok(None);
        };
        let task: Task = serde_json::from_str(&raw)?;
        Ok(Some(task))
    }

    /// List all tasks in a scope. O(N) scan of `A2aTask` keys for the
    /// tenant — keep result sets bounded by the caller (pagination
    /// lives in the API layer in a follow-up).
    pub async fn list_tasks(&self, scope: &TaskScope) -> Result<Vec<Task>, TaskEngineError> {
        let entries = self
            .state
            .scan_keys(&scope.namespace, &scope.tenant, KeyKind::A2aTask, None)
            .await?;
        let mut out = Vec::with_capacity(entries.len());
        for (_, raw) in entries {
            match serde_json::from_str::<Task>(&raw) {
                Ok(t) => out.push(t),
                Err(e) => {
                    warn!(error = %e, "skipping malformed task row during list");
                }
            }
        }
        Ok(out)
    }

    /// Transition a task to a new state with an optional driving
    /// message. CAS-retries on contention.
    pub async fn transition_task(
        &self,
        scope: &TaskScope,
        task_id: &str,
        next: TaskState,
        message: Option<TaskMessage>,
    ) -> Result<Task, TaskEngineError> {
        let key = scope.task_key(task_id);
        // Capture the pre-mutation state for the SSE emission. This is
        // a small extra read on top of `cas_mutate`'s own; only fired
        // when a stream sink is attached, and only used after a
        // successful transition.
        let from_state = if self.stream_tx.is_some() {
            self.get_task(scope, task_id).await?.map(|t| t.status.state)
        } else {
            None
        };
        let task = self
            .cas_mutate(&key, task_id, "transition", move |task: &mut Task| {
                task.transition_to(next, message.clone())?;
                Ok(())
            })
            .await?;
        if let Some(from) = from_state {
            self.emit_stream(
                &scope.namespace,
                &scope.tenant,
                task_id,
                acteon_core::StreamEventType::TaskTransitioned {
                    task_id: task_id.to_string(),
                    from,
                    to: task.status.state,
                },
            );
        }
        Ok(task)
    }

    /// Transition a task to [`TaskState::Failed`] **iff** it is still
    /// stale at `now` when the fresh row is loaded under CAS.
    ///
    /// This is the stale-task reaper's mutation entry point. A task
    /// that sits in a non-terminal state past its `working_ttl_ms`
    /// without recorded progress is a zombie; failing it makes that
    /// verdict durable rather than only derived on read by
    /// [`Task::is_stale_at`].
    ///
    /// Returns the updated task if it was reaped, or `None` if —
    /// between the reaper's scan and this write — the task recorded
    /// progress or reached a terminal state and is no longer stale.
    /// The staleness re-check runs against the *fresh* CAS-loaded
    /// row, so a task a producer just heartbeated is never failed out
    /// from under it. [`Task::is_stale_at`] already excludes terminal
    /// tasks, and `Failed` is a legal transition from every
    /// non-terminal state, so the transition itself cannot be
    /// rejected.
    pub async fn fail_if_stale(
        &self,
        scope: &TaskScope,
        task_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<Task>, TaskEngineError> {
        let key = scope.task_key(task_id);
        let reason = TaskMessage::text(
            format!("stale-reaper-{}", now.timestamp_millis()),
            TaskRole::Agent,
            "Task failed by the stale-task reaper: no recorded progress within its working TTL.",
        );
        for _ in 0..MAX_CAS_RETRY_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                // Task was deleted between the reaper's scan and now.
                return Ok(None);
            };
            let mut task: Task = serde_json::from_str(&raw)?;
            if !task.is_stale_at(now) {
                // Recorded progress or reached a terminal state since
                // the scan — no longer a zombie, leave it untouched.
                return Ok(None);
            }
            let from_state = task.status.state;
            task.transition_to(TaskState::Failed, Some(reason.clone()))?;
            let payload = serde_json::to_string(&task)?;
            match self
                .state
                .compare_and_swap(&key, version, &payload, None)
                .await?
            {
                CasResult::Ok => {
                    debug!(task_id = %task_id, "stale task reaped to Failed");
                    self.emit_audit(&task, "reap", Some(from_state)).await;
                    self.emit_stream(
                        &scope.namespace,
                        &scope.tenant,
                        task_id,
                        acteon_core::StreamEventType::TaskTransitioned {
                            task_id: task_id.to_string(),
                            from: from_state,
                            to: TaskState::Failed,
                        },
                    );
                    return Ok(Some(task));
                }
                CasResult::Conflict { .. } => {
                    // Lost the race; re-read and re-evaluate staleness.
                }
            }
        }
        Err(TaskEngineError::CasExhausted(task_id.to_string()))
    }

    /// Append a history message to a task. Idempotent on
    /// `message.message_id`: a duplicate submission returns the
    /// already-committed task unmodified.
    ///
    /// Ordering, in four steps:
    ///
    /// 1. Validate the message against the parent task id — catches
    ///    the 1-hop self-cycle cheaply.
    /// 2. Read-only dedup *probe*: if this `messageId` was already
    ///    applied, return the current task immediately. A retry of an
    ///    accepted message must not pay for the reference-graph walk.
    /// 3. Walk the multi-hop reference graph — the expensive step.
    /// 4. Atomically *claim* the dedup marker, then apply the message.
    ///
    /// The marker is written only in step 4, after a successful walk:
    /// a message rejected by the walk never reaches step 4, so it
    /// does not burn its `messageId` — a corrected re-submission with
    /// the same id still applies. The probe (step 2) and the claim
    /// (step 4) are deliberately distinct: the claim's atomic
    /// `check_and_set` is the real idempotency gate under concurrent
    /// identical submissions; the probe is only a fast path that
    /// spares an already-applied retry the cost of the walk.
    pub async fn append_history(
        &self,
        scope: &TaskScope,
        task_id: &str,
        message: TaskMessage,
    ) -> Result<Task, TaskEngineError> {
        message.validate_in_task(task_id)?;
        // Step 2: cheap read-only probe before the expensive walk.
        if self
            .message_already_applied(scope, &message.message_id)
            .await?
        {
            return self
                .get_task(scope, task_id)
                .await?
                .ok_or_else(|| TaskEngineError::NotFound(task_id.to_string()));
        }
        // Step 3: walk the reference graph this message introduces.
        let seed: Vec<&str> = message
            .reference_task_ids
            .iter()
            .map(String::as_str)
            .collect();
        self.check_reference_graph(scope, task_id, &seed).await?;
        // Step 4: claim the dedup marker only now — after a clean
        // walk — then apply. A claim that finds the marker already
        // present lost a race with a concurrent identical submission.
        if self.dedup_message(scope, &message.message_id).await? {
            return self
                .get_task(scope, task_id)
                .await?
                .ok_or_else(|| TaskEngineError::NotFound(task_id.to_string()));
        }
        let key = scope.task_key(task_id);
        // Clone the message id for the post-commit stream emission;
        // `message` itself is consumed by the closure.
        let message_id_for_emit = message.message_id.clone();
        let task = self
            .cas_mutate(&key, task_id, "append_history", move |task: &mut Task| {
                task.append_history(message.clone())?;
                Ok(())
            })
            .await?;
        self.emit_stream(
            &scope.namespace,
            &scope.tenant,
            task_id,
            acteon_core::StreamEventType::TaskHistoryAppended {
                task_id: task_id.to_string(),
                message_id: message_id_for_emit,
            },
        );
        Ok(task)
    }

    /// Apply an artifact-update event from the streaming layer,
    /// CAS-retrying on contention.
    ///
    /// Beyond the per-event structural checks of
    /// [`TaskArtifactUpdateEvent::validate`], this runs the
    /// artifact-stream gatekeeper (`Task::apply_artifact_event`):
    /// the cross-delivery invariants — no updates after a
    /// `lastChunk`, strictly in-order `chunkIndex`, and `totalChunks`
    /// completeness on close — which need the Task's stored
    /// per-artifact stream state and so are enforced inside the CAS
    /// loop.
    pub async fn apply_artifact_update(
        &self,
        scope: &TaskScope,
        event: TaskArtifactUpdateEvent,
    ) -> Result<Task, TaskEngineError> {
        event.validate()?;
        let key = scope.task_key(&event.task_id);
        // `task_id` is cloned out before the closure: `cas_mutate`
        // borrows it for error reporting while the closure moves the
        // whole `event` in to apply it. Capture the artifact id and the
        // `last_chunk` flag here too — they feed the post-commit stream
        // emission and are otherwise lost into the moved `event`.
        let task_id = event.task_id.clone();
        let artifact_id_for_emit = event.artifact.artifact_id.clone();
        let last_chunk_for_emit = event.last_chunk;
        let task = self
            .cas_mutate(&key, &task_id, "artifact_update", move |task: &mut Task| {
                task.apply_artifact_event(&event)?;
                Ok(())
            })
            .await?;
        self.emit_stream(
            &scope.namespace,
            &scope.tenant,
            &task_id,
            acteon_core::StreamEventType::TaskArtifactUpdated {
                task_id: task_id.clone(),
                artifact_id: artifact_id_for_emit,
                last_chunk: last_chunk_for_emit,
            },
        );
        Ok(task)
    }

    /// Record a liveness heartbeat. Bumps `last_progress_at` without
    /// changing state — for long-running tasks that produce no
    /// intermediate output but want to defeat the staleness reaper.
    pub async fn record_progress(
        &self,
        scope: &TaskScope,
        task_id: &str,
    ) -> Result<Task, TaskEngineError> {
        let key = scope.task_key(task_id);
        self.cas_mutate(&key, task_id, "progress", |task: &mut Task| {
            task.record_progress();
            Ok(())
        })
        .await
    }

    /// Stamp a pending approval id on a task — the single-source-of-
    /// truth pointer for "this task is paused awaiting human X." The
    /// caller is responsible for separately transitioning the task to
    /// `InputRequired` or `AuthRequired`.
    pub async fn set_pending_approval(
        &self,
        scope: &TaskScope,
        task_id: &str,
        approval_id: String,
    ) -> Result<Task, TaskEngineError> {
        let key = scope.task_key(task_id);
        self.cas_mutate(&key, task_id, "pending_approval", move |task: &mut Task| {
            task.set_pending_approval(approval_id.clone());
            Ok(())
        })
        .await
    }

    /// Pause a Task on a human and create the [`BusApproval`] row
    /// that represents the pause.
    ///
    /// This is the Task-side entry point for the `BusApproval`
    /// generalization. It performs two writes:
    ///
    /// 1. Persists a fresh task-pause [`BusApproval`] row (status
    ///    `Pending`) at [`KeyKind::BusApproval`], plus a
    ///    [`KeyKind::PendingBusApprovals`] index entry so the row
    ///    appears in `status=pending` listings.
    /// 2. CAS-mutates the Task: transitions it to the state `kind`
    ///    maps to ([`TaskState::AuthRequired`] for
    ///    [`PauseKind::UserAuth`], [`TaskState::InputRequired`] for
    ///    [`PauseKind::UserInput`]) and stamps the approval id onto
    ///    `Task.pending_approval_id`.
    ///
    /// `kind` must be a task-pause kind;
    /// [`PauseKind::OperatorApproval`] returns
    /// [`TaskEngineError::InvalidPauseKind`].
    ///
    /// The transition itself can be rejected: A2A only allows
    /// `AuthRequired` / `InputRequired` from `Working`, so a task
    /// still `Submitted` or already terminal cannot be paused.
    ///
    /// The two writes are not one transaction (separate state keys —
    /// the same trade-off the bus's park flow accepts). The approval
    /// row is written first; if the task mutation then fails — the
    /// task is missing, the transition is illegal, or CAS contention
    /// is exhausted — the orphan approval row and its index entry are
    /// deleted best-effort before the error propagates, so a failed
    /// pause leaves no dangling "waiting on human" record.
    ///
    /// `ttl` defaults to [`DEFAULT_APPROVAL_TTL_MS`] and is clamped to
    /// [`MAX_APPROVAL_TTL_MS`].
    pub async fn pause_for_human(
        &self,
        scope: &TaskScope,
        task_id: &str,
        kind: PauseKind,
        reason: Option<String>,
        ttl: Option<Duration>,
    ) -> Result<(Task, BusApproval), TaskEngineError> {
        // A task-pause kind only — `OperatorApproval` gates a bus
        // tool-call, not a Task, and maps to no task state.
        let Some(target_state) = kind.task_state() else {
            return Err(TaskEngineError::InvalidPauseKind(kind));
        };

        // Resolve + clamp the TTL, then build and validate the row.
        let ttl_ms = ttl
            .map_or(DEFAULT_APPROVAL_TTL_MS, |d| {
                u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
            })
            .min(MAX_APPROVAL_TTL_MS);
        let now = Utc::now();
        let expires_at =
            now + chrono::Duration::milliseconds(i64::try_from(ttl_ms).unwrap_or(i64::MAX));
        let approval_id = uuid::Uuid::now_v7().to_string();
        let approval = BusApproval::new_task_pause(
            &approval_id,
            &scope.namespace,
            &scope.tenant,
            kind,
            task_id,
            reason,
            now,
            expires_at,
        );
        approval.validate()?;

        // Write 1: persist the approval row. `check_and_set` so a
        // (vanishingly unlikely) UUIDv7 collision fails loudly rather
        // than clobbering an existing row.
        let approval_key = StateKey::new(
            scope.namespace.clone(),
            scope.tenant.clone(),
            KeyKind::BusApproval,
            &approval_id,
        );
        let raw = serde_json::to_string(&approval)?;
        if !self.state.check_and_set(&approval_key, &raw, None).await? {
            return Err(TaskEngineError::ApprovalConflict(approval_id.clone()));
        }
        // Pending-approvals index: lets `status=pending` listings scan
        // a smaller keyspace. Best-effort — a failed index write only
        // costs this row its place in pending-filtered listings, which
        // an unfiltered list still recovers.
        let index_key = StateKey::new(
            scope.namespace.clone(),
            scope.tenant.clone(),
            KeyKind::PendingBusApprovals,
            &approval_id,
        );
        if let Err(e) = self.state.set(&index_key, "", None).await {
            warn!(
                approval_id = %approval_id,
                error = %e,
                "failed to write pending-approvals index entry for task pause",
            );
        }

        // Write 2: transition the Task and stamp the approval id —
        // one CAS closure, so both land atomically on the task row.
        let key = scope.task_key(task_id);
        let stamp_id = approval_id.clone();
        let mutated = self
            .cas_mutate(&key, task_id, "pause", move |task: &mut Task| {
                task.transition_to(target_state, None)?;
                task.set_pending_approval(stamp_id.clone());
                Ok(())
            })
            .await;

        match mutated {
            Ok(task) => {
                debug!(
                    task_id = %task_id,
                    approval_id = %approval_id,
                    kind = kind.as_str(),
                    "task paused for human",
                );
                // A successful pause always transitions `Working ->
                // target_state` (the gate inside `transition_to` rejects
                // any other origin). Emit the transition for streaming
                // subscribers; carries `from = Working` unconditionally.
                self.emit_stream(
                    &scope.namespace,
                    &scope.tenant,
                    task_id,
                    acteon_core::StreamEventType::TaskTransitioned {
                        task_id: task_id.to_string(),
                        from: TaskState::Working,
                        to: target_state,
                    },
                );
                Ok((task, approval))
            }
            Err(e) => {
                // The task didn't move — drop the orphan approval row
                // and its index entry so a failed pause leaves no
                // dangling row behind.
                if let Err(del) = self.state.delete(&approval_key).await {
                    warn!(
                        approval_id = %approval_id,
                        error = %del,
                        "failed to delete orphan approval row after task pause failed",
                    );
                }
                let _ = self.state.delete(&index_key).await;
                Err(e)
            }
        }
    }

    /// Set the linked Acteon Chain id on a Task. Pass `Some(chain_id)`
    /// to link the task to an executing chain, or `None` to unlink.
    /// The matching `ChainState.task_id` is set separately by
    /// [`crate::task_chain_bridge::link_task_to_chain`], which
    /// coordinates both sides of the link.
    pub async fn link_to_chain(
        &self,
        scope: &TaskScope,
        task_id: &str,
        chain_id: Option<String>,
    ) -> Result<Task, TaskEngineError> {
        let key = scope.task_key(task_id);
        self.cas_mutate(&key, task_id, "link_chain", move |task: &mut Task| {
            task.chain_id.clone_from(&chain_id);
            Ok(())
        })
        .await
    }

    /// Direct artifact upsert (e.g. from a non-streaming producer).
    /// `apply_artifact_update` is the preferred entry; this is a
    /// convenience for callers that aren't producing wire events.
    pub async fn upsert_artifact(
        &self,
        scope: &TaskScope,
        task_id: &str,
        artifact: Artifact,
        append: bool,
    ) -> Result<Task, TaskEngineError> {
        let key = scope.task_key(task_id);
        // Capture the artifact id before the closure consumes
        // `artifact`. Non-stream callers don't carry a `last_chunk`
        // signal; `false` is the safe default — a subscriber treats it
        // as "more may follow," and a follow-up `apply_artifact_update`
        // is the only path that can flip it to `true`.
        let artifact_id_for_emit = artifact.artifact_id.clone();
        let task = self
            .cas_mutate(&key, task_id, "artifact_upsert", move |task: &mut Task| {
                task.upsert_artifact(artifact.clone(), append)?;
                Ok(())
            })
            .await?;
        self.emit_stream(
            &scope.namespace,
            &scope.tenant,
            task_id,
            acteon_core::StreamEventType::TaskArtifactUpdated {
                task_id: task_id.to_string(),
                artifact_id: artifact_id_for_emit,
                last_chunk: false,
            },
        );
        Ok(task)
    }

    /// Atomically read-modify-write the task at `key` via CAS retry.
    /// The closure is reapplied on each retry against a fresh copy.
    ///
    /// On commit, emits an audit record tagged with `operation` and
    /// the task's pre-mutation state, so the audit trail captures the
    /// `from → to` transition rather than only the resulting state.
    async fn cas_mutate<F>(
        &self,
        key: &StateKey,
        task_id: &str,
        operation: &str,
        mut mutate: F,
    ) -> Result<Task, TaskEngineError>
    where
        F: FnMut(&mut Task) -> Result<(), TaskValidationError>,
    {
        for _ in 0..MAX_CAS_RETRY_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(key).await? else {
                return Err(TaskEngineError::NotFound(task_id.to_string()));
            };
            let mut task: Task = serde_json::from_str(&raw)?;
            let from_state = task.status.state;
            mutate(&mut task)?;
            let payload = serde_json::to_string(&task)?;
            match self
                .state
                .compare_and_swap(key, version, &payload, None)
                .await?
            {
                CasResult::Ok => {
                    debug!(task_id = %task_id, state = ?task.status.state, "task mutated");
                    self.emit_audit(&task, operation, Some(from_state)).await;
                    return Ok(task);
                }
                CasResult::Conflict { .. } => {
                    // Lost the race; re-read and re-apply.
                }
            }
        }
        Err(TaskEngineError::CasExhausted(task_id.to_string()))
    }

    /// Read-only probe: has this `messageId` already been recorded as
    /// applied? Unlike `dedup_message`, this does **not** write the
    /// marker — it only reads it. `append_history` uses the probe to
    /// short-circuit a retry of an already-applied message before the
    /// reference-graph walk, without claiming a marker for a message
    /// that may yet be rejected by that walk.
    async fn message_already_applied(
        &self,
        scope: &TaskScope,
        message_id: &str,
    ) -> Result<bool, TaskEngineError> {
        let key = scope.dedup_key(message_id);
        Ok(self.state.get(&key).await?.is_some())
    }

    /// Mark a `messageId` as seen. Returns `true` if the marker
    /// already existed (i.e. duplicate), `false` if newly inserted.
    ///
    /// Markers are stored with [`MESSAGE_DEDUP_TTL`] so the dedup
    /// keyspace doesn't grow without bound. See the constant's docs
    /// for the late-retry trade-off.
    async fn dedup_message(
        &self,
        scope: &TaskScope,
        message_id: &str,
    ) -> Result<bool, TaskEngineError> {
        let key = scope.dedup_key(message_id);
        let now = Utc::now().timestamp_millis().to_string();
        let inserted = self
            .state
            .check_and_set(&key, &now, Some(MESSAGE_DEDUP_TTL))
            .await?;
        Ok(!inserted)
    }

    /// Breadth-first walk of the Task → Task reference graph rooted at
    /// `root_task_id` (the task being written), starting from
    /// `seed_refs` — the `referenceTaskIds` the pending write
    /// introduces. Rejects three classes of abuse:
    ///
    /// - a path that loops back to `root_task_id`
    ///   ([`TaskEngineError::ReferenceCycle`]),
    /// - a path deeper than [`MAX_REFERENCE_DEPTH`] hops
    ///   ([`TaskEngineError::ReferenceDepthExceeded`]),
    /// - a graph wider than [`MAX_REFERENCE_GRAPH_NODES`] distinct
    ///   referenced tasks ([`TaskEngineError::ReferenceGraphTooLarge`]).
    ///
    /// References to tasks absent from the scope are treated as dead
    /// ends, not errors: a cited task may have been deleted, or may
    /// live in another system. Only structural abuse is refused.
    ///
    /// A pre-existing cycle that does *not* pass through the root is
    /// traversed safely (the visited set prevents re-expansion) and
    /// not re-flagged — this check rejects only what the current
    /// write would introduce.
    ///
    /// Cost: each BFS level's tasks are fetched concurrently with
    /// bounded parallelism, and out-edges are read straight from each
    /// task's history — an id is cloned only when it is a genuinely
    /// new node for the next frontier, never once per reference
    /// entry. The walk therefore allocates O(distinct nodes), not
    /// O(history length × nodes).
    ///
    /// Concurrency boundary: the walk and the subsequent commit are
    /// not one transaction — each Task row has its own CAS and there
    /// is no cross-row lock. Two writers that concurrently add edges
    /// forming a cycle only *together* can each pass their own walk
    /// and both commit. This residual race is accepted; closing it
    /// would need a graph-wide write lock, unjustified for a
    /// structural-abuse guard. Write-time validation is thus
    /// best-effort, not a proof: nothing traverses the reference
    /// graph transitively today (`get_task` is a single-row read,
    /// `list_tasks` a flat scan), so a residual cycle is currently
    /// inert — but any future consumer that walks the graph
    /// transitively MUST carry its own visited set and depth bound
    /// rather than trust this check.
    async fn check_reference_graph(
        &self,
        scope: &TaskScope,
        root_task_id: &str,
        seed_refs: &[&str],
    ) -> Result<(), TaskEngineError> {
        // Initial frontier: the distinct seed references. Deduping
        // before cloning keeps a write whose own history repeats one
        // citation thousands of times from cloning thousands of ids.
        let mut frontier: HashSet<String> = HashSet::new();
        for &r in seed_refs {
            if !frontier.contains(r) {
                frontier.insert(r.to_string());
            }
        }

        // `visited` holds every referenced task already expanded. The
        // root is deliberately never inserted: a reference reaching
        // the root is a cycle and must hit the `tid == root` check
        // below, not be silently dropped as "already visited".
        let mut visited: HashSet<String> = HashSet::new();
        let mut depth = 1usize;

        while !frontier.is_empty() {
            if depth > MAX_REFERENCE_DEPTH {
                return Err(TaskEngineError::ReferenceDepthExceeded {
                    task_id: root_task_id.to_string(),
                });
            }
            // A reference back to the root closes a cycle.
            for tid in &frontier {
                if tid.as_str() == root_task_id {
                    return Err(TaskEngineError::ReferenceCycle {
                        task_id: root_task_id.to_string(),
                    });
                }
            }
            // Each level's frontier is distinct and disjoint from
            // every prior level (the expansion below filters on
            // `visited`), so `visited.len() + frontier.len()` is the
            // exact post-visit node count. Check the fan-out budget
            // before spending any I/O on this level.
            if visited.len() + frontier.len() > MAX_REFERENCE_GRAPH_NODES {
                return Err(TaskEngineError::ReferenceGraphTooLarge {
                    task_id: root_task_id.to_string(),
                });
            }
            // Fetch the whole level concurrently, bounded so a wide
            // frontier cannot open hundreds of simultaneous reads.
            let mut reads = Vec::with_capacity(frontier.len());
            for tid in &frontier {
                let tid = tid.clone();
                reads.push(async move { self.get_task(scope, &tid).await });
            }
            let fetched: Vec<Option<Task>> = stream::iter(reads)
                .buffer_unordered(REFERENCE_GRAPH_FETCH_CONCURRENCY)
                .try_collect()
                .await?;
            // This level is now visited; move (do not clone) its ids.
            visited.extend(frontier);
            // Expand: union the out-edges of every fetched task,
            // reading history directly. Clone an id only when it is a
            // genuinely new node — never once per reference entry.
            // References to absent tasks are dead ends, not errors.
            let mut next: HashSet<String> = HashSet::new();
            for task in fetched.into_iter().flatten() {
                for message in &task.history {
                    for r in &message.reference_task_ids {
                        if !visited.contains(r) && !next.contains(r) {
                            next.insert(r.clone());
                        }
                    }
                }
            }
            frontier = next;
            depth += 1;
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TaskEngineError {
    #[error("task '{0}' already exists")]
    AlreadyExists(String),
    #[error("task '{0}' not found")]
    NotFound(String),
    #[error("state error: {0}")]
    State(#[from] StateError),
    #[error("validation error: {0}")]
    Validation(#[from] TaskValidationError),
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("CAS contention exceeded {MAX_CAS_RETRY_ATTEMPTS} retries for task '{0}'")]
    CasExhausted(String),
    #[error("reference graph for task '{task_id}' contains a cycle back to itself")]
    ReferenceCycle { task_id: String },
    #[error(
        "reference graph for task '{task_id}' is deeper than the {MAX_REFERENCE_DEPTH}-hop limit"
    )]
    ReferenceDepthExceeded { task_id: String },
    #[error(
        "reference graph for task '{task_id}' exceeds the {MAX_REFERENCE_GRAPH_NODES}-node budget"
    )]
    ReferenceGraphTooLarge { task_id: String },
    #[error(
        "pause kind {0:?} is not a task-pause kind — pause_for_human needs UserAuth or UserInput"
    )]
    InvalidPauseKind(PauseKind),
    #[error("approval row for the pause is invalid: {0}")]
    Approval(#[from] BusApprovalValidationError),
    #[error("generated approval id '{0}' collided with an existing row")]
    ApprovalConflict(String),
}

// `PartialEq` for testing assertions. Stringly-compared so error
// variants with non-Eq inner types (StateError, serde_json::Error)
// still work.
impl PartialEq for TaskEngineError {
    fn eq(&self, other: &Self) -> bool {
        self.to_string() == other.to_string()
    }
}

// ---------------------------------------------------------------------
// Convenience: a small wrapper that keeps the (namespace, tenant)
// pair alongside the engine for handler code that has a stable scope
// for many calls. Optional sugar; the bare engine is the contract.
// ---------------------------------------------------------------------

/// A [`TaskEngine`] pre-bound to a [`TaskScope`]. Useful for API
/// handlers that derive scope once from the caller identity and then
/// dispatch many engine calls.
#[derive(Clone, Debug)]
pub struct ScopedTaskEngine {
    engine: TaskEngine,
    scope: TaskScope,
}

impl ScopedTaskEngine {
    /// Bind a Task Engine to a scope.
    #[must_use]
    pub fn new(engine: TaskEngine, scope: TaskScope) -> Self {
        Self { engine, scope }
    }

    /// Underlying engine reference.
    #[must_use]
    pub fn engine(&self) -> &TaskEngine {
        &self.engine
    }

    /// Bound scope.
    #[must_use]
    pub fn scope(&self) -> &TaskScope {
        &self.scope
    }

    pub async fn create(&self, task: Task) -> Result<Task, TaskEngineError> {
        self.engine.create_task(task).await
    }

    pub async fn get(&self, task_id: &str) -> Result<Option<Task>, TaskEngineError> {
        self.engine.get_task(&self.scope, task_id).await
    }

    pub async fn list(&self) -> Result<Vec<Task>, TaskEngineError> {
        self.engine.list_tasks(&self.scope).await
    }

    pub async fn transition(
        &self,
        task_id: &str,
        next: TaskState,
        message: Option<TaskMessage>,
    ) -> Result<Task, TaskEngineError> {
        self.engine
            .transition_task(&self.scope, task_id, next, message)
            .await
    }

    pub async fn append_history(
        &self,
        task_id: &str,
        message: TaskMessage,
    ) -> Result<Task, TaskEngineError> {
        self.engine
            .append_history(&self.scope, task_id, message)
            .await
    }

    pub async fn apply_artifact_update(
        &self,
        event: TaskArtifactUpdateEvent,
    ) -> Result<Task, TaskEngineError> {
        self.engine.apply_artifact_update(&self.scope, event).await
    }

    pub async fn pause_for_human(
        &self,
        task_id: &str,
        kind: PauseKind,
        reason: Option<String>,
        ttl: Option<Duration>,
    ) -> Result<(Task, BusApproval), TaskEngineError> {
        self.engine
            .pause_for_human(&self.scope, task_id, kind, reason, ttl)
            .await
    }

    pub async fn link_to_chain(
        &self,
        task_id: &str,
        chain_id: Option<String>,
    ) -> Result<Task, TaskEngineError> {
        self.engine
            .link_to_chain(&self.scope, task_id, chain_id)
            .await
    }
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_core::{Artifact, TaskPart as Part, TaskRole as Role};
    use acteon_state_memory::MemoryStateStore;

    fn engine() -> TaskEngine {
        TaskEngine::new(Arc::new(MemoryStateStore::new()))
    }

    fn scope() -> TaskScope {
        TaskScope::new("agents", "demo")
    }

    fn sample_task(id: &str) -> Task {
        Task::new(id, "agents", "demo")
    }

    #[tokio::test]
    async fn create_then_get_task() {
        let e = engine();
        e.create_task(sample_task("t1")).await.unwrap();
        let got = e.get_task(&scope(), "t1").await.unwrap().unwrap();
        assert_eq!(got.id, "t1");
        assert_eq!(got.status.state, TaskState::Submitted);
    }

    #[tokio::test]
    async fn create_rejects_duplicate_id() {
        let e = engine();
        e.create_task(sample_task("t1")).await.unwrap();
        let err = e.create_task(sample_task("t1")).await.unwrap_err();
        assert!(matches!(err, TaskEngineError::AlreadyExists(_)));
    }

    #[tokio::test]
    async fn create_validates_payload() {
        let e = engine();
        let mut bad = sample_task("t1");
        bad.id = "bad/id".into();
        let err = e.create_task(bad).await.unwrap_err();
        assert!(matches!(err, TaskEngineError::Validation(_)));
    }

    #[tokio::test]
    async fn get_returns_none_for_missing() {
        assert!(
            engine()
                .get_task(&scope(), "missing")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn list_returns_all_in_scope() {
        let e = engine();
        e.create_task(sample_task("t1")).await.unwrap();
        e.create_task(sample_task("t2")).await.unwrap();
        let all = e.list_tasks(&scope()).await.unwrap();
        assert_eq!(all.len(), 2);
        let mut ids: Vec<_> = all.iter().map(|t| t.id.clone()).collect();
        ids.sort();
        assert_eq!(ids, vec!["t1".to_string(), "t2".to_string()]);
    }

    #[tokio::test]
    async fn transition_persists_new_state() {
        let e = engine();
        e.create_task(sample_task("t1")).await.unwrap();
        let updated = e
            .transition_task(&scope(), "t1", TaskState::Working, None)
            .await
            .unwrap();
        assert_eq!(updated.status.state, TaskState::Working);
        let reloaded = e.get_task(&scope(), "t1").await.unwrap().unwrap();
        assert_eq!(reloaded.status.state, TaskState::Working);
    }

    #[tokio::test]
    async fn transition_rejects_illegal_state() {
        let e = engine();
        e.create_task(sample_task("t1")).await.unwrap();
        // Submitted -> Completed is illegal.
        let err = e
            .transition_task(&scope(), "t1", TaskState::Completed, None)
            .await
            .unwrap_err();
        assert!(matches!(err, TaskEngineError::Validation(_)));
        // State unchanged.
        let reloaded = e.get_task(&scope(), "t1").await.unwrap().unwrap();
        assert_eq!(reloaded.status.state, TaskState::Submitted);
    }

    #[tokio::test]
    async fn transition_on_missing_task_errors() {
        let err = engine()
            .transition_task(&scope(), "missing", TaskState::Working, None)
            .await
            .unwrap_err();
        assert!(matches!(err, TaskEngineError::NotFound(_)));
    }

    #[tokio::test]
    async fn append_history_persists_message() {
        let e = engine();
        e.create_task(sample_task("t1")).await.unwrap();
        let m = TaskMessage::text("m1", Role::User, "hello");
        let updated = e.append_history(&scope(), "t1", m).await.unwrap();
        assert_eq!(updated.history.len(), 1);
    }

    #[tokio::test]
    async fn append_history_is_idempotent_on_message_id() {
        let e = engine();
        e.create_task(sample_task("t1")).await.unwrap();
        let m = TaskMessage::text("m-once", Role::User, "hi");
        e.append_history(&scope(), "t1", m.clone()).await.unwrap();
        // Re-submit with same messageId — should be a no-op.
        let again = e.append_history(&scope(), "t1", m).await.unwrap();
        assert_eq!(again.history.len(), 1);
    }

    #[tokio::test]
    async fn apply_artifact_update_inserts_new_artifact() {
        let e = engine();
        e.create_task(sample_task("t1")).await.unwrap();
        let ev = TaskArtifactUpdateEvent::single_shot(
            "t1",
            Artifact::new("art-1", vec![Part::text("output")]),
        );
        let updated = e.apply_artifact_update(&scope(), ev).await.unwrap();
        assert_eq!(updated.artifacts.len(), 1);
        assert_eq!(updated.artifacts[0].artifact_id, "art-1");
    }

    #[tokio::test]
    async fn apply_artifact_update_appends_on_subsequent_chunk() {
        let e = engine();
        e.create_task(sample_task("t1")).await.unwrap();
        // First chunk: replace (default for chunk_index 0).
        e.apply_artifact_update(
            &scope(),
            TaskArtifactUpdateEvent::chunk(
                "t1",
                Artifact::new("art-1", vec![Part::text("a")]),
                0,
                false,
            ),
        )
        .await
        .unwrap();
        // Second chunk: append.
        let updated = e
            .apply_artifact_update(
                &scope(),
                TaskArtifactUpdateEvent::chunk(
                    "t1",
                    Artifact::new("art-1", vec![Part::text("b")]),
                    1,
                    true,
                ),
            )
            .await
            .unwrap();
        assert_eq!(updated.artifacts.len(), 1);
        assert_eq!(updated.artifacts[0].parts.len(), 2);
    }

    #[tokio::test]
    async fn apply_artifact_update_validates_event() {
        let e = engine();
        e.create_task(sample_task("t1")).await.unwrap();
        let mut bad = TaskArtifactUpdateEvent::single_shot(
            "t1",
            Artifact::new("art-1", vec![Part::text("x")]),
        );
        bad.chunk_index = Some(-1);
        let err = e.apply_artifact_update(&scope(), bad).await.unwrap_err();
        assert!(matches!(err, TaskEngineError::Validation(_)));
    }

    #[tokio::test]
    async fn apply_artifact_update_rejects_update_after_last_chunk() {
        // A lastChunk = true envelope closes the artifact stream; any
        // further update for that artifactId is rejected.
        let e = engine();
        e.create_task(sample_task("t1")).await.unwrap();
        e.apply_artifact_update(
            &scope(),
            TaskArtifactUpdateEvent::chunk(
                "t1",
                Artifact::new("art-1", vec![Part::text("a")]),
                0,
                true,
            ),
        )
        .await
        .unwrap();
        let err = e
            .apply_artifact_update(
                &scope(),
                TaskArtifactUpdateEvent::single_shot(
                    "t1",
                    Artifact::new("art-1", vec![Part::text("b")]),
                ),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, TaskEngineError::Validation(_)));
    }

    #[tokio::test]
    async fn apply_artifact_update_rejects_out_of_order_chunk() {
        // chunkIndex must advance by exactly one; a gap is rejected.
        let e = engine();
        e.create_task(sample_task("t1")).await.unwrap();
        e.apply_artifact_update(
            &scope(),
            TaskArtifactUpdateEvent::chunk(
                "t1",
                Artifact::new("art-1", vec![Part::text("a")]),
                0,
                false,
            ),
        )
        .await
        .unwrap();
        let err = e
            .apply_artifact_update(
                &scope(),
                TaskArtifactUpdateEvent::chunk(
                    "t1",
                    Artifact::new("art-1", vec![Part::text("c")]),
                    2,
                    false,
                ),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, TaskEngineError::Validation(_)));
    }

    #[tokio::test]
    async fn record_progress_bumps_last_progress_at() {
        let e = engine();
        let mut t = sample_task("t1");
        let original = Utc::now() - chrono::Duration::seconds(10);
        t.last_progress_at = Some(original);
        e.create_task(t).await.unwrap();
        let updated = e.record_progress(&scope(), "t1").await.unwrap();
        assert!(updated.last_progress_at.unwrap() > original);
    }

    #[tokio::test]
    async fn set_pending_approval_stamps_id() {
        let e = engine();
        e.create_task(sample_task("t1")).await.unwrap();
        let updated = e
            .set_pending_approval(&scope(), "t1", "appr-42".into())
            .await
            .unwrap();
        assert_eq!(updated.pending_approval_id.as_deref(), Some("appr-42"));
    }

    // Concurrency: four writers race the same Submitted -> Working
    // transition. The CAS loop must serialize them so exactly one
    // commit lands; the losers re-read the now-Working task and fail
    // the transition legitimately (not via CAS exhaustion).
    #[tokio::test]
    async fn cas_retry_handles_concurrent_writers() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let e = TaskEngine::new(store.clone());
        e.create_task(sample_task("t1")).await.unwrap();

        // Spawn 4 concurrent transitions from Submitted -> Working.
        // All but one should error with IllegalTransition (since the
        // first arriver moves to Working and the rest can't repeat),
        // but none should be a NotFound or CasExhausted.
        let mut handles = Vec::new();
        for _ in 0..4 {
            let e2 = e.clone();
            handles.push(tokio::spawn(async move {
                e2.transition_task(&scope(), "t1", TaskState::Working, None)
                    .await
            }));
        }
        let mut ok = 0;
        let mut illegal = 0;
        for h in handles {
            match h.await.unwrap() {
                Ok(_) => ok += 1,
                Err(TaskEngineError::Validation(_)) => illegal += 1,
                Err(e) => panic!("unexpected error: {e}"),
            }
        }
        assert_eq!(ok, 1);
        assert_eq!(illegal, 3);
    }

    // Scoped wrapper sanity check.
    #[tokio::test]
    async fn scoped_engine_dispatch() {
        let e = engine();
        let s = ScopedTaskEngine::new(e, scope());
        s.create(sample_task("t1")).await.unwrap();
        let got = s.get("t1").await.unwrap().unwrap();
        assert_eq!(got.id, "t1");
        s.transition("t1", TaskState::Working, None).await.unwrap();
        assert_eq!(s.list().await.unwrap().len(), 1);
    }

    // --- Reference-graph validation ---

    /// Create a task that optionally references one other task via a
    /// single history message.
    async fn create_with_ref(e: &TaskEngine, id: &str, refs_to: Option<&str>) {
        let mut t = sample_task(id);
        if let Some(r) = refs_to {
            let mut m = TaskMessage::text(format!("{id}-msg"), Role::Agent, "x");
            m.reference_task_ids = vec![r.to_string()];
            t.history.push(m);
        }
        e.create_task(t).await.unwrap();
    }

    #[tokio::test]
    async fn reference_to_existing_task_ok() {
        let e = engine();
        create_with_ref(&e, "target", None).await;
        create_with_ref(&e, "citing", Some("target")).await;
        assert!(e.get_task(&scope(), "citing").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn reference_to_missing_task_is_dead_end_ok() {
        // Citing a task that doesn't exist locally is allowed — it may
        // have been deleted or live in another system. Dead end, not
        // an error.
        let e = engine();
        create_with_ref(&e, "citing", Some("ghost")).await;
        assert!(e.get_task(&scope(), "citing").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn reference_self_loop_rejected_as_validation() {
        // A 1-hop self-reference is caught by `validate_in_task`
        // before the graph walk runs — surfaces as Validation.
        let e = engine();
        create_with_ref(&e, "solo", None).await;
        let mut m = TaskMessage::text("self-msg", Role::Agent, "x");
        m.reference_task_ids = vec!["solo".into()];
        let err = e.append_history(&scope(), "solo", m).await.unwrap_err();
        assert!(matches!(err, TaskEngineError::Validation(_)));
    }

    #[tokio::test]
    async fn reference_direct_cycle_rejected() {
        let e = engine();
        create_with_ref(&e, "a", None).await;
        create_with_ref(&e, "b", Some("a")).await; // b -> a
        // Appending a -> b closes the cycle a -> b -> a.
        let mut m = TaskMessage::text("a-cycle", Role::Agent, "x");
        m.reference_task_ids = vec!["b".into()];
        let err = e.append_history(&scope(), "a", m).await.unwrap_err();
        assert!(matches!(err, TaskEngineError::ReferenceCycle { .. }));
    }

    #[tokio::test]
    async fn reference_depth_at_limit_ok() {
        // Chain exactly MAX_REFERENCE_DEPTH (5) hops from the root.
        let e = engine();
        create_with_ref(&e, "d5", None).await;
        create_with_ref(&e, "d4", Some("d5")).await;
        create_with_ref(&e, "d3", Some("d4")).await;
        create_with_ref(&e, "d2", Some("d3")).await;
        create_with_ref(&e, "d1", Some("d2")).await;
        // root5 -> d1 -> d2 -> d3 -> d4 -> d5 : d5 sits at depth 5.
        create_with_ref(&e, "root5", Some("d1")).await;
        assert!(e.get_task(&scope(), "root5").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn reference_depth_exceeded_rejected() {
        let e = engine();
        create_with_ref(&e, "t6", None).await;
        create_with_ref(&e, "t5", Some("t6")).await;
        create_with_ref(&e, "t4", Some("t5")).await;
        create_with_ref(&e, "t3", Some("t4")).await;
        create_with_ref(&e, "t2", Some("t3")).await;
        create_with_ref(&e, "t1", Some("t2")).await; // chain t1..t6, 5 deep
        // t0 -> t1 pushes t6 to depth 6.
        let mut t0 = sample_task("t0");
        let mut m = TaskMessage::text("t0-msg", Role::Agent, "x");
        m.reference_task_ids = vec!["t1".into()];
        t0.history.push(m);
        let err = e.create_task(t0).await.unwrap_err();
        assert!(matches!(
            err,
            TaskEngineError::ReferenceDepthExceeded { .. }
        ));
    }

    #[tokio::test]
    async fn reference_graph_too_large_rejected() {
        let e = engine();
        let leaf_count = MAX_REFERENCE_GRAPH_NODES + 8;
        for i in 0..leaf_count {
            e.create_task(sample_task(&format!("leaf-{i}")))
                .await
                .unwrap();
        }
        // Root whose history references every leaf, chunked into
        // messages of <= MAX_REFERENCE_TASK_IDS refs each.
        let mut root = sample_task("root");
        let mut leaves: Vec<String> = (0..leaf_count).map(|i| format!("leaf-{i}")).collect();
        let mut idx = 0;
        while !leaves.is_empty() {
            let take = leaves.len().min(acteon_core::MAX_REFERENCE_TASK_IDS);
            let chunk: Vec<String> = leaves.drain(..take).collect();
            let mut m = TaskMessage::text(format!("m{idx}"), Role::Agent, "x");
            m.reference_task_ids = chunk;
            root.history.push(m);
            idx += 1;
        }
        let err = e.create_task(root).await.unwrap_err();
        assert!(matches!(
            err,
            TaskEngineError::ReferenceGraphTooLarge { .. }
        ));
    }

    #[tokio::test]
    async fn failed_graph_check_does_not_consume_dedup_marker() {
        // A graph-check rejection on append must happen *before* the
        // dedup marker is written — otherwise a corrected re-submission
        // with the same messageId would be silently swallowed.
        let e = engine();
        create_with_ref(&e, "x", None).await;
        create_with_ref(&e, "y", Some("x")).await;
        // First attempt: append x -> y closes the cycle x -> y -> x.
        let mut bad = TaskMessage::text("dup-id", Role::Agent, "bad");
        bad.reference_task_ids = vec!["y".into()];
        assert!(e.append_history(&scope(), "x", bad).await.is_err());
        // Re-submit the same messageId without the cycle — must apply.
        let good = TaskMessage::text("dup-id", Role::Agent, "fixed");
        let updated = e.append_history(&scope(), "x", good).await.unwrap();
        assert_eq!(updated.history.len(), 1);
    }

    #[tokio::test]
    async fn reference_walk_follows_edges_across_history_messages() {
        // A task's out-edges are the union over *all* its history
        // messages, not just the first. Build `b` so its only edge to
        // `a` lives in its second message; closing a -> b must still
        // detect the a -> b -> a cycle, which only happens if the
        // walker scans every message of a fetched task.
        let e = engine();
        create_with_ref(&e, "a", None).await;
        let mut b = sample_task("b");
        b.history
            .push(TaskMessage::text("b-m1", Role::Agent, "filler"));
        let mut m2 = TaskMessage::text("b-m2", Role::Agent, "edge");
        m2.reference_task_ids = vec!["a".into()];
        b.history.push(m2);
        e.create_task(b).await.unwrap();
        // Close the cycle: a -> b.
        let mut m = TaskMessage::text("a-edge", Role::Agent, "x");
        m.reference_task_ids = vec!["b".into()];
        let err = e.append_history(&scope(), "a", m).await.unwrap_err();
        assert!(matches!(err, TaskEngineError::ReferenceCycle { .. }));
    }

    #[tokio::test]
    async fn duplicate_message_skips_reference_graph_walk() {
        // Once a messageId is applied, a re-submission is resolved by
        // the read-only dedup probe *before* the reference-graph walk
        // runs. Prove the ordering: re-submit the same id carrying a
        // payload that would fail the walk — the probe short-circuits,
        // so it still succeeds rather than reporting a cycle.
        let e = engine();
        create_with_ref(&e, "p", None).await;
        create_with_ref(&e, "q", Some("p")).await; // q -> p
        // Apply a clean (reference-free) message to p.
        e.append_history(
            &scope(),
            "p",
            TaskMessage::text("same-id", Role::Agent, "clean"),
        )
        .await
        .unwrap();
        // Re-submit the same id, now carrying a cycle-closing p -> q.
        // If the walk ran, this would be a ReferenceCycle; the probe
        // makes it a no-op dedup hit instead.
        let mut replay = TaskMessage::text("same-id", Role::Agent, "would-cycle");
        replay.reference_task_ids = vec!["q".into()];
        let task = e.append_history(&scope(), "p", replay).await.unwrap();
        assert_eq!(task.history.len(), 1);
    }

    #[tokio::test]
    async fn wide_but_bounded_reference_graph_ok() {
        // A fan-out exactly at the node budget must pass — exercises
        // the bounded-concurrency level fetch across several batches
        // and the inclusive `> MAX_REFERENCE_GRAPH_NODES` boundary.
        let e = engine();
        let width = MAX_REFERENCE_GRAPH_NODES;
        for i in 0..width {
            e.create_task(sample_task(&format!("w{i}"))).await.unwrap();
        }
        let mut root = sample_task("wide-root");
        let mut leaves: Vec<String> = (0..width).map(|i| format!("w{i}")).collect();
        let mut idx = 0;
        while !leaves.is_empty() {
            let take = leaves.len().min(acteon_core::MAX_REFERENCE_TASK_IDS);
            let chunk: Vec<String> = leaves.drain(..take).collect();
            let mut m = TaskMessage::text(format!("wm{idx}"), Role::Agent, "x");
            m.reference_task_ids = chunk;
            root.history.push(m);
            idx += 1;
        }
        e.create_task(root).await.unwrap();
        assert!(e.get_task(&scope(), "wide-root").await.unwrap().is_some());
    }

    // --- Stale-task reaper (fail_if_stale) ---

    /// A task whose last progress is `age_secs` in the past with a
    /// `ttl_ms` working TTL — stale once `age > ttl`.
    fn aged_task(id: &str, age_secs: i64, ttl_ms: i64) -> Task {
        let mut t = sample_task(id);
        t.last_progress_at = Some(Utc::now() - chrono::Duration::seconds(age_secs));
        t.working_ttl_ms = ttl_ms;
        t
    }

    #[tokio::test]
    async fn fail_if_stale_reaps_a_stale_task() {
        let e = engine();
        // Submitted, last progress an hour ago, 60s TTL → stale.
        e.create_task(aged_task("zombie", 3600, 60_000))
            .await
            .unwrap();
        let reaped = e
            .fail_if_stale(&scope(), "zombie", Utc::now())
            .await
            .unwrap();
        assert!(reaped.is_some());
        let t = e.get_task(&scope(), "zombie").await.unwrap().unwrap();
        assert_eq!(t.status.state, TaskState::Failed);
        // The failure carries an explanatory status message.
        assert!(t.status.message.is_some());
    }

    #[tokio::test]
    async fn fail_if_stale_leaves_a_fresh_task() {
        let e = engine();
        // Fresh task: last progress is now, well within its TTL.
        e.create_task(sample_task("alive")).await.unwrap();
        let reaped = e
            .fail_if_stale(&scope(), "alive", Utc::now())
            .await
            .unwrap();
        assert!(reaped.is_none());
        let t = e.get_task(&scope(), "alive").await.unwrap().unwrap();
        assert_eq!(t.status.state, TaskState::Submitted);
    }

    #[tokio::test]
    async fn fail_if_stale_leaves_a_terminal_task() {
        // A terminal task is never stale; fail_if_stale must no-op
        // rather than re-transition it.
        let e = engine();
        e.create_task(sample_task("done")).await.unwrap();
        e.transition_task(&scope(), "done", TaskState::Working, None)
            .await
            .unwrap();
        e.transition_task(&scope(), "done", TaskState::Completed, None)
            .await
            .unwrap();
        let reaped = e.fail_if_stale(&scope(), "done", Utc::now()).await.unwrap();
        assert!(reaped.is_none());
        let t = e.get_task(&scope(), "done").await.unwrap().unwrap();
        assert_eq!(t.status.state, TaskState::Completed);
    }

    #[tokio::test]
    async fn fail_if_stale_reaps_a_stale_working_task() {
        // Staleness applies to any non-terminal state. Reap a Working
        // task by evaluating against a `now` far past its TTL.
        let e = engine();
        e.create_task(sample_task("busy")).await.unwrap();
        e.transition_task(&scope(), "busy", TaskState::Working, None)
            .await
            .unwrap();
        let far_future = Utc::now() + chrono::Duration::days(1);
        let reaped = e.fail_if_stale(&scope(), "busy", far_future).await.unwrap();
        assert!(reaped.is_some());
        let t = e.get_task(&scope(), "busy").await.unwrap().unwrap();
        assert_eq!(t.status.state, TaskState::Failed);
    }

    #[tokio::test]
    async fn fail_if_stale_on_missing_task_is_noop() {
        let e = engine();
        let reaped = e
            .fail_if_stale(&scope(), "ghost", Utc::now())
            .await
            .unwrap();
        assert!(reaped.is_none());
    }

    // --- Audit integration ---

    use acteon_audit::store::AuditStore;
    use acteon_audit::{AuditError, AuditPage, AuditQuery, AuditRecord};

    /// In-memory `AuditStore` that just collects every record it is
    /// handed, so a test can assert on what the engine emitted.
    #[derive(Default)]
    struct CollectingAudit {
        records: parking_lot::Mutex<Vec<AuditRecord>>,
    }

    impl CollectingAudit {
        fn records(&self) -> Vec<AuditRecord> {
            self.records.lock().clone()
        }
    }

    #[async_trait::async_trait]
    impl AuditStore for CollectingAudit {
        async fn record(&self, entry: AuditRecord) -> Result<(), AuditError> {
            self.records.lock().push(entry);
            Ok(())
        }
        async fn get_by_action_id(&self, _: &str) -> Result<Option<AuditRecord>, AuditError> {
            Ok(None)
        }
        async fn get_by_id(&self, _: &str) -> Result<Option<AuditRecord>, AuditError> {
            Ok(None)
        }
        async fn query(&self, _: &AuditQuery) -> Result<AuditPage, AuditError> {
            Ok(AuditPage {
                records: Vec::new(),
                total: Some(0),
                limit: 0,
                offset: 0,
                next_cursor: None,
            })
        }
        async fn cleanup_expired(&self) -> Result<u64, AuditError> {
            Ok(0)
        }
    }

    /// An audit store that always fails to record — proves an audit
    /// write failure never fails the underlying mutation.
    struct FailingAudit;

    #[async_trait::async_trait]
    impl AuditStore for FailingAudit {
        async fn record(&self, _: AuditRecord) -> Result<(), AuditError> {
            Err(AuditError::Storage("synthetic audit failure".into()))
        }
        async fn get_by_action_id(&self, _: &str) -> Result<Option<AuditRecord>, AuditError> {
            Ok(None)
        }
        async fn get_by_id(&self, _: &str) -> Result<Option<AuditRecord>, AuditError> {
            Ok(None)
        }
        async fn query(&self, _: &AuditQuery) -> Result<AuditPage, AuditError> {
            Ok(AuditPage {
                records: Vec::new(),
                total: Some(0),
                limit: 0,
                offset: 0,
                next_cursor: None,
            })
        }
        async fn cleanup_expired(&self) -> Result<u64, AuditError> {
            Ok(0)
        }
    }

    fn audited_engine() -> (TaskEngine, Arc<CollectingAudit>) {
        let audit = Arc::new(CollectingAudit::default());
        let engine = TaskEngine::new(Arc::new(MemoryStateStore::new()))
            .with_audit(Arc::clone(&audit) as Arc<dyn AuditStore>);
        (engine, audit)
    }

    #[tokio::test]
    async fn audit_records_task_creation() {
        let (e, audit) = audited_engine();
        e.create_task(sample_task("t1")).await.unwrap();
        let records = audit.records();
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(r.action_id, "t1");
        assert_eq!(r.provider, "a2a");
        assert_eq!(r.action_type, "a2a.task.transition");
        assert_eq!(r.outcome, "submitted");
        assert_eq!(r.outcome_details["operation"], "create");
        // A fresh task has no prior state.
        assert!(r.outcome_details["from_state"].is_null());
        assert_eq!(r.outcome_details["to_state"], "submitted");
    }

    #[tokio::test]
    async fn audit_records_state_transition_with_from_state() {
        let (e, audit) = audited_engine();
        e.create_task(sample_task("t1")).await.unwrap();
        e.transition_task(&scope(), "t1", TaskState::Working, None)
            .await
            .unwrap();
        let records = audit.records();
        assert_eq!(records.len(), 2);
        let t = &records[1];
        assert_eq!(t.outcome_details["operation"], "transition");
        assert_eq!(t.outcome_details["from_state"], "submitted");
        assert_eq!(t.outcome_details["to_state"], "working");
        assert_eq!(t.outcome, "working");
    }

    #[tokio::test]
    async fn audit_records_history_and_artifact_operations() {
        let (e, audit) = audited_engine();
        e.create_task(sample_task("t1")).await.unwrap();
        e.append_history(&scope(), "t1", TaskMessage::text("m1", Role::User, "hi"))
            .await
            .unwrap();
        e.apply_artifact_update(
            &scope(),
            TaskArtifactUpdateEvent::single_shot(
                "t1",
                Artifact::new("art-1", vec![Part::text("out")]),
            ),
        )
        .await
        .unwrap();
        let ops: Vec<String> = audit
            .records()
            .iter()
            .map(|r| r.outcome_details["operation"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(ops, vec!["create", "append_history", "artifact_update"]);
    }

    #[tokio::test]
    async fn audit_records_stale_task_reap() {
        let (e, audit) = audited_engine();
        e.create_task(aged_task("zombie", 3600, 60_000))
            .await
            .unwrap();
        e.fail_if_stale(&scope(), "zombie", Utc::now())
            .await
            .unwrap()
            .expect("task should be reaped");
        let records = audit.records();
        // create + reap.
        assert_eq!(records.len(), 2);
        let reap = &records[1];
        assert_eq!(reap.outcome_details["operation"], "reap");
        assert_eq!(reap.outcome_details["from_state"], "submitted");
        assert_eq!(reap.outcome, "failed");
    }

    #[tokio::test]
    async fn audit_skips_rejected_transition() {
        // An illegal transition errors before the CAS commit, so no
        // audit record is emitted for it.
        let (e, audit) = audited_engine();
        e.create_task(sample_task("t1")).await.unwrap();
        // Submitted -> Completed is illegal.
        e.transition_task(&scope(), "t1", TaskState::Completed, None)
            .await
            .unwrap_err();
        // Only the creation record exists.
        assert_eq!(audit.records().len(), 1);
    }

    #[tokio::test]
    async fn audit_failure_does_not_fail_mutation() {
        // An audit write failure is logged, never propagated — the
        // task transition still succeeds and persists.
        let engine = TaskEngine::new(Arc::new(MemoryStateStore::new()))
            .with_audit(Arc::new(FailingAudit) as Arc<dyn AuditStore>);
        engine.create_task(sample_task("t1")).await.unwrap();
        let updated = engine
            .transition_task(&scope(), "t1", TaskState::Working, None)
            .await
            .unwrap();
        assert_eq!(updated.status.state, TaskState::Working);
    }

    #[tokio::test]
    async fn engine_without_audit_emits_nothing() {
        // The bare engine (no audit sink) still mutates correctly.
        let e = engine();
        e.create_task(sample_task("t1")).await.unwrap();
        e.transition_task(&scope(), "t1", TaskState::Working, None)
            .await
            .unwrap();
        assert!(e.audit.is_none());
    }

    // --- Human-in-the-loop pauses (BusApproval generalization) ---

    use acteon_core::BusApprovalStatus;

    /// Read the persisted approval row for an id straight from the
    /// engine's state store.
    async fn stored_approval(e: &TaskEngine, approval_id: &str) -> Option<BusApproval> {
        let key = StateKey::new("agents", "demo", KeyKind::BusApproval, approval_id);
        let raw = e.state.get(&key).await.unwrap()?;
        Some(serde_json::from_str(&raw).unwrap())
    }

    /// Count the persisted `BusApproval` rows in the test scope.
    async fn approval_row_count(e: &TaskEngine) -> usize {
        e.state
            .scan_keys("agents", "demo", KeyKind::BusApproval, None)
            .await
            .unwrap()
            .len()
    }

    /// Move a freshly-created task to `Working` — the only state A2A
    /// allows `InputRequired` / `AuthRequired` to be entered from.
    async fn working_task(e: &TaskEngine, id: &str) {
        e.create_task(sample_task(id)).await.unwrap();
        e.transition_task(&scope(), id, TaskState::Working, None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn pause_for_human_input_required() {
        let e = engine();
        working_task(&e, "t1").await;
        let (task, approval) = e
            .pause_for_human(
                &scope(),
                "t1",
                PauseKind::UserInput,
                Some("clarify the date".into()),
                None,
            )
            .await
            .unwrap();
        assert_eq!(task.status.state, TaskState::InputRequired);
        assert_eq!(
            task.pending_approval_id.as_deref(),
            Some(approval.approval_id.as_str()),
        );
        assert_eq!(approval.kind, PauseKind::UserInput);
        assert_eq!(approval.task_id.as_deref(), Some("t1"));
        assert_eq!(approval.status, BusApprovalStatus::Pending);
        assert!(approval.envelope.is_none());
        assert!(approval.conversation_id.is_none());
        // The row is persisted, not just returned, and is well-formed.
        let stored = stored_approval(&e, &approval.approval_id).await.unwrap();
        assert_eq!(stored.kind, PauseKind::UserInput);
        stored.validate().unwrap();
    }

    #[tokio::test]
    async fn pause_for_human_auth_required() {
        let e = engine();
        working_task(&e, "t1").await;
        let (task, approval) = e
            .pause_for_human(&scope(), "t1", PauseKind::UserAuth, None, None)
            .await
            .unwrap();
        assert_eq!(task.status.state, TaskState::AuthRequired);
        assert_eq!(approval.kind, PauseKind::UserAuth);
        assert_eq!(approval.task_id.as_deref(), Some("t1"));
    }

    #[tokio::test]
    async fn pause_for_human_writes_pending_index() {
        let e = engine();
        working_task(&e, "t1").await;
        let (_, approval) = e
            .pause_for_human(&scope(), "t1", PauseKind::UserInput, None, None)
            .await
            .unwrap();
        let idx = StateKey::new(
            "agents",
            "demo",
            KeyKind::PendingBusApprovals,
            &approval.approval_id,
        );
        assert!(e.state.get(&idx).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn pause_for_human_rejects_operator_kind() {
        let e = engine();
        working_task(&e, "t1").await;
        let err = e
            .pause_for_human(&scope(), "t1", PauseKind::OperatorApproval, None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, TaskEngineError::InvalidPauseKind(_)));
        // Rejected before any write: the task is untouched and no
        // approval row exists.
        let t = e.get_task(&scope(), "t1").await.unwrap().unwrap();
        assert_eq!(t.status.state, TaskState::Working);
        assert!(t.pending_approval_id.is_none());
        assert_eq!(approval_row_count(&e).await, 0);
    }

    #[tokio::test]
    async fn pause_for_human_missing_task_leaves_no_orphan() {
        let e = engine();
        let err = e
            .pause_for_human(&scope(), "ghost", PauseKind::UserInput, None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, TaskEngineError::NotFound(_)));
        // The approval row written before the failed task mutation
        // was cleaned up.
        assert_eq!(approval_row_count(&e).await, 0);
    }

    #[tokio::test]
    async fn pause_for_human_illegal_transition_leaves_no_orphan() {
        // `Submitted` cannot go straight to `InputRequired`; the pause
        // must fail and clean up the approval row it pre-wrote.
        let e = engine();
        e.create_task(sample_task("t1")).await.unwrap();
        let err = e
            .pause_for_human(&scope(), "t1", PauseKind::UserInput, None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, TaskEngineError::Validation(_)));
        let t = e.get_task(&scope(), "t1").await.unwrap().unwrap();
        assert_eq!(t.status.state, TaskState::Submitted);
        assert!(t.pending_approval_id.is_none());
        assert_eq!(approval_row_count(&e).await, 0);
    }

    #[tokio::test]
    async fn pause_for_human_via_scoped_engine() {
        let e = engine();
        working_task(&e, "t1").await;
        let scoped = ScopedTaskEngine::new(e, scope());
        let (task, approval) = scoped
            .pause_for_human("t1", PauseKind::UserAuth, None, None)
            .await
            .unwrap();
        assert_eq!(task.status.state, TaskState::AuthRequired);
        assert_eq!(approval.kind, PauseKind::UserAuth);
    }

    // --- SSE event emission (Phase 3.2.a) ---

    #[tokio::test]
    async fn transition_emits_a_task_transitioned_event_when_sink_attached() {
        let (tx, mut rx) = tokio::sync::broadcast::channel::<acteon_core::StreamEvent>(16);
        let e = TaskEngine::new(Arc::new(MemoryStateStore::new())).with_stream_tx(tx);
        e.create_task(sample_task("t1")).await.unwrap();
        e.transition_task(&scope(), "t1", TaskState::Working, None)
            .await
            .unwrap();
        let evt = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("emission within the timeout")
            .expect("broadcast receive ok");
        assert_eq!(evt.namespace, "agents");
        assert_eq!(evt.tenant, "demo");
        assert_eq!(evt.action_type.as_deref(), Some("a2a.task"));
        assert_eq!(evt.action_id.as_deref(), Some("t1"));
        match evt.event_type {
            acteon_core::StreamEventType::TaskTransitioned { task_id, from, to } => {
                assert_eq!(task_id, "t1");
                assert_eq!(from, TaskState::Submitted);
                assert_eq!(to, TaskState::Working);
            }
            other => panic!("expected TaskTransitioned, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn append_history_emits_a_task_history_appended_event() {
        let (tx, mut rx) = tokio::sync::broadcast::channel::<acteon_core::StreamEvent>(16);
        let e = TaskEngine::new(Arc::new(MemoryStateStore::new())).with_stream_tx(tx);
        e.create_task(sample_task("t1")).await.unwrap();
        // Drain the create-emission (which is none — create does not emit).
        // First emission comes from the append.
        let msg = user_msg_in_task("m-1", "t1");
        e.append_history(&scope(), "t1", msg).await.unwrap();
        let evt = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("emission within timeout")
            .expect("broadcast ok");
        assert_eq!(evt.action_id.as_deref(), Some("t1"));
        match evt.event_type {
            acteon_core::StreamEventType::TaskHistoryAppended {
                task_id,
                message_id,
            } => {
                assert_eq!(task_id, "t1");
                assert_eq!(message_id, "m-1");
            }
            other => panic!("expected TaskHistoryAppended, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn apply_artifact_update_emits_a_task_artifact_updated_event() {
        let (tx, mut rx) = tokio::sync::broadcast::channel::<acteon_core::StreamEvent>(16);
        let e = TaskEngine::new(Arc::new(MemoryStateStore::new())).with_stream_tx(tx);
        e.create_task(sample_task("t1")).await.unwrap();
        let ev = TaskArtifactUpdateEvent::single_shot(
            "t1",
            Artifact::new("art-1", vec![Part::text("ok")]),
        );
        // `single_shot` flips `last_chunk` to true — assert it carries through.
        e.apply_artifact_update(&scope(), ev).await.unwrap();
        let evt = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("emission within timeout")
            .expect("broadcast ok");
        assert_eq!(evt.action_id.as_deref(), Some("t1"));
        match evt.event_type {
            acteon_core::StreamEventType::TaskArtifactUpdated {
                task_id,
                artifact_id,
                last_chunk,
            } => {
                assert_eq!(task_id, "t1");
                assert_eq!(artifact_id, "art-1");
                assert!(last_chunk, "single_shot must propagate last_chunk = true");
            }
            other => panic!("expected TaskArtifactUpdated, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn upsert_artifact_emits_a_task_artifact_updated_event_with_last_chunk_false() {
        let (tx, mut rx) = tokio::sync::broadcast::channel::<acteon_core::StreamEvent>(16);
        let e = TaskEngine::new(Arc::new(MemoryStateStore::new())).with_stream_tx(tx);
        e.create_task(sample_task("t1")).await.unwrap();
        e.upsert_artifact(
            &scope(),
            "t1",
            Artifact::new("art-9", vec![Part::text("direct")]),
            false,
        )
        .await
        .unwrap();
        let evt = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("emission within timeout")
            .expect("broadcast ok");
        match evt.event_type {
            acteon_core::StreamEventType::TaskArtifactUpdated {
                task_id,
                artifact_id,
                last_chunk,
            } => {
                assert_eq!(task_id, "t1");
                assert_eq!(artifact_id, "art-9");
                assert!(!last_chunk, "upsert path defaults last_chunk = false");
            }
            other => panic!("expected TaskArtifactUpdated, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn pause_for_human_emits_a_task_transitioned_event() {
        let (tx, mut rx) = tokio::sync::broadcast::channel::<acteon_core::StreamEvent>(16);
        let e = TaskEngine::new(Arc::new(MemoryStateStore::new())).with_stream_tx(tx);
        e.create_task(sample_task("t1")).await.unwrap();
        // Drain the Submitted → Working emission from the transition.
        e.transition_task(&scope(), "t1", TaskState::Working, None)
            .await
            .unwrap();
        let _ = rx.recv().await.unwrap();
        // Now pause: emits Working → AuthRequired.
        e.pause_for_human(&scope(), "t1", PauseKind::UserAuth, None, None)
            .await
            .unwrap();
        let evt = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("emission within timeout")
            .expect("broadcast ok");
        match evt.event_type {
            acteon_core::StreamEventType::TaskTransitioned { from, to, .. } => {
                assert_eq!(from, TaskState::Working);
                assert_eq!(to, TaskState::AuthRequired);
            }
            other => panic!("expected TaskTransitioned, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fail_if_stale_emits_a_task_transitioned_to_failed_event() {
        let (tx, mut rx) = tokio::sync::broadcast::channel::<acteon_core::StreamEvent>(16);
        let e = TaskEngine::new(Arc::new(MemoryStateStore::new())).with_stream_tx(tx);
        e.create_task(sample_task("t1")).await.unwrap();
        e.transition_task(&scope(), "t1", TaskState::Working, None)
            .await
            .unwrap();
        // Drain the two earlier emissions (create has none; transition has one).
        let _ = rx.recv().await.unwrap();
        // Default `working_ttl_ms` is 30 minutes; jump an hour ahead to
        // make the task indisputably stale to the reaper.
        let future = Utc::now() + chrono::Duration::hours(1);
        let reaped = e
            .fail_if_stale(&scope(), "t1", future)
            .await
            .unwrap()
            .expect("task should be reaped");
        assert_eq!(reaped.status.state, TaskState::Failed);
        let evt = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("emission within timeout")
            .expect("broadcast ok");
        match evt.event_type {
            acteon_core::StreamEventType::TaskTransitioned { from, to, .. } => {
                assert_eq!(from, TaskState::Working);
                assert_eq!(to, TaskState::Failed);
            }
            other => panic!("expected TaskTransitioned, got {other:?}"),
        }
    }

    /// Build a user message scoped to a parent task — needed because
    /// `append_history` runs `validate_in_task` against the parent id.
    fn user_msg_in_task(message_id: &str, task_id: &str) -> TaskMessage {
        let mut m = TaskMessage::text(message_id.to_string(), acteon_core::TaskRole::User, "hi");
        m.task_id = Some(task_id.to_string());
        m
    }

    #[tokio::test]
    async fn transition_without_sink_is_a_noop() {
        // Without a stream sink, the engine still functions and
        // doesn't pay for the pre-mutation read.
        let e = engine();
        e.create_task(sample_task("t1")).await.unwrap();
        e.transition_task(&scope(), "t1", TaskState::Working, None)
            .await
            .unwrap();
        // Nothing observable to assert beyond "didn't error".
    }

    #[tokio::test]
    async fn transition_no_emission_on_illegal_transition() {
        let (tx, mut rx) = tokio::sync::broadcast::channel::<acteon_core::StreamEvent>(16);
        let e = TaskEngine::new(Arc::new(MemoryStateStore::new())).with_stream_tx(tx);
        e.create_task(sample_task("t1")).await.unwrap();
        // Submitted → InputRequired is illegal (only Working allows it).
        let err = e
            .transition_task(&scope(), "t1", TaskState::InputRequired, None)
            .await
            .unwrap_err();
        assert!(matches!(err, TaskEngineError::Validation(_)));
        // No event emitted for a failed transition.
        let recv = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;
        assert!(
            recv.is_err(),
            "no event should be emitted on failed transition"
        );
    }
}
