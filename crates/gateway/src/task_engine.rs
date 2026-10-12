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
//! task transition is the source of truth. Terminal transitions use a
//! stable receipt ID and are reconciled after an audit-store outage;
//! non-terminal projections remain best-effort.
//!
//! ## Human-in-the-loop pauses
//!
//! [`TaskEngine::pause_for_human`] creates structured-input pauses, while
//! [`TaskEngine::pause_for_authorization`] requires the host's complete
//! verifier binding before creating an authorization pause. Both stamp the
//! matching [`acteon_core::BusApproval`] ID onto `Task.pending_approval_id`.
//! `BusApproval` is the single
//! "waiting on a human" record — the same row type the bus's
//! operator-approval gate uses. The Task transition itself rides the
//! audit integration above.
//!
//! ## Out of scope for this PR (deferred to follow-ups)
//!
//! - **Protocol codecs**: A2A JSON-RPC 2.0 + REST framing lives in
//!   the server layer, not here. The engine speaks pure Rust types.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::stream::{self, StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, warn};

use acteon_audit::{AuditError, store::AuditStore};
use acteon_core::{
    Artifact, BusApproval, BusApprovalStatus, BusApprovalValidationError, DEFAULT_APPROVAL_TTL_MS,
    DlqFailureKind, MAX_APPROVAL_TTL_MS, MAX_DLQ_ERROR_BYTES, MAX_DLQ_EVENT_BYTES,
    MAX_REFERENCE_DEPTH, PauseKind, PushDeliveryDlqEntry, Task, TaskArtifactUpdateEvent,
    TaskAuthorizationRequirement, TaskAuthorizationResolution, TaskMessage, TaskPauseResolution,
    TaskPushConfigValidationError, TaskPushNotificationConfig, TaskRole, TaskState,
    TaskValidationError,
};
use acteon_state::{CasResult, KeyKind, StateError, StateKey, StateStore};

use crate::audit_helpers::{
    build_task_audit_record, build_task_push_config_audit_record, build_task_terminal_audit_record,
    task_terminal_audit_id,
};
use crate::task_authorization::{
    TaskAuthorizationVerification, TaskAuthorizationVerificationError, TaskAuthorizationVerifier,
    VerifiedTaskAuthorization,
};

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

/// Current private persistence format for an A2A task row.
const DURABLE_TASK_RECORD_VERSION: u32 = 1;

/// Maximum unacknowledged callback events retained in one task row. The task
/// mutation fails closed at this boundary while any registered callback still
/// needs the oldest event; tasks without callbacks continuously prune through
/// their previous tail.
pub const MAX_DURABLE_TASK_EVENTS: usize = 256;

/// Maximum callback configurations embedded in one Task envelope. Each
/// configuration can carry bounded URL and credential material, so this cap
/// keeps one CAS value bounded across every state backend.
pub const MAX_DURABLE_TASK_CALLBACKS: usize = 64;

/// Maximum dead-lettered callback events retained inside one Task envelope.
pub const MAX_DURABLE_TASK_DEAD_LETTERS: usize = 64;

/// Upper bound for one callback delivery lease. Workers renew by claiming
/// again only after expiry; keeping leases short bounds takeover latency.
pub const MAX_TASK_PUSH_LEASE: Duration = Duration::from_secs(5 * 60);

/// Backend-neutral task envelope. The public A2A [`Task`] remains the wire
/// projection; this record is private storage state that can grow delivery
/// cursors, leases, and an ordered mutation log without changing that API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DurableTaskRecord {
    record_version: u32,
    task: Task,
    #[serde(default)]
    delivery: DurableTaskDelivery,
}

/// Reserved delivery state for the ordered outbox phases. Keeping it in the
/// v1 envelope now proves that every task mutation preserves private state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DurableTaskDelivery {
    /// True after standalone `A2aTaskPushConfig` rows have been examined and
    /// imported. New tasks set this immediately; legacy task rows begin false.
    #[serde(default)]
    legacy_config_scan_complete: bool,
    #[serde(default)]
    next_sequence: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    events: Vec<DurableTaskEvent>,
    /// Governed callback configurations and their ordered acknowledgement
    /// cursors. Configuration identity and delivery progress share the Task CAS.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    callbacks: BTreeMap<String, DurableTaskCallback>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    dead_letters: Vec<PushDeliveryDlqEntry>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DurableTaskCallback {
    config: TaskPushNotificationConfig,
    acknowledged_sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lease: Option<DurableTaskDeliveryLease>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_attempt_at: Option<DateTime<Utc>>,
    #[serde(default)]
    attempts: u32,
    #[serde(default)]
    blocked: bool,
}

impl DurableTaskCallback {
    fn new(config: TaskPushNotificationConfig, acknowledged_sequence: u64) -> Self {
        Self {
            config,
            acknowledged_sequence,
            lease: None,
            next_attempt_at: None,
            attempts: 0,
            blocked: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DurableTaskDeliveryLease {
    lease_id: String,
    worker_id: String,
    sequence: u64,
    event_id: String,
    attempt: u32,
    claimed_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

/// Fenced ownership of one callback's next ordered event. The lease ID must be
/// presented to acknowledge or fail the delivery.
#[derive(Clone)]
pub struct TaskPushDeliveryClaim {
    pub task_id: String,
    pub config: TaskPushNotificationConfig,
    pub sequence: u64,
    pub event_id: String,
    pub occurred_at: DateTime<Utc>,
    pub payload: serde_json::Value,
    pub attempt: u32,
    pub lease_id: String,
    pub lease_expires_at: DateTime<Utc>,
}

/// Repairable state-backend wake-up hint for a Task with callback work. The
/// Task envelope remains authoritative; stale or missing hints are safe and
/// are corrected by bounded Task-page repair.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskPushReadyHint {
    pub namespace: String,
    pub tenant: String,
    pub task_id: String,
    pub ready_at: DateTime<Utc>,
}

/// Operator disposition for a callback event parked in durable dead-letter
/// state. Retry preserves the callback cursor; discard advances it exactly
/// once so later events may continue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushDeadLetterResolution {
    Retry,
    Discard,
}

impl std::fmt::Debug for DurableTaskCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DurableTaskCallback")
            .field("config", &self.config)
            .field("acknowledged_sequence", &self.acknowledged_sequence)
            .field("lease", &self.lease)
            .field("next_attempt_at", &self.next_attempt_at)
            .field("attempts", &self.attempts)
            .field("blocked", &self.blocked)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DurableTaskEvent {
    sequence: u64,
    event_id: String,
    occurred_at: DateTime<Utc>,
    payload: serde_json::Value,
}

impl DurableTaskDelivery {
    fn ready_at(&self, immediate_at: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.callbacks
            .values()
            .filter_map(|callback| {
                if callback.blocked
                    || !self.events.iter().any(|event| {
                        event.sequence == callback.acknowledged_sequence.saturating_add(1)
                    })
                {
                    return None;
                }
                Some(
                    callback
                        .lease
                        .as_ref()
                        .map(|lease| lease.expires_at)
                        .or(callback.next_attempt_at)
                        .unwrap_or(immediate_at),
                )
            })
            .min()
    }

    fn validate(&self, task: &Task) -> Result<(), TaskEngineError> {
        if self.callbacks.len() > MAX_DURABLE_TASK_CALLBACKS {
            return Err(TaskEngineError::InvalidDurableRecord(
                "callback configuration capacity exceeded".into(),
            ));
        }
        if self.dead_letters.len() > MAX_DURABLE_TASK_DEAD_LETTERS {
            return Err(TaskEngineError::InvalidDurableRecord(
                "callback dead-letter capacity exceeded".into(),
            ));
        }
        if self.events.len() > MAX_DURABLE_TASK_EVENTS {
            return Err(TaskEngineError::InvalidDurableRecord(
                "delivery event capacity exceeded".into(),
            ));
        }
        let mut previous = None;
        let mut ids = HashSet::with_capacity(self.events.len());
        for event in &self.events {
            if event.sequence == 0
                || event.sequence > self.next_sequence
                || previous.is_some_and(|value| event.sequence != value + 1)
                || !ids.insert(event.event_id.as_str())
                || uuid::Uuid::parse_str(&event.event_id).is_err()
                || event.occurred_at > task.updated_at
                || !valid_push_payload(&event.payload, &task.id, task.context_id.as_deref())
            {
                return Err(TaskEngineError::InvalidDurableRecord(
                    "invalid ordered delivery event log".into(),
                ));
            }
            previous = Some(event.sequence);
        }
        for (id, callback) in &self.callbacks {
            if id != &callback.config.id
                || callback.config.task_id != task.id
                || callback.config.namespace != task.namespace
                || callback.config.tenant != task.tenant
                || callback.config.validate().is_err()
                || callback.acknowledged_sequence > self.next_sequence
            {
                return Err(TaskEngineError::InvalidDurableRecord(
                    "invalid callback configuration or cursor".into(),
                ));
            }
            if callback.blocked && (callback.lease.is_some() || callback.next_attempt_at.is_some())
                || callback.lease.as_ref().is_some_and(|lease| {
                    lease.sequence != callback.acknowledged_sequence.saturating_add(1)
                        || lease.attempt != callback.attempts
                        || lease.claimed_at >= lease.expires_at
                        || uuid::Uuid::parse_str(&lease.lease_id).is_err()
                        || lease.worker_id.is_empty()
                        || !self.events.iter().any(|event| {
                            event.sequence == lease.sequence && event.event_id == lease.event_id
                        })
                })
                || callback.lease.is_some() && callback.next_attempt_at.is_some()
            {
                return Err(TaskEngineError::InvalidDurableRecord(
                    "invalid callback lease or retry state".into(),
                ));
            }
        }
        self.validate_dead_letters(task)?;
        if let Some(first) = self.events.first()
            && self
                .callbacks
                .values()
                .any(|callback| callback.acknowledged_sequence.saturating_add(1) < first.sequence)
        {
            return Err(TaskEngineError::InvalidDurableRecord(
                "delivery log skipped an unacknowledged callback event".into(),
            ));
        }
        if self.events.is_empty()
            && self
                .callbacks
                .values()
                .any(|callback| callback.acknowledged_sequence != self.next_sequence)
        {
            return Err(TaskEngineError::InvalidDurableRecord(
                "delivery log omitted an unacknowledged callback event".into(),
            ));
        }
        Ok(())
    }

    fn validate_dead_letters(&self, task: &Task) -> Result<(), TaskEngineError> {
        let mut ids = HashSet::with_capacity(self.dead_letters.len());
        for entry in &self.dead_letters {
            let valid_delivery_identity = match (entry.sequence, entry.event_id.as_deref()) {
                (Some(sequence), Some(event_id)) => {
                    sequence > 0 && uuid::Uuid::parse_str(event_id).is_ok()
                }
                (None, None) => true,
                _ => false,
            };
            if entry.task_id != task.id
                || entry.namespace != task.namespace
                || entry.tenant != task.tenant
                || uuid::Uuid::parse_str(&entry.id).is_err()
                || !ids.insert(entry.id.as_str())
                || !valid_delivery_identity
            {
                return Err(TaskEngineError::InvalidDurableRecord(
                    "invalid callback dead letter".into(),
                ));
            }
        }
        if self.callbacks.iter().any(|(config_id, callback)| {
            callback.blocked
                && !self.dead_letters.iter().any(|entry| {
                    entry.config_id == *config_id
                        && entry.sequence == callback.acknowledged_sequence.checked_add(1)
                        && entry.event_id.as_ref().is_some_and(|event_id| {
                            self.events.iter().any(|event| {
                                Some(event.sequence) == entry.sequence
                                    && event.event_id == *event_id
                            })
                        })
                })
        }) {
            return Err(TaskEngineError::InvalidDurableRecord(
                "blocked callback has no matching dead letter".into(),
            ));
        }
        Ok(())
    }

    fn append(
        &mut self,
        event_id: String,
        occurred_at: DateTime<Utc>,
        payload: serde_json::Value,
    ) -> Result<(), TaskEngineError> {
        let acknowledged = self
            .callbacks
            .values()
            .map(|callback| callback.acknowledged_sequence)
            .min()
            .unwrap_or(self.next_sequence);
        self.events.retain(|event| event.sequence > acknowledged);
        if self.events.len() >= MAX_DURABLE_TASK_EVENTS {
            return Err(TaskEngineError::DeliveryLogFull);
        }
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or_else(|| TaskEngineError::InvalidDurableRecord("sequence exhausted".into()))?;
        self.events.push(DurableTaskEvent {
            sequence: self.next_sequence,
            event_id,
            occurred_at,
            payload,
        });
        Ok(())
    }

    fn prune_acknowledged(&mut self) {
        let acknowledged = self
            .callbacks
            .values()
            .map(|callback| callback.acknowledged_sequence)
            .min()
            .unwrap_or(self.next_sequence);
        self.events.retain(|event| event.sequence > acknowledged);
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum PersistedPushPayload {
    Status {
        #[serde(rename = "statusUpdate")]
        update: PersistedStatusUpdate,
    },
    Artifact {
        #[serde(rename = "artifactUpdate")]
        update: PersistedArtifactUpdate,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedStatusUpdate {
    task_id: String,
    #[serde(default)]
    context_id: Option<String>,
    status: PersistedProtocolStatus,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedProtocolStatus {
    state: PersistedProtocolTaskState,
    #[serde(default)]
    message: Option<serde_json::Value>,
    timestamp: DateTime<Utc>,
}

#[derive(Deserialize)]
enum PersistedProtocolTaskState {
    #[serde(rename = "TASK_STATE_SUBMITTED")]
    Submitted,
    #[serde(rename = "TASK_STATE_WORKING")]
    Working,
    #[serde(rename = "TASK_STATE_COMPLETED")]
    Completed,
    #[serde(rename = "TASK_STATE_FAILED")]
    Failed,
    #[serde(rename = "TASK_STATE_CANCELED")]
    Canceled,
    #[serde(rename = "TASK_STATE_INPUT_REQUIRED")]
    InputRequired,
    #[serde(rename = "TASK_STATE_AUTH_REQUIRED")]
    AuthRequired,
    #[serde(rename = "TASK_STATE_REJECTED")]
    Rejected,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedArtifactUpdate {
    task_id: String,
    #[serde(default)]
    context_id: Option<String>,
    artifact: Artifact,
    append: bool,
    last_chunk: bool,
}

fn valid_push_payload(
    payload: &serde_json::Value,
    expected_task_id: &str,
    expected_context_id: Option<&str>,
) -> bool {
    match serde_json::from_value::<PersistedPushPayload>(payload.clone()) {
        Ok(PersistedPushPayload::Status { update }) => {
            update.task_id == expected_task_id
                && update.context_id.as_deref() == expected_context_id
                && update
                    .status
                    .message
                    .as_ref()
                    .is_none_or(serde_json::Value::is_object)
                && {
                    let _ = (&update.status.state, update.status.timestamp);
                    true
                }
        }
        Ok(PersistedPushPayload::Artifact { update }) => {
            update.task_id == expected_task_id
                && update.context_id.as_deref() == expected_context_id
                && update.artifact.validate().is_ok()
                && !update.append
                && {
                    let _ = update.last_chunk;
                    true
                }
        }
        Err(_) => false,
    }
}

/// Project one exact A2A 1.0 status-update callback payload. Both the durable
/// mutation log and the live compatibility worker use this function, keeping
/// delayed delivery and replay on the same wire representation.
pub fn project_task_status_push_payload(
    task: &Task,
    state: TaskState,
    timestamp: DateTime<Utc>,
) -> Result<serde_json::Value, TaskEngineError> {
    let mut status = task.status.clone();
    if status.state != state {
        status.state = state;
        status.message = None;
        status.timestamp = timestamp;
    }
    let mut status = serde_json::to_value(&status)?;
    let object = status.as_object_mut().ok_or_else(|| {
        TaskEngineError::InvalidDurableRecord("task status is not an object".into())
    })?;
    if let Some(state) = object.get_mut("state")
        && let Some(name) = state.as_str()
    {
        *state = serde_json::Value::String(format!("TASK_STATE_{}", name.to_ascii_uppercase()));
    }
    if let Some(message) = object
        .get_mut("message")
        .and_then(serde_json::Value::as_object_mut)
        && let Some(role) = message.get_mut("role")
        && let Some(name) = role.as_str()
    {
        *role = serde_json::Value::String(format!("ROLE_{}", name.to_ascii_uppercase()));
    }
    let mut update = serde_json::json!({
        "taskId": task.id,
        "status": status,
    });
    if let Some(context_id) = &task.context_id {
        update["contextId"] = serde_json::Value::String(context_id.clone());
    }
    Ok(serde_json::json!({"statusUpdate": update}))
}

/// Project one exact A2A 1.0 artifact-update callback payload from the durable
/// resolved artifact snapshot.
pub fn project_task_artifact_push_payload(
    task: &Task,
    artifact_id: &str,
    last_chunk: bool,
) -> Result<serde_json::Value, TaskEngineError> {
    let artifact = task
        .artifacts
        .iter()
        .find(|artifact| artifact.artifact_id == artifact_id)
        .ok_or_else(|| {
            TaskEngineError::InvalidDurableRecord(format!(
                "mutated artifact '{artifact_id}' is absent"
            ))
        })?;
    let mut update = serde_json::json!({
        "taskId": task.id,
        "artifact": artifact,
        "append": false,
        "lastChunk": last_chunk,
    });
    if let Some(context_id) = &task.context_id {
        update["contextId"] = serde_json::Value::String(context_id.clone());
    }
    Ok(serde_json::json!({"artifactUpdate": update}))
}

impl DurableTaskRecord {
    fn new(task: Task) -> Self {
        Self {
            record_version: DURABLE_TASK_RECORD_VERSION,
            task,
            delivery: DurableTaskDelivery {
                legacy_config_scan_complete: true,
                ..DurableTaskDelivery::default()
            },
        }
    }

    fn from_legacy(task: Task) -> Self {
        Self {
            record_version: DURABLE_TASK_RECORD_VERSION,
            task,
            delivery: DurableTaskDelivery::default(),
        }
    }

    fn decode(raw: &str) -> Result<Self, TaskEngineError> {
        let value: serde_json::Value = serde_json::from_str(raw)?;
        if value.get("recordVersion").is_none() {
            // Rows written before the envelope rollout remain readable and
            // migrate atomically on their next successful task mutation.
            return Ok(Self::from_legacy(serde_json::from_value(value)?));
        }

        let record: Self = serde_json::from_value(value)?;
        if record.record_version != DURABLE_TASK_RECORD_VERSION {
            return Err(TaskEngineError::UnsupportedRecordVersion(
                record.record_version,
            ));
        }
        record.delivery.validate(&record.task)?;
        Ok(record)
    }

    fn encode(&self) -> Result<String, TaskEngineError> {
        Ok(serde_json::to_string(self)?)
    }

    fn ready_hint(&self) -> Option<TaskPushReadyHint> {
        self.delivery
            .ready_at(self.task.updated_at)
            .map(|ready_at| TaskPushReadyHint {
                namespace: self.task.namespace.clone(),
                tenant: self.task.tenant.clone(),
                task_id: self.task.id.clone(),
                ready_at,
            })
    }
}

pub(crate) fn decode_persisted_task(raw: &str) -> Result<Task, TaskEngineError> {
    Ok(DurableTaskRecord::decode(raw)?.task)
}

fn canonical_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let sorted = map
                .iter()
                .map(|(key, value)| (key.clone(), canonical_json(value)))
                .collect();
            serde_json::Value::Object(sorted)
        }
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(canonical_json).collect())
        }
        scalar => scalar.clone(),
    }
}

fn task_message_digest(message: &TaskMessage) -> Result<String, serde_json::Error> {
    let value = canonical_json(&serde_json::to_value(message)?);
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(&value)?)))
}

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

    fn push_ready_key(&self, task_id: &str) -> StateKey {
        StateKey::new(
            self.namespace.clone(),
            self.tenant.clone(),
            KeyKind::A2aPushReady,
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
    legacy_only: bool,
    state: Arc<dyn StateStore>,
    clock: Arc<dyn acteon_time::Clock>,
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
            clock: Arc::new(acteon_time::SystemClock::default()),
            audit: None,
            stream_tx: None,
            legacy_only: false,
        }
    }

    async fn sync_push_ready_hint(
        &self,
        record: &DurableTaskRecord,
    ) -> Result<Option<TaskPushReadyHint>, TaskEngineError> {
        let scope = TaskScope::new(&record.task.namespace, &record.task.tenant);
        let key = scope.push_ready_key(&record.task.id);
        let hint = record.ready_hint();
        if let Some(hint) = &hint {
            self.state
                .set(&key, &serde_json::to_string(hint)?, None)
                .await?;
        } else {
            self.state.delete(&key).await?;
        }
        Ok(hint)
    }

    async fn sync_push_ready_hint_best_effort(&self, record: &DurableTaskRecord) {
        if let Err(error) = self.sync_push_ready_hint(record).await {
            warn!(
                %error,
                task_id = %record.task.id,
                "A2A callback ready-index update deferred to repair"
            );
        }
    }

    /// Rebuild one Task's callback-ready hint from its authoritative envelope.
    /// Workers call this while walking bounded Task pages, so a missed hint
    /// write can delay work only until the next repair pass.
    pub async fn repair_push_ready_hint(
        &self,
        scope: &TaskScope,
        task_id: &str,
    ) -> Result<Option<TaskPushReadyHint>, TaskEngineError> {
        let Some(raw) = self.state.get(&scope.task_key(task_id)).await? else {
            self.state.delete(&scope.push_ready_key(task_id)).await?;
            return Ok(None);
        };
        let record = DurableTaskRecord::decode(&raw)?;
        if record.task.namespace != scope.namespace
            || record.task.tenant != scope.tenant
            || record.task.id != task_id
        {
            return Err(TaskEngineError::InvalidDurableRecord(
                "A2A Task row identity does not match its ready-index repair key".into(),
            ));
        }
        self.sync_push_ready_hint(&record).await
    }

    /// Restrict a tenant-level legacy protocol adapter to legacy tasks. Governed
    /// services require their own original-requester authorization and lifecycle.
    #[must_use]
    pub fn with_legacy_task_access(mut self) -> Self {
        self.legacy_only = true;
        self
    }

    async fn hidden_governed_task(
        &self,
        scope: &TaskScope,
        id: &str,
    ) -> Result<bool, TaskEngineError> {
        Ok(self.legacy_only
            && crate::agent_runtime::is_governed_agent_task(
                self.state.as_ref(),
                &scope.namespace,
                &scope.tenant,
                id,
            )
            .await?)
    }

    /// Use the same clock as the gateway and state store.
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn acteon_time::Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Clock for task construction and bridge retry waits.
    #[must_use]
    pub fn clock(&self) -> Arc<dyn acteon_time::Clock> {
        Arc::clone(&self.clock)
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
                timestamp: self.clock.now(),
                event_type,
                namespace: namespace.to_string(),
                tenant: tenant.to_string(),
                action_type: Some("a2a.task".to_string()),
                action_id: Some(task_id.to_string()),
            };
            let _ = tx.send(evt);
        }
    }

    /// Emit an A2A task-transition audit record. A write failure is logged,
    /// never propagated — the persisted task is the source of truth and must
    /// not be rolled back over an audit miss. Terminal transitions use a
    /// stable receipt so reconciliation can finish a missed audit later.
    async fn emit_audit(&self, task: &Task, operation: &str, from_state: Option<TaskState>) {
        let Some(audit) = &self.audit else {
            return;
        };
        let terminal_transition =
            from_state.is_some_and(|from| !from.is_terminal() && task.status.state.is_terminal());
        if terminal_transition {
            let record = build_task_terminal_audit_record(
                task,
                operation,
                from_state,
                task.status.timestamp,
            );
            match audit.get_by_id(&record.id).await {
                Ok(Some(_)) => {}
                Ok(None) => {
                    if let Err(error) = audit.record(record).await {
                        warn!(
                            %error,
                            task_id = %task.id,
                            operation,
                            "terminal A2A task audit retained for recovery"
                        );
                    }
                }
                Err(error) => {
                    warn!(
                        %error,
                        task_id = %task.id,
                        operation,
                        "terminal A2A task audit receipt check failed"
                    );
                }
            }
        } else {
            let record =
                build_task_audit_record(task, operation, from_state, self.clock.now(), None);
            if let Err(error) = audit.record(record).await {
                warn!(
                    %error,
                    task_id = %task.id,
                    operation,
                    "A2A task audit write failed"
                );
            }
        }
    }

    /// Emit credential-free evidence for a callback configuration mutation.
    /// Audit failure cannot roll back the already-committed Task CAS.
    async fn emit_push_config_audit(&self, task: &Task, operation: &str, config_id: &str) {
        let Some(audit) = &self.audit else {
            return;
        };
        let record =
            build_task_push_config_audit_record(task, operation, config_id, self.clock.now());
        if let Err(error) = audit.record(record).await {
            warn!(
                %error,
                task_id = %task.id,
                config_id,
                operation,
                "A2A push-config audit write failed"
            );
        }
    }

    /// Replay terminal task audits whose stable receipt is absent.
    ///
    /// The task row is authoritative: once a task has reached a terminal
    /// state, its status timestamp and final state are sufficient to recreate
    /// the audit record after an unavailable audit backend or restart. A
    /// successful receipt is never duplicated because the audit ID is stable.
    pub(crate) async fn reconcile_terminal_audits(&self) -> Result<usize, TaskEngineError> {
        let Some(audit) = &self.audit else {
            return Ok(0);
        };

        let entries = self.state.scan_keys_by_kind(KeyKind::A2aTask).await?;
        let mut replayed = 0;
        let mut first_error = None;
        for (_, raw) in entries {
            let task = match decode_persisted_task(&raw) {
                Ok(task) => task,
                Err(error) => {
                    warn!(%error, "skipping malformed task row during terminal audit recovery");
                    continue;
                }
            };
            if !task.status.state.is_terminal() {
                continue;
            }

            let receipt_id = task_terminal_audit_id(&task);
            match audit.get_by_id(&receipt_id).await {
                Ok(Some(_)) => {}
                Ok(None) => {
                    let record = build_task_terminal_audit_record(
                        &task,
                        "terminal_recovery",
                        None,
                        task.status.timestamp,
                    );
                    match audit.record(record).await {
                        Ok(()) => replayed += 1,
                        Err(error) => {
                            first_error.get_or_insert(error);
                        }
                    }
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }

        match first_error {
            Some(error) => Err(TaskEngineError::Audit(error)),
            None => Ok(replayed),
        }
    }

    /// Create a new task. Fails with [`TaskEngineError::AlreadyExists`]
    /// if a task with the same id already exists in the same scope.
    ///
    /// If the task's history carries `referenceTaskIds`, the reference
    /// graph is walked first — see the `check_reference_graph` method.
    pub async fn create_task(&self, task: Task) -> Result<Task, TaskEngineError> {
        self.create_task_with_push_configs(task, Vec::new()).await
    }

    /// Create a Task and its initial push configurations in one state-store
    /// insertion. Inline A2A admission uses this boundary so a runnable event
    /// can never commit before its callback cursor exists.
    pub async fn create_task_with_push_configs(
        &self,
        task: Task,
        configs: Vec<TaskPushNotificationConfig>,
    ) -> Result<Task, TaskEngineError> {
        task.validate()?;
        let scope = TaskScope::new(&task.namespace, &task.tenant);
        if self.hidden_governed_task(&scope, &task.id).await? {
            return Err(TaskEngineError::NotFound(task.id));
        }
        let seed: Vec<&str> = task
            .history
            .iter()
            .flat_map(|m| m.reference_task_ids.iter().map(String::as_str))
            .collect();
        self.check_reference_graph(&scope, &task.id, &seed).await?;
        let key = scope.task_key(&task.id);
        let mut record = DurableTaskRecord::new(task.clone());
        let mut config_ids = Vec::with_capacity(configs.len());
        for config in configs {
            Self::validate_push_config_identity(&scope, &task.id, &config)?;
            if record.delivery.callbacks.contains_key(&config.id) {
                return Err(TaskEngineError::PushConfigAlreadyExists(config.id));
            }
            config_ids.push(config.id.clone());
            record
                .delivery
                .callbacks
                .insert(config.id.clone(), DurableTaskCallback::new(config, 0));
            if record.delivery.callbacks.len() > MAX_DURABLE_TASK_CALLBACKS {
                return Err(TaskEngineError::PushConfigCapacityExceeded);
            }
        }
        record.delivery.validate(&record.task)?;
        let payload = record.encode()?;
        let inserted = self.state.check_and_set(&key, &payload, None).await?;
        if !inserted {
            return Err(TaskEngineError::AlreadyExists(task.id));
        }
        debug!(task_id = %task.id, namespace = %task.namespace, tenant = %task.tenant, "task created");
        self.emit_audit(&task, "create", None).await;
        for config_id in config_ids {
            self.emit_push_config_audit(&task, "create", &config_id)
                .await;
        }
        Ok(task)
    }

    fn validate_push_config_identity(
        scope: &TaskScope,
        task_id: &str,
        config: &TaskPushNotificationConfig,
    ) -> Result<(), TaskEngineError> {
        config.validate()?;
        if config.task_id != task_id
            || config.namespace != scope.namespace
            || config.tenant != scope.tenant
            || config.created_at > config.updated_at
        {
            return Err(TaskEngineError::PushConfigIdentityMismatch);
        }
        Ok(())
    }

    /// Fetch a task by scope + id.
    pub async fn get_task(
        &self,
        scope: &TaskScope,
        task_id: &str,
    ) -> Result<Option<Task>, TaskEngineError> {
        Ok(self
            .get_task_versioned(scope, task_id)
            .await?
            .map(|(task, _)| task))
    }

    /// Fetch a task together with its authoritative state-store version.
    ///
    /// The version is an opaque monotonic cursor within this task key. HTTP
    /// adapters use it for conditional observation; callers must not infer
    /// task state from the number itself.
    pub async fn get_task_versioned(
        &self,
        scope: &TaskScope,
        task_id: &str,
    ) -> Result<Option<(Task, u64)>, TaskEngineError> {
        if self.hidden_governed_task(scope, task_id).await? {
            return Ok(None);
        }
        let key = scope.task_key(task_id);
        let Some((raw, version)) = self.state.get_versioned(&key).await? else {
            return Ok(None);
        };
        let task = decode_persisted_task(&raw)?;
        Ok(Some((task, version)))
    }

    /// Upsert one callback configuration inside the Task envelope. A new
    /// callback starts at the current delivery tail; updating one preserves
    /// its delivery state and original creation timestamp. An actively leased
    /// callback cannot change underneath the request already in flight.
    pub async fn upsert_push_config(
        &self,
        scope: &TaskScope,
        task_id: &str,
        mut config: TaskPushNotificationConfig,
    ) -> Result<TaskPushNotificationConfig, TaskEngineError> {
        Self::validate_push_config_identity(scope, task_id, &config)?;
        if self.hidden_governed_task(scope, task_id).await? {
            return Err(TaskEngineError::NotFound(task_id.into()));
        }
        let key = scope.task_key(task_id);
        for _ in 0..MAX_CAS_RETRY_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                return Err(TaskEngineError::NotFound(task_id.into()));
            };
            let mut record = DurableTaskRecord::decode(&raw)?;
            let existed = record.delivery.callbacks.contains_key(&config.id);
            if !existed && record.delivery.callbacks.len() >= MAX_DURABLE_TASK_CALLBACKS {
                return Err(TaskEngineError::PushConfigCapacityExceeded);
            }
            let callback = match record.delivery.callbacks.get(&config.id) {
                Some(existing) => {
                    if existing.lease.is_some() {
                        return Err(TaskEngineError::PushConfigDeliveryInFlight);
                    }
                    config.created_at = existing.config.created_at;
                    let mut updated = existing.clone();
                    updated.config = config.clone();
                    updated
                }
                None => DurableTaskCallback::new(config.clone(), record.delivery.next_sequence),
            };
            if config.updated_at < config.created_at {
                return Err(TaskEngineError::PushConfigIdentityMismatch);
            }
            record
                .delivery
                .callbacks
                .insert(config.id.clone(), callback);
            record.delivery.validate(&record.task)?;
            let payload = record.encode()?;
            match self
                .state
                .compare_and_swap(&key, version, &payload, None)
                .await?
            {
                CasResult::Ok => {
                    self.sync_push_ready_hint_best_effort(&record).await;
                    let operation = if existed { "update" } else { "create" };
                    self.emit_push_config_audit(&record.task, operation, &config.id)
                        .await;
                    return Ok(config);
                }
                CasResult::Conflict { .. } => {}
            }
        }
        Err(TaskEngineError::CasExhausted(task_id.into()))
    }

    /// Read one callback configuration without exposing cursor or delivery
    /// state.
    pub async fn get_push_config(
        &self,
        scope: &TaskScope,
        task_id: &str,
        config_id: &str,
    ) -> Result<Option<TaskPushNotificationConfig>, TaskEngineError> {
        if self.hidden_governed_task(scope, task_id).await? {
            return Err(TaskEngineError::NotFound(task_id.into()));
        }
        let key = scope.task_key(task_id);
        let Some(raw) = self.state.get(&key).await? else {
            return Err(TaskEngineError::NotFound(task_id.into()));
        };
        let record = DurableTaskRecord::decode(&raw)?;
        Ok(record
            .delivery
            .callbacks
            .get(config_id)
            .map(|callback| callback.config.clone()))
    }

    /// List callback configurations in stable ID order.
    pub async fn list_push_configs(
        &self,
        scope: &TaskScope,
        task_id: &str,
    ) -> Result<Vec<TaskPushNotificationConfig>, TaskEngineError> {
        if self.hidden_governed_task(scope, task_id).await? {
            return Err(TaskEngineError::NotFound(task_id.into()));
        }
        let key = scope.task_key(task_id);
        let Some(raw) = self.state.get(&key).await? else {
            return Err(TaskEngineError::NotFound(task_id.into()));
        };
        let record = DurableTaskRecord::decode(&raw)?;
        Ok(record
            .delivery
            .callbacks
            .values()
            .map(|callback| callback.config.clone())
            .collect())
    }

    /// Claim one callback's next unacknowledged event under a fenced lease.
    /// Concurrent replicas race through the Task CAS; at most one receives a
    /// claim for a given callback and sequence.
    pub async fn claim_push_delivery(
        &self,
        scope: &TaskScope,
        task_id: &str,
        config_id: &str,
        worker_id: &str,
        lease_for: Duration,
    ) -> Result<Option<TaskPushDeliveryClaim>, TaskEngineError> {
        if worker_id.is_empty() || worker_id.len() > 256 {
            return Err(TaskEngineError::InvalidPushWorkerId);
        }
        if lease_for.is_zero() || lease_for > MAX_TASK_PUSH_LEASE {
            return Err(TaskEngineError::InvalidPushLeaseDuration);
        }
        if self.hidden_governed_task(scope, task_id).await? {
            return Err(TaskEngineError::NotFound(task_id.into()));
        }
        let lease_delta = chrono::Duration::from_std(lease_for)
            .map_err(|_| TaskEngineError::InvalidPushLeaseDuration)?;
        let key = scope.task_key(task_id);
        for _ in 0..MAX_CAS_RETRY_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                return Err(TaskEngineError::NotFound(task_id.into()));
            };
            let mut record = DurableTaskRecord::decode(&raw)?;
            let now = self.clock.now();
            let callback = record
                .delivery
                .callbacks
                .get(config_id)
                .ok_or_else(|| TaskEngineError::PushConfigNotFound(config_id.into()))?;
            if callback.blocked
                || callback.next_attempt_at.is_some_and(|ready| ready > now)
                || callback
                    .lease
                    .as_ref()
                    .is_some_and(|lease| lease.expires_at > now)
            {
                return Ok(None);
            }
            let sequence = callback
                .acknowledged_sequence
                .checked_add(1)
                .ok_or(TaskEngineError::PushDeliverySequenceExhausted)?;
            let Some(event) = record
                .delivery
                .events
                .iter()
                .find(|event| event.sequence == sequence)
                .cloned()
            else {
                return Ok(None);
            };
            let attempt = callback
                .attempts
                .checked_add(1)
                .ok_or(TaskEngineError::PushDeliveryAttemptExhausted)?;
            let lease = DurableTaskDeliveryLease {
                lease_id: uuid::Uuid::now_v7().to_string(),
                worker_id: worker_id.into(),
                sequence,
                event_id: event.event_id.clone(),
                attempt,
                claimed_at: now,
                expires_at: now + lease_delta,
            };
            let config = {
                let callback = record
                    .delivery
                    .callbacks
                    .get_mut(config_id)
                    .expect("callback remained present inside one record");
                callback.attempts = attempt;
                callback.next_attempt_at = None;
                callback.lease = Some(lease.clone());
                callback.config.clone()
            };
            record.delivery.validate(&record.task)?;
            let payload = record.encode()?;
            match self
                .state
                .compare_and_swap(&key, version, &payload, None)
                .await?
            {
                CasResult::Ok => {
                    self.sync_push_ready_hint_best_effort(&record).await;
                    return Ok(Some(TaskPushDeliveryClaim {
                        task_id: task_id.into(),
                        config,
                        sequence,
                        event_id: event.event_id,
                        occurred_at: event.occurred_at,
                        payload: event.payload,
                        attempt,
                        lease_id: lease.lease_id,
                        lease_expires_at: lease.expires_at,
                    }));
                }
                CasResult::Conflict { .. } => {}
            }
        }
        Err(TaskEngineError::CasExhausted(task_id.into()))
    }

    /// Acknowledge a successful leased delivery and advance exactly one
    /// sequence. A stale or replaced owner cannot advance the cursor.
    pub async fn acknowledge_push_delivery(
        &self,
        scope: &TaskScope,
        task_id: &str,
        config_id: &str,
        lease_id: &str,
    ) -> Result<(), TaskEngineError> {
        self.finish_push_delivery(scope, task_id, config_id, lease_id, None)
            .await
    }

    /// Release a failed claim and durably schedule its same sequence for a
    /// later attempt.
    pub async fn retry_push_delivery_at(
        &self,
        scope: &TaskScope,
        task_id: &str,
        config_id: &str,
        lease_id: &str,
        retry_at: DateTime<Utc>,
    ) -> Result<(), TaskEngineError> {
        self.finish_push_delivery(scope, task_id, config_id, lease_id, Some(retry_at))
            .await
    }

    async fn finish_push_delivery(
        &self,
        scope: &TaskScope,
        task_id: &str,
        config_id: &str,
        lease_id: &str,
        retry_at: Option<DateTime<Utc>>,
    ) -> Result<(), TaskEngineError> {
        let key = scope.task_key(task_id);
        for _ in 0..MAX_CAS_RETRY_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                return Err(TaskEngineError::NotFound(task_id.into()));
            };
            let mut record = DurableTaskRecord::decode(&raw)?;
            let callback = record
                .delivery
                .callbacks
                .get_mut(config_id)
                .ok_or_else(|| TaskEngineError::PushConfigNotFound(config_id.into()))?;
            let lease = callback
                .lease
                .as_ref()
                .filter(|lease| lease.lease_id == lease_id)
                .cloned()
                .ok_or(TaskEngineError::PushDeliveryLeaseLost)?;
            if lease.expires_at <= self.clock.now() {
                return Err(TaskEngineError::PushDeliveryLeaseLost);
            }
            callback.lease = None;
            if let Some(retry_at) = retry_at {
                if retry_at < self.clock.now() {
                    return Err(TaskEngineError::InvalidPushRetryTime);
                }
                callback.next_attempt_at = Some(retry_at);
            } else {
                callback.acknowledged_sequence = lease.sequence;
                callback.next_attempt_at = None;
                callback.attempts = 0;
                record.delivery.prune_acknowledged();
            }
            record.delivery.validate(&record.task)?;
            let payload = record.encode()?;
            match self
                .state
                .compare_and_swap(&key, version, &payload, None)
                .await?
            {
                CasResult::Ok => {
                    self.sync_push_ready_hint_best_effort(&record).await;
                    return Ok(());
                }
                CasResult::Conflict { .. } => {}
            }
        }
        Err(TaskEngineError::CasExhausted(task_id.into()))
    }

    /// Move a terminally failed leased event into the Task envelope's bounded
    /// dead-letter state. The callback remains blocked at this sequence until
    /// an operator policy resolves it.
    pub async fn dead_letter_push_delivery(
        &self,
        scope: &TaskScope,
        task_id: &str,
        config_id: &str,
        lease_id: &str,
        failure_kind: DlqFailureKind,
        last_error: &str,
    ) -> Result<PushDeliveryDlqEntry, TaskEngineError> {
        let key = scope.task_key(task_id);
        for _ in 0..MAX_CAS_RETRY_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                return Err(TaskEngineError::NotFound(task_id.into()));
            };
            let mut record = DurableTaskRecord::decode(&raw)?;
            if record.delivery.dead_letters.len() >= MAX_DURABLE_TASK_DEAD_LETTERS {
                return Err(TaskEngineError::PushDeadLetterCapacityExceeded);
            }
            let callback = record
                .delivery
                .callbacks
                .get(config_id)
                .ok_or_else(|| TaskEngineError::PushConfigNotFound(config_id.into()))?;
            let lease = callback
                .lease
                .as_ref()
                .filter(|lease| lease.lease_id == lease_id)
                .cloned()
                .ok_or(TaskEngineError::PushDeliveryLeaseLost)?;
            let now = self.clock.now();
            if lease.expires_at <= now {
                return Err(TaskEngineError::PushDeliveryLeaseLost);
            }
            let event = record
                .delivery
                .events
                .iter()
                .find(|event| event.sequence == lease.sequence)
                .ok_or_else(|| {
                    TaskEngineError::InvalidDurableRecord("leased event is absent".into())
                })?;
            let entry = PushDeliveryDlqEntry::new_at(
                uuid::Uuid::now_v7().to_string(),
                config_id,
                task_id,
                &scope.namespace,
                &scope.tenant,
                &callback.config.url,
                failure_kind,
                last_error,
                callback.attempts,
                serde_json::to_string(&event.payload)?,
                now,
            )
            .with_delivery_identity(lease.sequence, &lease.event_id);
            let callback = record
                .delivery
                .callbacks
                .get_mut(config_id)
                .expect("callback remained present inside one record");
            callback.lease = None;
            callback.next_attempt_at = None;
            callback.blocked = true;
            record.delivery.dead_letters.push(entry.clone());
            record.delivery.validate(&record.task)?;
            let payload = record.encode()?;
            match self
                .state
                .compare_and_swap(&key, version, &payload, None)
                .await?
            {
                CasResult::Ok => {
                    self.sync_push_ready_hint_best_effort(&record).await;
                    return Ok(entry);
                }
                CasResult::Conflict { .. } => {}
            }
        }
        Err(TaskEngineError::CasExhausted(task_id.into()))
    }

    /// Project Task-embedded dead letters without exposing callback lease state.
    pub async fn list_push_dead_letters(
        &self,
        scope: &TaskScope,
        task_id: &str,
    ) -> Result<Vec<PushDeliveryDlqEntry>, TaskEngineError> {
        let key = scope.task_key(task_id);
        let Some(raw) = self.state.get(&key).await? else {
            return Err(TaskEngineError::NotFound(task_id.into()));
        };
        Ok(DurableTaskRecord::decode(&raw)?.delivery.dead_letters)
    }

    /// Import append-only dead letters written by a pre-envelope worker.
    ///
    /// The Task envelope wins an exact ID replay. A duplicate ID with different
    /// evidence is rejected rather than hidden, and the capacity check covers
    /// the merged set before the Task CAS commits. Callers may delete standalone
    /// rows only after this method succeeds.
    pub async fn import_legacy_push_dead_letters(
        &self,
        scope: &TaskScope,
        task_id: &str,
        entries: Vec<PushDeliveryDlqEntry>,
    ) -> Result<usize, TaskEngineError> {
        let mut incoming_ids = HashSet::with_capacity(entries.len());
        for entry in &entries {
            if entry.task_id != task_id
                || entry.namespace != scope.namespace
                || entry.tenant != scope.tenant
                || entry.sequence.is_some()
                || entry.event_id.is_some()
                || uuid::Uuid::parse_str(&entry.id).is_err()
                || entry.config_id.is_empty()
                || entry.url.is_empty()
                || entry.attempts == 0
                || entry.first_failed_at > entry.last_failed_at
                || entry.last_error.len() > MAX_DLQ_ERROR_BYTES
                || entry.event_json.len() > MAX_DLQ_EVENT_BYTES
                || !incoming_ids.insert(entry.id.as_str())
            {
                return Err(TaskEngineError::InvalidDurableRecord(
                    "invalid legacy callback dead letter".into(),
                ));
            }
        }
        let key = scope.task_key(task_id);
        for _ in 0..MAX_CAS_RETRY_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                return Err(TaskEngineError::NotFound(task_id.into()));
            };
            let mut record = DurableTaskRecord::decode(&raw)?;
            let mut imported = 0usize;
            for entry in &entries {
                if let Some(existing) = record
                    .delivery
                    .dead_letters
                    .iter()
                    .find(|existing| existing.id == entry.id)
                {
                    if existing != entry {
                        return Err(TaskEngineError::InvalidDurableRecord(
                            "legacy callback dead-letter ID conflicts with Task evidence".into(),
                        ));
                    }
                    continue;
                }
                record.delivery.dead_letters.push(entry.clone());
                imported += 1;
            }
            if imported == 0 {
                return Ok(0);
            }
            if record.delivery.dead_letters.len() > MAX_DURABLE_TASK_DEAD_LETTERS {
                return Err(TaskEngineError::PushDeadLetterCapacityExceeded);
            }
            record.delivery.validate(&record.task)?;
            let payload = record.encode()?;
            match self
                .state
                .compare_and_swap(&key, version, &payload, None)
                .await?
            {
                CasResult::Ok => return Ok(imported),
                CasResult::Conflict { .. } => {}
            }
        }
        Err(TaskEngineError::CasExhausted(task_id.into()))
    }

    /// Resolve one Task-embedded dead letter under the same CAS as its callback
    /// cursor. Retrying keeps the failed sequence pending; discarding advances
    /// over that sequence. Both operations unblock the callback atomically.
    pub async fn resolve_push_dead_letter(
        &self,
        scope: &TaskScope,
        task_id: &str,
        entry_id: &str,
        resolution: PushDeadLetterResolution,
    ) -> Result<PushDeliveryDlqEntry, TaskEngineError> {
        let key = scope.task_key(task_id);
        for _ in 0..MAX_CAS_RETRY_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                return Err(TaskEngineError::NotFound(task_id.into()));
            };
            let mut record = DurableTaskRecord::decode(&raw)?;
            let index = record
                .delivery
                .dead_letters
                .iter()
                .position(|entry| entry.id == entry_id)
                .ok_or_else(|| TaskEngineError::PushDeadLetterNotFound(entry_id.into()))?;
            let entry = record.delivery.dead_letters[index].clone();
            if let Some(callback) = record.delivery.callbacks.get_mut(&entry.config_id)
                && callback.blocked
                && callback.lease.is_none()
                && entry.sequence == callback.acknowledged_sequence.checked_add(1)
            {
                if resolution == PushDeadLetterResolution::Discard {
                    callback.acknowledged_sequence = callback
                        .acknowledged_sequence
                        .checked_add(1)
                        .ok_or(TaskEngineError::PushDeliverySequenceExhausted)?;
                }
                callback.attempts = 0;
                callback.next_attempt_at = None;
                callback.blocked = false;
            }
            record.delivery.dead_letters.remove(index);
            record.delivery.prune_acknowledged();
            record.delivery.validate(&record.task)?;
            let payload = record.encode()?;
            match self
                .state
                .compare_and_swap(&key, version, &payload, None)
                .await?
            {
                CasResult::Ok => {
                    self.sync_push_ready_hint_best_effort(&record).await;
                    return Ok(entry);
                }
                CasResult::Conflict { .. } => {}
            }
        }
        Err(TaskEngineError::CasExhausted(task_id.into()))
    }

    /// Whether the one-time standalone push-config scan has been committed for
    /// this Task. Callers use this before scanning the legacy key kind.
    pub async fn push_config_migration_complete(
        &self,
        scope: &TaskScope,
        task_id: &str,
    ) -> Result<bool, TaskEngineError> {
        if self.hidden_governed_task(scope, task_id).await? {
            return Err(TaskEngineError::NotFound(task_id.into()));
        }
        let key = scope.task_key(task_id);
        let Some(raw) = self.state.get(&key).await? else {
            return Err(TaskEngineError::NotFound(task_id.into()));
        };
        Ok(DurableTaskRecord::decode(&raw)?
            .delivery
            .legacy_config_scan_complete)
    }

    /// Import legacy standalone configurations once and seal the migration
    /// marker in the same Task CAS. Existing envelope entries win a race with a
    /// stale legacy row; newly imported callbacks start at the current tail.
    pub async fn import_legacy_push_configs(
        &self,
        scope: &TaskScope,
        task_id: &str,
        configs: Vec<TaskPushNotificationConfig>,
    ) -> Result<Vec<TaskPushNotificationConfig>, TaskEngineError> {
        for config in &configs {
            Self::validate_push_config_identity(scope, task_id, config)?;
        }
        if self.hidden_governed_task(scope, task_id).await? {
            return Err(TaskEngineError::NotFound(task_id.into()));
        }
        let key = scope.task_key(task_id);
        for _ in 0..MAX_CAS_RETRY_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                return Err(TaskEngineError::NotFound(task_id.into()));
            };
            let mut record = DurableTaskRecord::decode(&raw)?;
            if !record.delivery.legacy_config_scan_complete {
                let tail = record.delivery.next_sequence;
                let mut imported_ids = Vec::new();
                for config in &configs {
                    if let std::collections::btree_map::Entry::Vacant(entry) =
                        record.delivery.callbacks.entry(config.id.clone())
                    {
                        imported_ids.push(config.id.clone());
                        entry.insert(DurableTaskCallback::new(config.clone(), tail));
                    }
                }
                if record.delivery.callbacks.len() > MAX_DURABLE_TASK_CALLBACKS {
                    return Err(TaskEngineError::PushConfigCapacityExceeded);
                }
                record.delivery.legacy_config_scan_complete = true;
                record.delivery.validate(&record.task)?;
                let payload = record.encode()?;
                if matches!(
                    self.state
                        .compare_and_swap(&key, version, &payload, None)
                        .await?,
                    CasResult::Conflict { .. }
                ) {
                    continue;
                }
                for config_id in imported_ids {
                    self.emit_push_config_audit(&record.task, "legacy_import", &config_id)
                        .await;
                }
            }
            return Ok(record
                .delivery
                .callbacks
                .values()
                .map(|callback| callback.config.clone())
                .collect());
        }
        Err(TaskEngineError::CasExhausted(task_id.into()))
    }

    /// Delete a callback configuration and immediately release any event-log
    /// prefix retained only for that callback. Returns `false` when absent.
    pub async fn delete_push_config(
        &self,
        scope: &TaskScope,
        task_id: &str,
        config_id: &str,
    ) -> Result<bool, TaskEngineError> {
        if self.hidden_governed_task(scope, task_id).await? {
            return Err(TaskEngineError::NotFound(task_id.into()));
        }
        let key = scope.task_key(task_id);
        for _ in 0..MAX_CAS_RETRY_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                return Err(TaskEngineError::NotFound(task_id.into()));
            };
            let mut record = DurableTaskRecord::decode(&raw)?;
            if record.delivery.callbacks.remove(config_id).is_none() {
                return Ok(false);
            }
            record.delivery.prune_acknowledged();
            record.delivery.validate(&record.task)?;
            let payload = record.encode()?;
            match self
                .state
                .compare_and_swap(&key, version, &payload, None)
                .await?
            {
                CasResult::Ok => {
                    self.sync_push_ready_hint_best_effort(&record).await;
                    self.emit_push_config_audit(&record.task, "delete", config_id)
                        .await;
                    return Ok(true);
                }
                CasResult::Conflict { .. } => {}
            }
        }
        Err(TaskEngineError::CasExhausted(task_id.into()))
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
        let key_prefix = scope.task_key("").canonical();
        for (key, raw) in entries {
            let Some(id) = key.strip_prefix(&key_prefix) else {
                warn!("skipping task row outside its canonical scope");
                continue;
            };
            match decode_persisted_task(&raw) {
                Ok(t) => {
                    if !self.hidden_governed_task(scope, id).await? {
                        out.push(t);
                    }
                }
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
            .cas_mutate(
                &key,
                task_id,
                "transition",
                None,
                move |task: &mut Task, now| {
                    task.transition_to_at(next, message.clone(), now)?;
                    Ok(())
                },
            )
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
        let status_event_id = uuid::Uuid::now_v7().to_string();
        for _ in 0..MAX_CAS_RETRY_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                // Task was deleted between the reaper's scan and now.
                return Ok(None);
            };
            let mut record = DurableTaskRecord::decode(&raw)?;
            let task = &mut record.task;
            // Governed work may have an unresolved external attempt. Its
            // runtime projects qualified execution evidence; elapsed time alone
            // cannot prove failure or release its retained capacity.
            if task
                .metadata
                .contains_key(crate::agent_runtime::GOVERNED_TASK_METADATA_KEY)
                || (task.is_stale_at(now)
                    && self
                        .state
                        .get(&StateKey::new(
                            scope.namespace.as_str(),
                            scope.tenant.as_str(),
                            KeyKind::Custom(crate::agent_runtime::ACCEPTANCE_KIND.into()),
                            task_id,
                        ))
                        .await?
                        .is_some())
                || !task.is_stale_at(now)
            {
                // Recorded progress or reached a terminal state since
                // the scan — no longer a zombie, leave it untouched.
                return Ok(None);
            }
            let from_state = task.status.state;
            task.transition_to_at(TaskState::Failed, Some(reason.clone()), now)?;
            let task = record.task.clone();
            record.delivery.append(
                status_event_id.clone(),
                now,
                project_task_status_push_payload(&task, TaskState::Failed, now)?,
            )?;
            let payload = record.encode()?;
            match self
                .state
                .compare_and_swap(&key, version, &payload, None)
                .await?
            {
                CasResult::Ok => {
                    self.sync_push_ready_hint_best_effort(&record).await;
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
                    return Ok(Some(task.clone()));
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
        if self.hidden_governed_task(scope, task_id).await? {
            return Err(TaskEngineError::NotFound(task_id.into()));
        }
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
            .cas_mutate(
                &key,
                task_id,
                "append_history",
                None,
                move |task: &mut Task, now| {
                    task.append_history_at(message.clone(), now)?;
                    Ok(())
                },
            )
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
            .cas_mutate(
                &key,
                &task_id,
                "artifact_update",
                Some((artifact_id_for_emit.clone(), last_chunk_for_emit)),
                move |task: &mut Task, now| {
                    task.apply_artifact_event_at(&event, now)?;
                    Ok(())
                },
            )
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
        self.cas_mutate(&key, task_id, "progress", None, |task: &mut Task, now| {
            task.record_progress_at(now);
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
        self.cas_mutate(
            &key,
            task_id,
            "pending_approval",
            None,
            move |task: &mut Task, _now| {
                task.set_pending_approval(approval_id.clone());
                Ok(())
            },
        )
        .await
    }

    /// Pause a Task for structured human input and create its
    /// [`BusApproval`] row.
    ///
    /// This is the Task-side entry point for the `BusApproval`
    /// generalization. It performs two writes:
    ///
    /// 1. Persists a fresh task-pause [`BusApproval`] row (status
    ///    `Pending`) at [`KeyKind::BusApproval`], plus a
    ///    [`KeyKind::PendingBusApprovals`] index entry so the row
    ///    appears in `status=pending` listings.
    /// 2. CAS-mutates the Task to [`TaskState::InputRequired`] and stamps the
    ///    approval id onto `Task.pending_approval_id`.
    ///
    /// `kind` must be [`PauseKind::UserInput`].
    /// [`PauseKind::OperatorApproval`] returns
    /// [`TaskEngineError::InvalidPauseKind`]; [`PauseKind::UserAuth`] returns
    /// [`TaskEngineError::AuthorizationRequirementRequired`] so callers use
    /// [`TaskEngine::pause_for_authorization`] with a host-owned binding.
    ///
    /// The transition itself can be rejected: A2A only allows
    /// `InputRequired` from `Working`, so a task
    /// still `Submitted` or already terminal cannot be paused.
    ///
    /// The two writes are not one transaction (separate state keys —
    /// the same trade-off the bus's park flow accepts). The approval
    /// row is written first. After a task-mutation error, Acteon reads
    /// the Task back: an exact matching pause proves a lost acknowledgement
    /// and is returned as success; a definitive non-match allows the orphan
    /// approval and index to be deleted. If that read is also unavailable,
    /// the approval is retained rather than risking deletion of a live gate.
    ///
    /// `ttl` defaults to [`DEFAULT_APPROVAL_TTL_MS`] and is clamped to
    /// [`MAX_APPROVAL_TTL_MS`].
    #[allow(clippy::too_many_lines)]
    pub async fn pause_for_human(
        &self,
        scope: &TaskScope,
        task_id: &str,
        kind: PauseKind,
        reason: Option<String>,
        ttl: Option<Duration>,
    ) -> Result<(Task, BusApproval), TaskEngineError> {
        if kind == PauseKind::UserAuth {
            return Err(TaskEngineError::AuthorizationRequirementRequired);
        }
        self.pause_task(scope, task_id, kind, reason, ttl, None)
            .await
    }

    /// Pause a task on an authorization request whose complete trust binding
    /// was issued by the host. Only a verifier with the pinned ID and revision
    /// can later resume it.
    pub async fn pause_for_authorization(
        &self,
        scope: &TaskScope,
        task_id: &str,
        requirement: TaskAuthorizationRequirement,
        reason: Option<String>,
        ttl: Option<Duration>,
    ) -> Result<(Task, BusApproval), TaskEngineError> {
        self.pause_task(
            scope,
            task_id,
            PauseKind::UserAuth,
            reason,
            ttl,
            Some(requirement),
        )
        .await
    }

    #[allow(clippy::too_many_lines)]
    async fn pause_task(
        &self,
        scope: &TaskScope,
        task_id: &str,
        kind: PauseKind,
        reason: Option<String>,
        ttl: Option<Duration>,
        authorization_requirement: Option<TaskAuthorizationRequirement>,
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
        let now = self.clock.now();
        let expires_at =
            now + chrono::Duration::milliseconds(i64::try_from(ttl_ms).unwrap_or(i64::MAX));
        let approval_id = uuid::Uuid::now_v7().to_string();
        let challenge_reason = reason.clone();
        let mut approval = BusApproval::new_task_pause(
            &approval_id,
            &scope.namespace,
            &scope.tenant,
            kind,
            task_id,
            reason,
            now,
            expires_at,
        );
        approval.authorization_requirement = authorization_requirement;
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
        let prompt_id = format!("{approval_id}.challenge");
        let mutated = self
            .cas_mutate(&key, task_id, "pause", None, move |task: &mut Task, now| {
                let prompt = challenge_reason.as_ref().map(|reason| {
                    let mut message =
                        TaskMessage::text(prompt_id.clone(), TaskRole::Agent, reason.clone());
                    message.task_id = Some(task.id.clone());
                    message.context_id.clone_from(&task.context_id);
                    message
                });
                task.transition_to_at(target_state, prompt, now)?;
                task.set_pending_approval(stamp_id.clone());
                Ok(())
            })
            .await;

        let (task, recovered) = match mutated {
            Ok(task) => (task, false),
            Err(e) => match self.get_task(scope, task_id).await {
                Ok(Some(task))
                    if task.status.state == target_state
                        && task.pending_approval_id.as_deref() == Some(approval_id.as_str()) =>
                {
                    (task, true)
                }
                Err(read_error) => {
                    warn!(%task_id, %approval_id, %e, %read_error, "task pause outcome is uncertain; retaining approval gate");
                    return Err(e);
                }
                Ok(_) => {
                    if let Err(del) = self.state.delete(&approval_key).await {
                        warn!(
                            approval_id = %approval_id,
                            error = %del,
                            "failed to delete orphan approval row after task pause failed",
                        );
                    }
                    let _ = self.state.delete(&index_key).await;
                    return Err(e);
                }
            },
        };
        if recovered {
            self.emit_audit(&task, "pause", Some(TaskState::Working))
                .await;
        }
        debug!(
            task_id = %task_id,
            approval_id = %approval_id,
            kind = kind.as_str(),
            recovered,
            "task paused for human",
        );
        // A successful pause always transitions `Working -> target_state`.
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

    /// Consume the exact pending `UserInput` challenge, append its response,
    /// and resume the task in one task-row CAS.
    ///
    /// The approval row is first moved to `Approving` with a digest-only
    /// response intent.  A crash at either side of the two-row boundary is
    /// recoverable by retrying the same `(approval_id, message)`; a different
    /// response is rejected.  Raw input is stored once, in task history.
    /// `UserAuth` pauses deliberately cannot use this operation because an A2A
    /// message is not proof that an authorization challenge was satisfied.
    pub async fn resolve_input(
        &self,
        scope: &TaskScope,
        task_id: &str,
        approval_id: &str,
        message: TaskMessage,
        decided_by: impl Into<String>,
    ) -> Result<(Task, BusApproval), TaskEngineError> {
        let resolution = self
            .prepare_input_resolution(scope, task_id, approval_id, &message)
            .await?;
        let actor = decided_by.into();
        if let Err(error) = self
            .claim_input_resolution(scope, task_id, approval_id, &resolution, &actor)
            .await
        {
            if matches!(
                error,
                TaskEngineError::ChallengeExpired(_)
                    | TaskEngineError::ChallengeClosed {
                        status: BusApprovalStatus::Expired,
                        ..
                    }
            ) && let Err(expire_error) = self
                .fail_expired_input_challenge(scope, task_id, approval_id)
                .await
            {
                warn!(%task_id, %approval_id, %expire_error, "failed to project expired input challenge onto task");
            }
            return Err(error);
        }
        let task = match self
            .apply_input_resolution(scope, task_id, approval_id, &resolution, message)
            .await
        {
            Ok(task) => task,
            Err(error) => {
                // These failures are definitive reads/validation failures: no
                // task-row CAS may have committed. Close the claimed intent so
                // a concurrent cancellation or replacement challenge cannot
                // strand the approval in Approving forever. Backend, serde and
                // CAS-exhaustion errors stay Approving because their commit
                // outcome may be uncertain and an identical retry can repair.
                if matches!(
                    error,
                    TaskEngineError::NotFound(_)
                        | TaskEngineError::Validation(_)
                        | TaskEngineError::ChallengeMismatch { .. }
                        | TaskEngineError::ChallengeResponseConflict(_)
                        | TaskEngineError::ChallengeResponseContextMismatch
                ) && let Err(close_error) = self
                    .reject_abandoned_input_resolution(scope, task_id, approval_id, &resolution)
                    .await
                {
                    warn!(%task_id, %approval_id, %close_error, "failed to close abandoned task input resolution");
                }
                return Err(error);
            }
        };
        let approval = self
            .finish_input_resolution(scope, task_id, approval_id, &resolution)
            .await?;

        let index_key = StateKey::new(
            scope.namespace.clone(),
            scope.tenant.clone(),
            KeyKind::PendingBusApprovals,
            approval_id,
        );
        if let Err(error) = self.state.delete(&index_key).await {
            warn!(%approval_id, %error, "failed to remove resolved task challenge from pending index");
        }
        debug!(%task_id, %approval_id, message_id = %resolution.message_id, "task input challenge resolved");
        Ok((task, approval))
    }

    /// Verify and consume the exact host-bound `UserAuth` challenge, then
    /// resume the task without accepting or persisting credential material.
    ///
    /// The verifier is selected by the persisted requirement, never by the
    /// caller. Its successful decision is claimed before the task-row CAS.
    /// Identical retries recover either side of that two-row boundary.
    pub async fn resolve_authorization(
        &self,
        scope: &TaskScope,
        task_id: &str,
        approval_id: &str,
        verifier: &dyn TaskAuthorizationVerifier,
    ) -> Result<(Task, BusApproval), TaskEngineError> {
        let (task, approval, requirement) = self
            .prepare_authorization_resolution(scope, task_id, approval_id)
            .await?;

        // A lost response after the task CAS needs no fresh external decision:
        // the exact claimed verifier result can only be finalized, never used
        // to mutate another task or challenge.
        if task.status.state == TaskState::Working && task.pending_approval_id.is_none() {
            let resolution = approval.authorization_resolution.as_ref().ok_or_else(|| {
                TaskEngineError::AuthorizationDecisionConflict(approval_id.to_string())
            })?;
            let finished = self
                .finish_authorization_resolution(scope, task_id, approval_id, resolution)
                .await?;
            self.remove_pending_challenge(scope, approval_id).await;
            return Ok((task, finished));
        }

        if verifier.verifier_id() != requirement.verifier_id
            || verifier.revision() != requirement.verifier_revision
        {
            return Err(TaskEngineError::AuthorizationVerifierMismatch {
                expected_id: requirement.verifier_id,
                expected_revision: requirement.verifier_revision,
            });
        }

        let request = TaskAuthorizationVerification {
            scope: scope.clone(),
            task_id: task_id.to_string(),
            challenge_id: approval_id.to_string(),
            requirement: requirement.clone(),
        };
        let evidence = verifier.verify(&request).await?;
        let proposed = self.authorization_resolution(&requirement, evidence)?;
        let claimed = match self
            .claim_authorization_resolution(scope, task_id, approval_id, &proposed)
            .await
        {
            Ok(claimed) => claimed,
            Err(error) => {
                if matches!(
                    error,
                    TaskEngineError::ChallengeExpired(_)
                        | TaskEngineError::ChallengeClosed {
                            status: BusApprovalStatus::Expired,
                            ..
                        }
                ) && let Err(expire_error) = self
                    .fail_expired_authorization_challenge(scope, task_id, approval_id)
                    .await
                {
                    warn!(%task_id, %approval_id, %expire_error, "failed to project expired authorization challenge onto task");
                }
                return Err(error);
            }
        };
        let resolution = claimed.authorization_resolution.clone().ok_or_else(|| {
            TaskEngineError::AuthorizationDecisionConflict(approval_id.to_string())
        })?;

        let task = match self
            .apply_authorization_resolution(scope, task_id, approval_id, &resolution)
            .await
        {
            Ok(task) => task,
            Err(error) => {
                if matches!(
                    error,
                    TaskEngineError::NotFound(_)
                        | TaskEngineError::Validation(_)
                        | TaskEngineError::ChallengeMismatch { .. }
                ) && let Err(close_error) = self
                    .reject_abandoned_authorization_resolution(
                        scope,
                        task_id,
                        approval_id,
                        &resolution,
                    )
                    .await
                {
                    warn!(%task_id, %approval_id, %close_error, "failed to close abandoned authorization resolution");
                }
                return Err(error);
            }
        };
        let approval = self
            .finish_authorization_resolution(scope, task_id, approval_id, &resolution)
            .await?;
        self.remove_pending_challenge(scope, approval_id).await;
        debug!(%task_id, %approval_id, decision_id = %resolution.decision_id, "task authorization challenge resolved");
        Ok((task, approval))
    }

    async fn prepare_authorization_resolution(
        &self,
        scope: &TaskScope,
        task_id: &str,
        approval_id: &str,
    ) -> Result<(Task, BusApproval, TaskAuthorizationRequirement), TaskEngineError> {
        let task = self
            .get_task(scope, task_id)
            .await?
            .ok_or_else(|| TaskEngineError::NotFound(task_id.to_string()))?;
        match task.status.state {
            TaskState::AuthRequired if task.pending_approval_id.as_deref() == Some(approval_id) => {
            }
            TaskState::Working if task.pending_approval_id.is_none() => {}
            _ => {
                return Err(TaskEngineError::ChallengeMismatch {
                    task_id: task_id.to_string(),
                    approval_id: approval_id.to_string(),
                });
            }
        }
        let key = StateKey::new(
            scope.namespace.clone(),
            scope.tenant.clone(),
            KeyKind::BusApproval,
            approval_id,
        );
        let raw = self
            .state
            .get(&key)
            .await?
            .ok_or_else(|| TaskEngineError::ChallengeNotFound(approval_id.to_string()))?;
        let approval: BusApproval = serde_json::from_str(&raw)?;
        let requirement =
            Self::validate_authorization_approval(scope, task_id, approval_id, &approval)?.clone();
        Ok((task, approval, requirement))
    }

    fn authorization_resolution(
        &self,
        requirement: &TaskAuthorizationRequirement,
        evidence: VerifiedTaskAuthorization,
    ) -> Result<TaskAuthorizationResolution, TaskEngineError> {
        let now = self.clock.now();
        if evidence.verified_at > now || evidence.valid_until <= now {
            return Err(TaskEngineError::AuthorizationEvidenceExpired);
        }
        let resolution = TaskAuthorizationResolution {
            verifier_id: requirement.verifier_id.clone(),
            verifier_revision: requirement.verifier_revision,
            authorization_request_digest: hex::encode(Sha256::digest(
                requirement.authorization_request_id.as_bytes(),
            )),
            decision_id: evidence.decision_id,
            subject: evidence.subject,
            verified_at: evidence.verified_at,
            valid_until: evidence.valid_until,
        };
        // Reuse BusApproval validation as the bounded wire-shape authority.
        let mut probe = BusApproval::new_task_pause(
            "authorization-probe",
            "probe",
            "probe",
            PauseKind::UserAuth,
            "probe",
            None,
            now,
            now + chrono::Duration::seconds(1),
        );
        probe.authorization_requirement = Some(requirement.clone());
        probe.authorization_resolution = Some(resolution.clone());
        probe.status = BusApprovalStatus::Approving;
        probe.validate()?;
        Ok(resolution)
    }

    fn same_authorization_decision(
        left: &TaskAuthorizationResolution,
        right: &TaskAuthorizationResolution,
    ) -> bool {
        left.verifier_id == right.verifier_id
            && left.verifier_revision == right.verifier_revision
            && left.authorization_request_digest == right.authorization_request_digest
            && left.decision_id == right.decision_id
            && left.subject == right.subject
    }

    #[allow(clippy::too_many_lines)]
    async fn claim_authorization_resolution(
        &self,
        scope: &TaskScope,
        task_id: &str,
        approval_id: &str,
        resolution: &TaskAuthorizationResolution,
    ) -> Result<BusApproval, TaskEngineError> {
        let key = StateKey::new(
            scope.namespace.clone(),
            scope.tenant.clone(),
            KeyKind::BusApproval,
            approval_id,
        );
        for _ in 0..MAX_CAS_RETRY_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                return Err(TaskEngineError::ChallengeNotFound(approval_id.to_string()));
            };
            let mut approval: BusApproval = serde_json::from_str(&raw)?;
            Self::validate_authorization_approval(scope, task_id, approval_id, &approval)?;
            match approval.status {
                BusApprovalStatus::Pending => {
                    let now = self.clock.now();
                    if now >= approval.expires_at {
                        approval.status = BusApprovalStatus::Expired;
                        approval.decided_at = Some(now);
                        approval.validate()?;
                        let payload = serde_json::to_string(&approval)?;
                        if matches!(
                            self.state
                                .compare_and_swap(&key, version, &payload, None)
                                .await?,
                            CasResult::Ok
                        ) {
                            self.remove_pending_challenge(scope, approval_id).await;
                            return Err(TaskEngineError::ChallengeExpired(approval_id.to_string()));
                        }
                        continue;
                    }
                    approval.status = BusApprovalStatus::Approving;
                    approval.authorization_resolution = Some(resolution.clone());
                    approval.decided_by = Some(resolution.subject.id().to_string());
                    approval.decided_at = Some(now);
                    approval.validate()?;
                    let payload = serde_json::to_string(&approval)?;
                    if matches!(
                        self.state
                            .compare_and_swap(&key, version, &payload, None)
                            .await?,
                        CasResult::Ok
                    ) {
                        return Ok(approval);
                    }
                }
                BusApprovalStatus::Approving | BusApprovalStatus::Approved => {
                    let Some(claimed) = approval.authorization_resolution.as_ref() else {
                        return Err(TaskEngineError::AuthorizationDecisionConflict(
                            approval_id.to_string(),
                        ));
                    };
                    if !Self::same_authorization_decision(claimed, resolution) {
                        return Err(TaskEngineError::AuthorizationDecisionConflict(
                            approval_id.to_string(),
                        ));
                    }
                    if approval.status == BusApprovalStatus::Approving {
                        let now = self.clock.now();
                        // The stable decision identity stays fixed, while a
                        // fresh verifier call may update its validity window.
                        if claimed != resolution {
                            approval.authorization_resolution = Some(resolution.clone());
                            approval.decided_at = Some(now);
                            approval.validate()?;
                            let payload = serde_json::to_string(&approval)?;
                            if matches!(
                                self.state
                                    .compare_and_swap(&key, version, &payload, None)
                                    .await?,
                                CasResult::Ok
                            ) {
                                return Ok(approval);
                            }
                            continue;
                        }
                    }
                    return Ok(approval);
                }
                BusApprovalStatus::Rejected | BusApprovalStatus::Expired => {
                    return Err(TaskEngineError::ChallengeClosed {
                        approval_id: approval_id.to_string(),
                        status: approval.status,
                    });
                }
            }
        }
        Err(TaskEngineError::ChallengeCasExhausted(
            approval_id.to_string(),
        ))
    }

    fn validate_authorization_approval<'a>(
        scope: &TaskScope,
        task_id: &str,
        approval_id: &str,
        approval: &'a BusApproval,
    ) -> Result<&'a TaskAuthorizationRequirement, TaskEngineError> {
        approval.validate()?;
        if approval.approval_id != approval_id
            || approval.namespace != scope.namespace
            || approval.tenant != scope.tenant
            || approval.task_id.as_deref() != Some(task_id)
        {
            return Err(TaskEngineError::ChallengeMismatch {
                task_id: task_id.to_string(),
                approval_id: approval_id.to_string(),
            });
        }
        if approval.kind != PauseKind::UserAuth {
            return Err(TaskEngineError::ChallengeKind {
                approval_id: approval_id.to_string(),
                actual: approval.kind,
            });
        }
        approval
            .authorization_requirement
            .as_ref()
            .ok_or_else(|| TaskEngineError::AuthorizationChallengeUnbound(approval_id.to_string()))
    }

    async fn apply_authorization_resolution(
        &self,
        scope: &TaskScope,
        task_id: &str,
        approval_id: &str,
        resolution: &TaskAuthorizationResolution,
    ) -> Result<Task, TaskEngineError> {
        let key = scope.task_key(task_id);
        let status_event_id = uuid::Uuid::now_v7().to_string();
        for _ in 0..MAX_CAS_RETRY_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                return Err(TaskEngineError::NotFound(task_id.to_string()));
            };
            let mut record = DurableTaskRecord::decode(&raw)?;
            let task = &mut record.task;
            if task.status.state == TaskState::Working && task.pending_approval_id.is_none() {
                return Ok(task.clone());
            }
            if task.status.state != TaskState::AuthRequired
                || task.pending_approval_id.as_deref() != Some(approval_id)
            {
                return Err(TaskEngineError::ChallengeMismatch {
                    task_id: task_id.to_string(),
                    approval_id: approval_id.to_string(),
                });
            }
            let now = self.clock.now();
            if resolution.valid_until <= now {
                return Err(TaskEngineError::AuthorizationEvidenceExpired);
            }
            task.transition_to_at(TaskState::Working, None, now)?;
            task.updated_at = now;
            let task = record.task.clone();
            record.delivery.append(
                status_event_id.clone(),
                now,
                project_task_status_push_payload(&task, TaskState::Working, now)?,
            )?;
            let payload = record.encode()?;
            if matches!(
                self.state
                    .compare_and_swap(&key, version, &payload, None)
                    .await?,
                CasResult::Ok
            ) {
                self.sync_push_ready_hint_best_effort(&record).await;
                self.emit_audit(
                    &task,
                    "resolve_authorization",
                    Some(TaskState::AuthRequired),
                )
                .await;
                self.emit_stream(
                    &scope.namespace,
                    &scope.tenant,
                    task_id,
                    acteon_core::StreamEventType::TaskTransitioned {
                        task_id: task_id.to_string(),
                        from: TaskState::AuthRequired,
                        to: TaskState::Working,
                    },
                );
                return Ok(task.clone());
            }
        }
        Err(TaskEngineError::CasExhausted(task_id.to_string()))
    }

    async fn finish_authorization_resolution(
        &self,
        scope: &TaskScope,
        task_id: &str,
        approval_id: &str,
        resolution: &TaskAuthorizationResolution,
    ) -> Result<BusApproval, TaskEngineError> {
        let key = StateKey::new(
            scope.namespace.clone(),
            scope.tenant.clone(),
            KeyKind::BusApproval,
            approval_id,
        );
        for _ in 0..MAX_CAS_RETRY_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                return Err(TaskEngineError::ChallengeNotFound(approval_id.to_string()));
            };
            let mut approval: BusApproval = serde_json::from_str(&raw)?;
            Self::validate_authorization_approval(scope, task_id, approval_id, &approval)?;
            if !approval
                .authorization_resolution
                .as_ref()
                .is_some_and(|claimed| Self::same_authorization_decision(claimed, resolution))
            {
                return Err(TaskEngineError::AuthorizationDecisionConflict(
                    approval_id.to_string(),
                ));
            }
            match approval.status {
                BusApprovalStatus::Approved => return Ok(approval),
                BusApprovalStatus::Approving => {
                    approval.status = BusApprovalStatus::Approved;
                    approval.validate()?;
                    let payload = serde_json::to_string(&approval)?;
                    if matches!(
                        self.state
                            .compare_and_swap(&key, version, &payload, None)
                            .await?,
                        CasResult::Ok
                    ) {
                        return Ok(approval);
                    }
                }
                status => {
                    return Err(TaskEngineError::ChallengeClosed {
                        approval_id: approval_id.to_string(),
                        status,
                    });
                }
            }
        }
        Err(TaskEngineError::ChallengeCasExhausted(
            approval_id.to_string(),
        ))
    }

    async fn reject_abandoned_authorization_resolution(
        &self,
        scope: &TaskScope,
        task_id: &str,
        approval_id: &str,
        resolution: &TaskAuthorizationResolution,
    ) -> Result<(), TaskEngineError> {
        let key = StateKey::new(
            scope.namespace.clone(),
            scope.tenant.clone(),
            KeyKind::BusApproval,
            approval_id,
        );
        for _ in 0..MAX_CAS_RETRY_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                return Ok(());
            };
            let mut approval: BusApproval = serde_json::from_str(&raw)?;
            Self::validate_authorization_approval(scope, task_id, approval_id, &approval)?;
            match approval.status {
                BusApprovalStatus::Approving
                    if approval
                        .authorization_resolution
                        .as_ref()
                        .is_some_and(|claimed| {
                            Self::same_authorization_decision(claimed, resolution)
                        }) =>
                {
                    approval.status = BusApprovalStatus::Rejected;
                    approval.authorization_resolution = None;
                    approval.validate()?;
                    let payload = serde_json::to_string(&approval)?;
                    if matches!(
                        self.state
                            .compare_and_swap(&key, version, &payload, None)
                            .await?,
                        CasResult::Ok
                    ) {
                        self.remove_pending_challenge(scope, approval_id).await;
                        return Ok(());
                    }
                }
                BusApprovalStatus::Approving => {
                    return Err(TaskEngineError::AuthorizationDecisionConflict(
                        approval_id.to_string(),
                    ));
                }
                _ => return Ok(()),
            }
        }
        Err(TaskEngineError::ChallengeCasExhausted(
            approval_id.to_string(),
        ))
    }

    async fn fail_expired_authorization_challenge(
        &self,
        scope: &TaskScope,
        task_id: &str,
        approval_id: &str,
    ) -> Result<(), TaskEngineError> {
        let key = scope.task_key(task_id);
        let status_event_id = uuid::Uuid::now_v7().to_string();
        for _ in 0..MAX_CAS_RETRY_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                return Ok(());
            };
            let mut record = DurableTaskRecord::decode(&raw)?;
            let task = &mut record.task;
            if task.status.state != TaskState::AuthRequired
                || task.pending_approval_id.as_deref() != Some(approval_id)
            {
                return Ok(());
            }
            let mut message = TaskMessage::text(
                format!("{approval_id}.expired"),
                TaskRole::Agent,
                "The authorization challenge expired before it was verified.",
            );
            message.task_id = Some(task_id.to_string());
            message.context_id.clone_from(&task.context_id);
            let now = self.clock.now();
            task.transition_to_at(TaskState::Failed, Some(message), now)?;
            let task = record.task.clone();
            record.delivery.append(
                status_event_id.clone(),
                now,
                project_task_status_push_payload(&task, TaskState::Failed, now)?,
            )?;
            let payload = record.encode()?;
            if matches!(
                self.state
                    .compare_and_swap(&key, version, &payload, None)
                    .await?,
                CasResult::Ok
            ) {
                self.sync_push_ready_hint_best_effort(&record).await;
                self.emit_audit(
                    &task,
                    "authorization_challenge_expired",
                    Some(TaskState::AuthRequired),
                )
                .await;
                self.emit_stream(
                    &scope.namespace,
                    &scope.tenant,
                    task_id,
                    acteon_core::StreamEventType::TaskTransitioned {
                        task_id: task_id.to_string(),
                        from: TaskState::AuthRequired,
                        to: TaskState::Failed,
                    },
                );
                return Ok(());
            }
        }
        Err(TaskEngineError::CasExhausted(task_id.to_string()))
    }

    async fn remove_pending_challenge(&self, scope: &TaskScope, approval_id: &str) {
        let index_key = StateKey::new(
            scope.namespace.clone(),
            scope.tenant.clone(),
            KeyKind::PendingBusApprovals,
            approval_id,
        );
        if let Err(error) = self.state.delete(&index_key).await {
            warn!(%approval_id, %error, "failed to remove resolved task challenge from pending index");
        }
    }

    async fn prepare_input_resolution(
        &self,
        scope: &TaskScope,
        task_id: &str,
        approval_id: &str,
        message: &TaskMessage,
    ) -> Result<TaskPauseResolution, TaskEngineError> {
        if message.task_id.as_deref() != Some(task_id) {
            return Err(TaskEngineError::ChallengeResponseTaskMismatch);
        }
        if message.role != TaskRole::User {
            return Err(TaskEngineError::ChallengeResponseRole);
        }
        message.validate_in_task(task_id)?;
        let resolution = TaskPauseResolution {
            message_id: message.message_id.clone(),
            content_digest: task_message_digest(message)?,
        };

        let current = self
            .get_task(scope, task_id)
            .await?
            .ok_or_else(|| TaskEngineError::NotFound(task_id.to_string()))?;
        match current.status.state {
            TaskState::InputRequired | TaskState::AuthRequired
                if current.pending_approval_id.as_deref() == Some(approval_id) => {}
            TaskState::Working if current.pending_approval_id.is_none() => {}
            _ => {
                let _ = self
                    .reject_abandoned_input_resolution(scope, task_id, approval_id, &resolution)
                    .await;
                return Err(TaskEngineError::ChallengeMismatch {
                    task_id: task_id.to_string(),
                    approval_id: approval_id.to_string(),
                });
            }
        }
        if message.context_id != current.context_id {
            let _ = self
                .reject_abandoned_input_resolution(scope, task_id, approval_id, &resolution)
                .await;
            return Err(TaskEngineError::ChallengeResponseContextMismatch);
        }

        let seed: Vec<&str> = message
            .reference_task_ids
            .iter()
            .map(String::as_str)
            .collect();
        self.check_reference_graph(scope, task_id, &seed).await?;

        if current.status.state == TaskState::InputRequired
            && current
                .history
                .iter()
                .any(|item| item.message_id == resolution.message_id)
        {
            return Err(TaskEngineError::ChallengeResponseConflict(
                approval_id.to_string(),
            ));
        }
        if current.status.state == TaskState::Working {
            let Some(applied) = current
                .history
                .iter()
                .find(|item| item.message_id == resolution.message_id)
            else {
                return Err(TaskEngineError::ChallengeMismatch {
                    task_id: task_id.to_string(),
                    approval_id: approval_id.to_string(),
                });
            };
            if task_message_digest(applied)? != resolution.content_digest {
                return Err(TaskEngineError::ChallengeResponseConflict(
                    approval_id.to_string(),
                ));
            }
        }
        Ok(resolution)
    }

    async fn claim_input_resolution(
        &self,
        scope: &TaskScope,
        task_id: &str,
        approval_id: &str,
        resolution: &TaskPauseResolution,
        actor: &str,
    ) -> Result<BusApproval, TaskEngineError> {
        let key = StateKey::new(
            scope.namespace.clone(),
            scope.tenant.clone(),
            KeyKind::BusApproval,
            approval_id,
        );
        for _ in 0..MAX_CAS_RETRY_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                return Err(TaskEngineError::ChallengeNotFound(approval_id.to_string()));
            };
            let mut approval: BusApproval = serde_json::from_str(&raw)?;
            Self::validate_input_approval(scope, task_id, approval_id, &approval)?;
            match approval.status {
                BusApprovalStatus::Pending => {
                    let now = self.clock.now();
                    if now >= approval.expires_at {
                        approval.status = BusApprovalStatus::Expired;
                        approval.decided_at = Some(now);
                        let payload = serde_json::to_string(&approval)?;
                        if matches!(
                            self.state
                                .compare_and_swap(&key, version, &payload, None)
                                .await?,
                            CasResult::Ok
                        ) {
                            let index_key = StateKey::new(
                                scope.namespace.clone(),
                                scope.tenant.clone(),
                                KeyKind::PendingBusApprovals,
                                approval_id,
                            );
                            let _ = self.state.delete(&index_key).await;
                            return Err(TaskEngineError::ChallengeExpired(approval_id.to_string()));
                        }
                        continue;
                    }
                    approval.status = BusApprovalStatus::Approving;
                    approval.task_resolution = Some(resolution.clone());
                    approval.decided_by = Some(actor.to_string());
                    approval.decided_at = Some(now);
                    approval.validate()?;
                    let payload = serde_json::to_string(&approval)?;
                    if matches!(
                        self.state
                            .compare_and_swap(&key, version, &payload, None)
                            .await?,
                        CasResult::Ok
                    ) {
                        return Ok(approval);
                    }
                }
                BusApprovalStatus::Approving | BusApprovalStatus::Approved => {
                    if approval.task_resolution.as_ref() != Some(resolution) {
                        return Err(TaskEngineError::ChallengeResponseConflict(
                            approval_id.to_string(),
                        ));
                    }
                    return Ok(approval);
                }
                BusApprovalStatus::Rejected | BusApprovalStatus::Expired => {
                    return Err(TaskEngineError::ChallengeClosed {
                        approval_id: approval_id.to_string(),
                        status: approval.status,
                    });
                }
            }
        }
        Err(TaskEngineError::ChallengeCasExhausted(
            approval_id.to_string(),
        ))
    }

    fn validate_input_approval(
        scope: &TaskScope,
        task_id: &str,
        approval_id: &str,
        approval: &BusApproval,
    ) -> Result<(), TaskEngineError> {
        approval.validate()?;
        if approval.approval_id != approval_id
            || approval.namespace != scope.namespace
            || approval.tenant != scope.tenant
            || approval.task_id.as_deref() != Some(task_id)
        {
            return Err(TaskEngineError::ChallengeMismatch {
                task_id: task_id.to_string(),
                approval_id: approval_id.to_string(),
            });
        }
        if approval.kind != PauseKind::UserInput {
            return Err(TaskEngineError::ChallengeKind {
                approval_id: approval_id.to_string(),
                actual: approval.kind,
            });
        }
        Ok(())
    }

    async fn apply_input_resolution(
        &self,
        scope: &TaskScope,
        task_id: &str,
        approval_id: &str,
        resolution: &TaskPauseResolution,
        message: TaskMessage,
    ) -> Result<Task, TaskEngineError> {
        let key = scope.task_key(task_id);
        let status_event_id = uuid::Uuid::now_v7().to_string();
        for _ in 0..MAX_CAS_RETRY_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                return Err(TaskEngineError::NotFound(task_id.to_string()));
            };
            let mut record = DurableTaskRecord::decode(&raw)?;
            let task = &mut record.task;
            if task.context_id != message.context_id {
                return Err(TaskEngineError::ChallengeResponseContextMismatch);
            }
            if task.status.state == TaskState::Working && task.pending_approval_id.is_none() {
                let Some(applied) = task
                    .history
                    .iter()
                    .find(|item| item.message_id == resolution.message_id)
                else {
                    return Err(TaskEngineError::ChallengeMismatch {
                        task_id: task_id.to_string(),
                        approval_id: approval_id.to_string(),
                    });
                };
                if task_message_digest(applied)? != resolution.content_digest {
                    return Err(TaskEngineError::ChallengeResponseConflict(
                        approval_id.to_string(),
                    ));
                }
                return Ok(task.clone());
            }
            if task.status.state != TaskState::InputRequired
                || task.pending_approval_id.as_deref() != Some(approval_id)
            {
                return Err(TaskEngineError::ChallengeMismatch {
                    task_id: task_id.to_string(),
                    approval_id: approval_id.to_string(),
                });
            }
            if task
                .history
                .iter()
                .any(|item| item.message_id == resolution.message_id)
            {
                return Err(TaskEngineError::ChallengeResponseConflict(
                    approval_id.to_string(),
                ));
            }
            task.append_history_at(message.clone(), self.clock.now())?;
            let now = self.clock.now();
            task.transition_to_at(TaskState::Working, None, now)?;
            task.updated_at = now;
            let task = record.task.clone();
            record.delivery.append(
                status_event_id.clone(),
                now,
                project_task_status_push_payload(&task, TaskState::Working, now)?,
            )?;
            let payload = record.encode()?;
            if matches!(
                self.state
                    .compare_and_swap(&key, version, &payload, None)
                    .await?,
                CasResult::Ok
            ) {
                self.sync_push_ready_hint_best_effort(&record).await;
                self.emit_audit(&task, "resolve_input", Some(TaskState::InputRequired))
                    .await;
                self.emit_stream(
                    &scope.namespace,
                    &scope.tenant,
                    task_id,
                    acteon_core::StreamEventType::TaskHistoryAppended {
                        task_id: task_id.to_string(),
                        message_id: resolution.message_id.clone(),
                    },
                );
                self.emit_stream(
                    &scope.namespace,
                    &scope.tenant,
                    task_id,
                    acteon_core::StreamEventType::TaskTransitioned {
                        task_id: task_id.to_string(),
                        from: TaskState::InputRequired,
                        to: TaskState::Working,
                    },
                );
                return Ok(task.clone());
            }
        }
        Err(TaskEngineError::CasExhausted(task_id.to_string()))
    }

    async fn finish_input_resolution(
        &self,
        scope: &TaskScope,
        task_id: &str,
        approval_id: &str,
        resolution: &TaskPauseResolution,
    ) -> Result<BusApproval, TaskEngineError> {
        let key = StateKey::new(
            scope.namespace.clone(),
            scope.tenant.clone(),
            KeyKind::BusApproval,
            approval_id,
        );
        for _ in 0..MAX_CAS_RETRY_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                return Err(TaskEngineError::ChallengeNotFound(approval_id.to_string()));
            };
            let mut approval: BusApproval = serde_json::from_str(&raw)?;
            Self::validate_input_approval(scope, task_id, approval_id, &approval)?;
            if approval.task_resolution.as_ref() != Some(resolution) {
                return Err(TaskEngineError::ChallengeResponseConflict(
                    approval_id.to_string(),
                ));
            }
            match approval.status {
                BusApprovalStatus::Approved => return Ok(approval),
                BusApprovalStatus::Approving => {
                    approval.status = BusApprovalStatus::Approved;
                    approval.validate()?;
                    let payload = serde_json::to_string(&approval)?;
                    if matches!(
                        self.state
                            .compare_and_swap(&key, version, &payload, None)
                            .await?,
                        CasResult::Ok
                    ) {
                        return Ok(approval);
                    }
                }
                status => {
                    return Err(TaskEngineError::ChallengeClosed {
                        approval_id: approval_id.to_string(),
                        status,
                    });
                }
            }
        }
        Err(TaskEngineError::ChallengeCasExhausted(
            approval_id.to_string(),
        ))
    }

    async fn reject_abandoned_input_resolution(
        &self,
        scope: &TaskScope,
        task_id: &str,
        approval_id: &str,
        resolution: &TaskPauseResolution,
    ) -> Result<(), TaskEngineError> {
        let key = StateKey::new(
            scope.namespace.clone(),
            scope.tenant.clone(),
            KeyKind::BusApproval,
            approval_id,
        );
        for _ in 0..MAX_CAS_RETRY_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                return Ok(());
            };
            let mut approval: BusApproval = serde_json::from_str(&raw)?;
            Self::validate_input_approval(scope, task_id, approval_id, &approval)?;
            match approval.status {
                BusApprovalStatus::Approving
                    if approval.task_resolution.as_ref() == Some(resolution) =>
                {
                    approval.status = BusApprovalStatus::Rejected;
                    approval.task_resolution = None;
                    approval.validate()?;
                    let payload = serde_json::to_string(&approval)?;
                    if matches!(
                        self.state
                            .compare_and_swap(&key, version, &payload, None)
                            .await?,
                        CasResult::Ok
                    ) {
                        let index_key = StateKey::new(
                            scope.namespace.clone(),
                            scope.tenant.clone(),
                            KeyKind::PendingBusApprovals,
                            approval_id,
                        );
                        let _ = self.state.delete(&index_key).await;
                        return Ok(());
                    }
                }
                BusApprovalStatus::Approving => {
                    return Err(TaskEngineError::ChallengeResponseConflict(
                        approval_id.to_string(),
                    ));
                }
                _ => return Ok(()),
            }
        }
        Err(TaskEngineError::ChallengeCasExhausted(
            approval_id.to_string(),
        ))
    }

    async fn fail_expired_input_challenge(
        &self,
        scope: &TaskScope,
        task_id: &str,
        approval_id: &str,
    ) -> Result<(), TaskEngineError> {
        let key = scope.task_key(task_id);
        let status_event_id = uuid::Uuid::now_v7().to_string();
        for _ in 0..MAX_CAS_RETRY_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                return Ok(());
            };
            let mut record = DurableTaskRecord::decode(&raw)?;
            let task = &mut record.task;
            if task.status.state != TaskState::InputRequired
                || task.pending_approval_id.as_deref() != Some(approval_id)
            {
                return Ok(());
            }
            let mut message = TaskMessage::text(
                format!("{approval_id}.expired"),
                TaskRole::Agent,
                "The required-input challenge expired before it was resolved.",
            );
            message.task_id = Some(task_id.to_string());
            message.context_id.clone_from(&task.context_id);
            let now = self.clock.now();
            task.transition_to_at(TaskState::Failed, Some(message), now)?;
            let task = record.task.clone();
            record.delivery.append(
                status_event_id.clone(),
                now,
                project_task_status_push_payload(&task, TaskState::Failed, now)?,
            )?;
            let payload = record.encode()?;
            if matches!(
                self.state
                    .compare_and_swap(&key, version, &payload, None)
                    .await?,
                CasResult::Ok
            ) {
                self.sync_push_ready_hint_best_effort(&record).await;
                self.emit_audit(
                    &task,
                    "input_challenge_expired",
                    Some(TaskState::InputRequired),
                )
                .await;
                self.emit_stream(
                    &scope.namespace,
                    &scope.tenant,
                    task_id,
                    acteon_core::StreamEventType::TaskTransitioned {
                        task_id: task_id.to_string(),
                        from: TaskState::InputRequired,
                        to: TaskState::Failed,
                    },
                );
                return Ok(());
            }
        }
        Err(TaskEngineError::CasExhausted(task_id.to_string()))
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
        self.cas_mutate(
            &key,
            task_id,
            "link_chain",
            None,
            move |task: &mut Task, _now| {
                task.chain_id.clone_from(&chain_id);
                Ok(())
            },
        )
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
            .cas_mutate(
                &key,
                task_id,
                "artifact_upsert",
                Some((artifact_id_for_emit.clone(), false)),
                move |task: &mut Task, now| {
                    task.upsert_artifact_at(artifact.clone(), append, now)?;
                    Ok(())
                },
            )
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

    /// Check every read and CAS retry against the immutable acceptance identity.
    pub(crate) fn validate_governed_projection(
        task: &Task,
        expected: &Task,
    ) -> Result<(), TaskValidationError> {
        task.validate()?;
        if task.id != expected.id
            || task.namespace != expected.namespace
            || task.tenant != expected.tenant
            || task.context_id != expected.context_id
            || task.chain_id.is_some()
            || task
                .metadata
                .get(crate::agent_runtime::GOVERNED_TASK_METADATA_KEY)
                != expected
                    .metadata
                    .get(crate::agent_runtime::GOVERNED_TASK_METADATA_KEY)
            || task
                .history
                .first()
                .map(serde_json::to_value)
                .transpose()
                .map_err(|_| TaskValidationError::PartDataInvalid)?
                != expected
                    .history
                    .first()
                    .map(serde_json::to_value)
                    .transpose()
                    .map_err(|_| TaskValidationError::PartDataInvalid)?
            || task
                .artifacts
                .iter()
                .any(|artifact| artifact.artifact_id != "governed-result")
        {
            return Err(TaskValidationError::IdentityMismatch);
        }
        Ok(())
    }

    /// Idempotent runtime projection; only its governed executor can start effects.
    pub(crate) async fn start_governed_task(
        &self,
        scope: &TaskScope,
        expected: &Task,
    ) -> Result<Task, TaskEngineError> {
        let task_id = expected.id.as_str();
        let key = scope.task_key(task_id);
        let mut transitioned = false;
        let task = self
            .cas_mutate(&key, task_id, "governed_working", None, |task, now| {
                Self::validate_governed_projection(task, expected)?;
                if task.status.state.is_terminal() {
                    transitioned = false;
                    return Ok(());
                }
                if !matches!(task.status.state, TaskState::Submitted | TaskState::Working) {
                    return Err(TaskValidationError::IllegalTransition {
                        from: task.status.state,
                        to: TaskState::Working,
                    });
                }
                transitioned = task.status.state != TaskState::Working;
                if transitioned {
                    task.transition_to_at(TaskState::Working, None, now)?;
                }
                Ok(())
            })
            .await?;
        if transitioned {
            self.emit_stream(
                &scope.namespace,
                &scope.tenant,
                task_id,
                acteon_core::StreamEventType::TaskTransitioned {
                    task_id: task_id.into(),
                    from: TaskState::Submitted,
                    to: TaskState::Working,
                },
            );
        }
        Ok(task)
    }

    /// Atomically project known governed completion and its result artifact.
    /// This host-only helper does not authorize execution. Same-result concurrent
    /// observers may repair/replay the projection without a second transition.
    pub(crate) async fn project_governed_result(
        &self,
        scope: &TaskScope,
        expected: &Task,
        next: TaskState,
        artifact: Artifact,
    ) -> Result<Task, TaskEngineError> {
        let task_id = expected.id.as_str();
        let key = scope.task_key(task_id);
        let mut transitioned = false;
        let artifact_id = artifact.artifact_id.clone();
        let task = self
            .cas_mutate(
                &key,
                task_id,
                "governed_result",
                Some((artifact_id.clone(), true)),
                |task, now| {
                    Self::validate_governed_projection(task, expected)?;
                    transitioned = task.status.state != next;
                    if transitioned {
                        // A missing projection is rebuilt as Submitted. A durable,
                        // qualified completion receipt authorizes repairing both
                        // transitions in this CAS without starting another effect.
                        if task.status.state == TaskState::Submitted {
                            task.transition_to_at(TaskState::Working, None, now)?;
                        }
                        task.transition_to_at(next, None, now)?;
                    }
                    task.upsert_artifact_at(artifact.clone(), false, now)?;
                    Ok(())
                },
            )
            .await?;
        self.emit_stream(
            &scope.namespace,
            &scope.tenant,
            task_id,
            acteon_core::StreamEventType::TaskArtifactUpdated {
                task_id: task_id.into(),
                artifact_id,
                last_chunk: true,
            },
        );
        if transitioned {
            self.emit_stream(
                &scope.namespace,
                &scope.tenant,
                task_id,
                acteon_core::StreamEventType::TaskTransitioned {
                    task_id: task_id.into(),
                    from: TaskState::Working,
                    to: next,
                },
            );
        }
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
        artifact_event: Option<(String, bool)>,
        mut mutate: F,
    ) -> Result<Task, TaskEngineError>
    where
        F: FnMut(&mut Task, DateTime<Utc>) -> Result<(), TaskValidationError>,
    {
        let status_event_id = uuid::Uuid::now_v7().to_string();
        let artifact_event_id = uuid::Uuid::now_v7().to_string();
        for _ in 0..MAX_CAS_RETRY_ATTEMPTS {
            if self
                .hidden_governed_task(
                    &TaskScope::new(key.namespace.as_str(), key.tenant.as_str()),
                    task_id,
                )
                .await?
            {
                return Err(TaskEngineError::NotFound(task_id.into()));
            }
            let Some((raw, version)) = self.state.get_versioned(key).await? else {
                return Err(TaskEngineError::NotFound(task_id.to_string()));
            };
            let mut record = DurableTaskRecord::decode(&raw)?;
            let task = &mut record.task;
            let from_state = task.status.state;
            let artifact_before = artifact_event
                .as_ref()
                .and_then(|(artifact_id, _)| {
                    task.artifacts
                        .iter()
                        .find(|artifact| artifact.artifact_id == *artifact_id)
                })
                .map(serde_json::to_value)
                .transpose()?;
            let now = self.clock.now();
            mutate(task, now)?;
            task.updated_at = now;
            let task = record.task.clone();
            if let Some((artifact_id, last_chunk)) = &artifact_event {
                let artifact_after = task
                    .artifacts
                    .iter()
                    .find(|artifact| artifact.artifact_id == *artifact_id)
                    .map(serde_json::to_value)
                    .transpose()?;
                if artifact_before != artifact_after {
                    let payload =
                        project_task_artifact_push_payload(&task, artifact_id, *last_chunk)?;
                    record
                        .delivery
                        .append(artifact_event_id.clone(), now, payload)?;
                }
            }
            if from_state != task.status.state {
                record.delivery.append(
                    status_event_id.clone(),
                    now,
                    project_task_status_push_payload(&task, task.status.state, now)?,
                )?;
            }
            let payload = record.encode()?;
            match self
                .state
                .compare_and_swap(key, version, &payload, None)
                .await?
            {
                CasResult::Ok => {
                    if !record.delivery.callbacks.is_empty() {
                        self.sync_push_ready_hint_best_effort(&record).await;
                    }
                    debug!(task_id = %task_id, state = ?task.status.state, "task mutated");
                    self.emit_audit(&task, operation, Some(from_state)).await;
                    return Ok(task.clone());
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
        let now = self.clock.now().timestamp_millis().to_string();
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
    #[error("audit error: {0}")]
    Audit(#[from] AuditError),
    #[error("validation error: {0}")]
    Validation(#[from] TaskValidationError),
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("unsupported durable task record version {0}")]
    UnsupportedRecordVersion(u32),
    #[error("invalid durable task record: {0}")]
    InvalidDurableRecord(String),
    #[error("durable task delivery log is full; callback acknowledgement is required")]
    DeliveryLogFull,
    #[error("push configuration identity does not match its task scope")]
    PushConfigIdentityMismatch,
    #[error("push configuration '{0}' already exists in the initial task envelope")]
    PushConfigAlreadyExists(String),
    #[error("task callback configuration capacity is limited to {MAX_DURABLE_TASK_CALLBACKS}")]
    PushConfigCapacityExceeded,
    #[error("push configuration '{0}' was not found")]
    PushConfigNotFound(String),
    #[error("push configuration cannot be updated while delivery is in flight")]
    PushConfigDeliveryInFlight,
    #[error("push delivery worker id is invalid")]
    InvalidPushWorkerId,
    #[error("push delivery lease duration must be between 1ns and 5 minutes")]
    InvalidPushLeaseDuration,
    #[error("push delivery retry time must not be in the past")]
    InvalidPushRetryTime,
    #[error("push delivery lease was lost or replaced")]
    PushDeliveryLeaseLost,
    #[error("push delivery sequence is exhausted")]
    PushDeliverySequenceExhausted,
    #[error("push delivery attempt counter is exhausted")]
    PushDeliveryAttemptExhausted,
    #[error("task callback dead-letter capacity is exhausted")]
    PushDeadLetterCapacityExceeded,
    #[error("task callback dead letter '{0}' was not found")]
    PushDeadLetterNotFound(String),
    #[error("push configuration validation failed: {0}")]
    PushConfigValidation(#[from] TaskPushConfigValidationError),
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
    #[error("task challenge '{0}' not found")]
    ChallengeNotFound(String),
    #[error("challenge '{approval_id}' is not the active challenge for task '{task_id}'")]
    ChallengeMismatch {
        task_id: String,
        approval_id: String,
    },
    #[error("challenge '{approval_id}' has kind {actual:?}; structured input requires UserInput")]
    ChallengeKind {
        approval_id: String,
        actual: PauseKind,
    },
    #[error("challenge '{approval_id}' is closed with status {status:?}")]
    ChallengeClosed {
        approval_id: String,
        status: BusApprovalStatus,
    },
    #[error("challenge '{0}' expired before it was claimed")]
    ChallengeExpired(String),
    #[error("challenge '{0}' was already claimed by a different response")]
    ChallengeResponseConflict(String),
    #[error("challenge response taskId must match the task being resumed")]
    ChallengeResponseTaskMismatch,
    #[error("challenge response contextId must exactly match the paused task")]
    ChallengeResponseContextMismatch,
    #[error("challenge response must have role 'user'")]
    ChallengeResponseRole,
    #[error("authorization challenge '{0}' has no host-owned verifier binding")]
    AuthorizationChallengeUnbound(String),
    #[error("UserAuth pauses require a host-owned authorization requirement")]
    AuthorizationRequirementRequired,
    #[error(
        "authorization challenge requires verifier '{expected_id}' revision {expected_revision}"
    )]
    AuthorizationVerifierMismatch {
        expected_id: String,
        expected_revision: u64,
    },
    #[error("authorization verifier evidence is not currently valid")]
    AuthorizationEvidenceExpired,
    #[error("authorization challenge '{0}' was claimed by a different verifier decision")]
    AuthorizationDecisionConflict(String),
    #[error("authorization verification failed: {0}")]
    AuthorizationVerification(#[from] TaskAuthorizationVerificationError),
    #[error("CAS contention exceeded {MAX_CAS_RETRY_ATTEMPTS} retries for challenge '{0}'")]
    ChallengeCasExhausted(String),
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

    pub async fn resolve_authorization(
        &self,
        task_id: &str,
        approval_id: &str,
        verifier: &dyn TaskAuthorizationVerifier,
    ) -> Result<(Task, BusApproval), TaskEngineError> {
        self.engine
            .resolve_authorization(&self.scope, task_id, approval_id, verifier)
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

    pub async fn pause_for_authorization(
        &self,
        task_id: &str,
        requirement: TaskAuthorizationRequirement,
        reason: Option<String>,
        ttl: Option<Duration>,
    ) -> Result<(Task, BusApproval), TaskEngineError> {
        self.engine
            .pause_for_authorization(&self.scope, task_id, requirement, reason, ttl)
            .await
    }

    pub async fn resolve_input(
        &self,
        task_id: &str,
        approval_id: &str,
        message: TaskMessage,
        decided_by: impl Into<String>,
    ) -> Result<(Task, BusApproval), TaskEngineError> {
        self.engine
            .resolve_input(&self.scope, task_id, approval_id, message, decided_by)
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
    use std::{
        collections::HashMap,
        sync::{
            Mutex,
            atomic::{AtomicBool, Ordering},
        },
    };

    use super::*;
    use acteon_core::{Artifact, TaskPart as Part, TaskRole as Role};
    use acteon_state::testing::faults::{FaultStore, FaultTiming, ReadOperation, WriteOperation};
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

    fn test_push_config(task: &Task, id: &str) -> TaskPushNotificationConfig {
        TaskPushNotificationConfig::new(
            id,
            &task.id,
            &task.namespace,
            &task.tenant,
            format!("https://{id}.example/a2a"),
        )
    }

    fn test_callback(task: &Task, acknowledged_sequence: u64) -> DurableTaskCallback {
        DurableTaskCallback::new(test_push_config(task, "callback"), acknowledged_sequence)
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
    async fn create_persists_private_versioned_envelope() {
        let state = Arc::new(MemoryStateStore::new());
        let e = TaskEngine::new(state.clone());
        e.create_task(sample_task("t1")).await.unwrap();

        let raw = state.get(&scope().task_key("t1")).await.unwrap().unwrap();
        let stored: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(stored["recordVersion"], 1);
        assert_eq!(stored["task"]["id"], "t1");
        assert_eq!(
            stored["delivery"],
            serde_json::json!({"legacyConfigScanComplete": true, "nextSequence": 0})
        );
        assert!(stored.get("id").is_none(), "Task must remain a projection");
    }

    #[tokio::test]
    async fn create_commits_inline_callback_with_the_task() {
        let state = Arc::new(MemoryStateStore::new());
        let e = TaskEngine::new(state.clone());
        let task = sample_task("t1");
        let config = test_push_config(&task, "inline");
        e.create_task_with_push_configs(task, vec![config.clone()])
            .await
            .unwrap();

        assert_eq!(
            e.get_push_config(&scope(), "t1", "inline")
                .await
                .unwrap()
                .unwrap()
                .url,
            config.url
        );
        let record =
            DurableTaskRecord::decode(&state.get(&scope().task_key("t1")).await.unwrap().unwrap())
                .unwrap();
        assert_eq!(record.delivery.callbacks["inline"].acknowledged_sequence, 0);
    }

    #[tokio::test]
    async fn callback_upsert_starts_at_tail_and_update_preserves_cursor_and_creation() {
        let state = Arc::new(MemoryStateStore::new());
        let e = TaskEngine::new(state.clone());
        let task = sample_task("t1");
        e.create_task(task.clone()).await.unwrap();
        e.transition_task(&scope(), "t1", TaskState::Working, None)
            .await
            .unwrap();

        let original = test_push_config(&task, "callback");
        let saved = e
            .upsert_push_config(&scope(), "t1", original.clone())
            .await
            .unwrap();
        let mut updated = original;
        updated.url = "https://replacement.example/a2a".into();
        updated.updated_at += chrono::Duration::seconds(1);
        let saved_update = e.upsert_push_config(&scope(), "t1", updated).await.unwrap();

        assert_eq!(saved_update.created_at, saved.created_at);
        assert_eq!(saved_update.url, "https://replacement.example/a2a");
        let record =
            DurableTaskRecord::decode(&state.get(&scope().task_key("t1")).await.unwrap().unwrap())
                .unwrap();
        assert_eq!(
            record.delivery.callbacks["callback"].acknowledged_sequence,
            1
        );
        assert_eq!(e.list_push_configs(&scope(), "t1").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn callback_delete_releases_its_delivery_retention_floor() {
        let state = Arc::new(MemoryStateStore::new());
        let e = TaskEngine::new(state.clone());
        let task = sample_task("t1");
        e.create_task_with_push_configs(task.clone(), vec![test_push_config(&task, "callback")])
            .await
            .unwrap();
        e.transition_task(&scope(), "t1", TaskState::Working, None)
            .await
            .unwrap();

        assert!(
            e.delete_push_config(&scope(), "t1", "callback")
                .await
                .unwrap()
        );
        assert!(
            e.get_push_config(&scope(), "t1", "callback")
                .await
                .unwrap()
                .is_none()
        );
        let record =
            DurableTaskRecord::decode(&state.get(&scope().task_key("t1")).await.unwrap().unwrap())
                .unwrap();
        assert!(record.delivery.events.is_empty());
        assert!(
            !e.delete_push_config(&scope(), "t1", "callback")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn callback_capacity_bounds_the_task_envelope() {
        let e = engine();
        let task = sample_task("t1");
        e.create_task(task.clone()).await.unwrap();
        for index in 0..MAX_DURABLE_TASK_CALLBACKS {
            e.upsert_push_config(
                &scope(),
                "t1",
                test_push_config(&task, &format!("callback-{index}")),
            )
            .await
            .unwrap();
        }
        let error = e
            .upsert_push_config(&scope(), "t1", test_push_config(&task, "overflow"))
            .await
            .unwrap_err();
        assert!(matches!(error, TaskEngineError::PushConfigCapacityExceeded));
        assert_eq!(
            e.list_push_configs(&scope(), "t1").await.unwrap().len(),
            MAX_DURABLE_TASK_CALLBACKS
        );
    }

    #[tokio::test]
    async fn callback_ready_hint_tracks_claim_retry_ack_and_repairs_loss() {
        let epoch = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let clock = Arc::new(acteon_time::ManualClock::new(epoch));
        let state = Arc::new(MemoryStateStore::new());
        let e = TaskEngine::new(state.clone()).with_clock(clock.clone());
        let task = Task::new_at("ready", "agents", "demo", epoch);
        e.create_task_with_push_configs(task.clone(), vec![test_push_config(&task, "callback")])
            .await
            .unwrap();
        let ready_key = scope().push_ready_key("ready");
        assert!(state.get(&ready_key).await.unwrap().is_none());

        e.transition_task(&scope(), "ready", TaskState::Working, None)
            .await
            .unwrap();
        let hint: TaskPushReadyHint =
            serde_json::from_str(&state.get(&ready_key).await.unwrap().expect("ready hint"))
                .unwrap();
        assert_eq!(hint.task_id, "ready");
        assert_eq!(hint.ready_at, epoch);

        state.delete(&ready_key).await.unwrap();
        assert!(
            e.repair_push_ready_hint(&scope(), "ready")
                .await
                .unwrap()
                .is_some()
        );
        let claim = e
            .claim_push_delivery(
                &scope(),
                "ready",
                "callback",
                "worker",
                Duration::from_secs(30),
            )
            .await
            .unwrap()
            .unwrap();
        let leased: TaskPushReadyHint =
            serde_json::from_str(&state.get(&ready_key).await.unwrap().unwrap()).unwrap();
        assert_eq!(leased.ready_at, epoch + chrono::Duration::seconds(30));

        let retry_at = epoch + chrono::Duration::seconds(10);
        e.retry_push_delivery_at(&scope(), "ready", "callback", &claim.lease_id, retry_at)
            .await
            .unwrap();
        let retry: TaskPushReadyHint =
            serde_json::from_str(&state.get(&ready_key).await.unwrap().unwrap()).unwrap();
        assert_eq!(retry.ready_at, retry_at);

        clock.advance_to(Duration::from_secs(10)).unwrap();
        let retry_claim = e
            .claim_push_delivery(
                &scope(),
                "ready",
                "callback",
                "worker",
                Duration::from_secs(30),
            )
            .await
            .unwrap()
            .unwrap();
        e.acknowledge_push_delivery(&scope(), "ready", "callback", &retry_claim.lease_id)
            .await
            .unwrap();
        assert!(state.get(&ready_key).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn callback_ready_hint_repair_rejects_a_mismatched_task_row() {
        let state = Arc::new(MemoryStateStore::new());
        let e = TaskEngine::new(state.clone());
        let mismatched = Task::new("different", "agents", "demo");
        state
            .set(
                &scope().task_key("indexed"),
                &serde_json::to_string(&mismatched).unwrap(),
                None,
            )
            .await
            .unwrap();

        let error = e
            .repair_push_ready_hint(&scope(), "indexed")
            .await
            .unwrap_err();

        assert!(matches!(error, TaskEngineError::InvalidDurableRecord(_)));
        assert!(
            state
                .get(&scope().push_ready_key("indexed"))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn callback_lease_fences_replicas_and_expired_acknowledgements() {
        let epoch = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let clock = Arc::new(acteon_time::ManualClock::new(epoch));
        let state = Arc::new(MemoryStateStore::new());
        let first = Arc::new(TaskEngine::new(state.clone()).with_clock(clock.clone()));
        let second = Arc::new(TaskEngine::new(state.clone()).with_clock(clock.clone()));
        let task = Task::new_at("t1", "agents", "demo", epoch);
        first
            .create_task_with_push_configs(task.clone(), vec![test_push_config(&task, "callback")])
            .await
            .unwrap();
        first
            .transition_task(&scope(), "t1", TaskState::Working, None)
            .await
            .unwrap();

        let a = first.clone();
        let b = second.clone();
        let (left, right) = tokio::join!(
            async move {
                a.claim_push_delivery(
                    &scope(),
                    "t1",
                    "callback",
                    "worker-a",
                    Duration::from_secs(5),
                )
                .await
                .unwrap()
            },
            async move {
                b.claim_push_delivery(
                    &scope(),
                    "t1",
                    "callback",
                    "worker-b",
                    Duration::from_secs(5),
                )
                .await
                .unwrap()
            }
        );
        assert!(left.is_some() ^ right.is_some());
        let original = left.or(right).unwrap();
        assert_eq!(original.sequence, 1);
        assert_eq!(original.attempt, 1);
        let mut in_flight_update = original.config.clone();
        in_flight_update.url = "https://updated.example/hook".into();
        assert!(matches!(
            first
                .upsert_push_config(&scope(), "t1", in_flight_update)
                .await,
            Err(TaskEngineError::PushConfigDeliveryInFlight)
        ));

        clock.advance_to(Duration::from_secs(5)).unwrap();
        let takeover = second
            .claim_push_delivery(
                &scope(),
                "t1",
                "callback",
                "worker-c",
                Duration::from_secs(5),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(takeover.sequence, original.sequence);
        assert_eq!(takeover.event_id, original.event_id);
        assert_eq!(takeover.attempt, 2);
        assert!(matches!(
            first
                .acknowledge_push_delivery(&scope(), "t1", "callback", &original.lease_id,)
                .await,
            Err(TaskEngineError::PushDeliveryLeaseLost)
        ));
        second
            .acknowledge_push_delivery(&scope(), "t1", "callback", &takeover.lease_id)
            .await
            .unwrap();
        assert!(
            first
                .claim_push_delivery(
                    &scope(),
                    "t1",
                    "callback",
                    "worker-a",
                    Duration::from_secs(5),
                )
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn callback_retry_and_dead_letter_survive_engine_replacement() {
        let epoch = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let clock = Arc::new(acteon_time::ManualClock::new(epoch));
        let state = Arc::new(MemoryStateStore::new());
        let engine = TaskEngine::new(state.clone()).with_clock(clock.clone());
        let task = Task::new_at("t1", "agents", "demo", epoch);
        engine
            .create_task_with_push_configs(task.clone(), vec![test_push_config(&task, "callback")])
            .await
            .unwrap();
        engine
            .transition_task(&scope(), "t1", TaskState::Working, None)
            .await
            .unwrap();
        let claim = engine
            .claim_push_delivery(
                &scope(),
                "t1",
                "callback",
                "worker-a",
                Duration::from_secs(30),
            )
            .await
            .unwrap()
            .unwrap();
        engine
            .retry_push_delivery_at(
                &scope(),
                "t1",
                "callback",
                &claim.lease_id,
                epoch + chrono::Duration::seconds(10),
            )
            .await
            .unwrap();

        let replacement = TaskEngine::new(state).with_clock(clock.clone());
        assert!(
            replacement
                .claim_push_delivery(
                    &scope(),
                    "t1",
                    "callback",
                    "worker-b",
                    Duration::from_secs(30),
                )
                .await
                .unwrap()
                .is_none()
        );
        clock.advance_to(Duration::from_secs(10)).unwrap();
        let retry = replacement
            .claim_push_delivery(
                &scope(),
                "t1",
                "callback",
                "worker-b",
                Duration::from_secs(30),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retry.sequence, claim.sequence);
        assert_eq!(retry.attempt, 2);
        let entry = replacement
            .dead_letter_push_delivery(
                &scope(),
                "t1",
                "callback",
                &retry.lease_id,
                DlqFailureKind::Exhausted,
                "HTTP 503",
            )
            .await
            .unwrap();
        assert_eq!(entry.attempts, 2);
        assert_eq!(entry.failure_kind, DlqFailureKind::Exhausted);
        assert_eq!(entry.sequence, Some(retry.sequence));
        assert_eq!(entry.event_id.as_deref(), Some(retry.event_id.as_str()));
        assert_eq!(
            replacement
                .list_push_dead_letters(&scope(), "t1")
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(
            replacement
                .claim_push_delivery(
                    &scope(),
                    "t1",
                    "callback",
                    "worker-c",
                    Duration::from_secs(30),
                )
                .await
                .unwrap()
                .is_none()
        );

        let mut updated = replacement
            .get_push_config(&scope(), "t1", "callback")
            .await
            .unwrap()
            .unwrap();
        updated.url = "https://repaired.example/hook".into();
        replacement
            .upsert_push_config(&scope(), "t1", updated)
            .await
            .unwrap();
        assert!(
            replacement
                .claim_push_delivery(
                    &scope(),
                    "t1",
                    "callback",
                    "worker-c",
                    Duration::from_secs(30),
                )
                .await
                .unwrap()
                .is_none()
        );

        replacement
            .resolve_push_dead_letter(&scope(), "t1", &entry.id, PushDeadLetterResolution::Retry)
            .await
            .unwrap();
        let replay = replacement
            .claim_push_delivery(
                &scope(),
                "t1",
                "callback",
                "worker-c",
                Duration::from_secs(30),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(replay.sequence, retry.sequence);
        assert_eq!(replay.event_id, retry.event_id);
        assert_eq!(replay.attempt, 1);
        assert_eq!(replay.config.url, "https://repaired.example/hook");
        let discarded = replacement
            .dead_letter_push_delivery(
                &scope(),
                "t1",
                "callback",
                &replay.lease_id,
                DlqFailureKind::Terminal,
                "HTTP 410",
            )
            .await
            .unwrap();
        replacement
            .resolve_push_dead_letter(
                &scope(),
                "t1",
                &discarded.id,
                PushDeadLetterResolution::Discard,
            )
            .await
            .unwrap();
        assert!(
            replacement
                .claim_push_delivery(
                    &scope(),
                    "t1",
                    "callback",
                    "worker-d",
                    Duration::from_secs(30),
                )
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn deleting_a_blocked_callback_preserves_resolvable_dead_letter_evidence() {
        let e = engine();
        let task = sample_task("blocked-delete");
        e.create_task_with_push_configs(task.clone(), vec![test_push_config(&task, "callback")])
            .await
            .unwrap();
        e.transition_task(&scope(), "blocked-delete", TaskState::Working, None)
            .await
            .unwrap();
        let claim = e
            .claim_push_delivery(
                &scope(),
                "blocked-delete",
                "callback",
                "worker",
                Duration::from_secs(30),
            )
            .await
            .unwrap()
            .unwrap();
        let entry = e
            .dead_letter_push_delivery(
                &scope(),
                "blocked-delete",
                "callback",
                &claim.lease_id,
                DlqFailureKind::Terminal,
                "HTTP 410",
            )
            .await
            .unwrap();

        assert!(
            e.delete_push_config(&scope(), "blocked-delete", "callback")
                .await
                .unwrap()
        );
        assert_eq!(
            e.list_push_dead_letters(&scope(), "blocked-delete")
                .await
                .unwrap()
                .len(),
            1
        );
        e.resolve_push_dead_letter(
            &scope(),
            "blocked-delete",
            &entry.id,
            PushDeadLetterResolution::Discard,
        )
        .await
        .unwrap();
        assert!(
            e.list_push_dead_letters(&scope(), "blocked-delete")
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn legacy_dead_letter_import_is_idempotent_and_detects_conflicts() {
        let e = engine();
        e.create_task(sample_task("legacy-dlq")).await.unwrap();
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let entry = PushDeliveryDlqEntry::new_at(
            uuid::Uuid::now_v7().to_string(),
            "callback",
            "legacy-dlq",
            "agents",
            "demo",
            "https://callback.example/a2a",
            DlqFailureKind::Terminal,
            "HTTP 410",
            1,
            r#"{"legacy":true}"#.into(),
            now,
        );

        assert_eq!(
            e.import_legacy_push_dead_letters(&scope(), "legacy-dlq", vec![entry.clone()])
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            e.import_legacy_push_dead_letters(&scope(), "legacy-dlq", vec![entry.clone()])
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            e.list_push_dead_letters(&scope(), "legacy-dlq")
                .await
                .unwrap(),
            vec![entry.clone()]
        );

        let mut conflicting = entry;
        conflicting.last_error = "different evidence".into();
        let error = e
            .import_legacy_push_dead_letters(&scope(), "legacy-dlq", vec![conflicting])
            .await
            .unwrap_err();
        assert!(matches!(error, TaskEngineError::InvalidDurableRecord(_)));
    }

    #[tokio::test]
    async fn legacy_callback_import_is_one_time_and_envelope_wins_stale_replay() {
        let state = Arc::new(MemoryStateStore::new());
        let e = TaskEngine::new(state.clone());
        let task = sample_task("legacy");
        state
            .set(
                &scope().task_key("legacy"),
                &serde_json::to_string(&task).unwrap(),
                None,
            )
            .await
            .unwrap();
        assert!(
            !e.push_config_migration_complete(&scope(), "legacy")
                .await
                .unwrap()
        );
        let legacy = test_push_config(&task, "callback");
        e.import_legacy_push_configs(&scope(), "legacy", vec![legacy])
            .await
            .unwrap();
        assert!(
            e.push_config_migration_complete(&scope(), "legacy")
                .await
                .unwrap()
        );

        let mut current = e
            .get_push_config(&scope(), "legacy", "callback")
            .await
            .unwrap()
            .unwrap();
        current.url = "https://current.example/a2a".into();
        current.updated_at += chrono::Duration::seconds(1);
        e.upsert_push_config(&scope(), "legacy", current)
            .await
            .unwrap();
        let stale = test_push_config(&task, "callback");
        let configs = e
            .import_legacy_push_configs(&scope(), "legacy", vec![stale])
            .await
            .unwrap();
        assert_eq!(configs[0].url, "https://current.example/a2a");
    }

    #[tokio::test]
    async fn legacy_raw_task_migrates_on_first_successful_mutation() {
        let state = Arc::new(MemoryStateStore::new());
        let e = TaskEngine::new(state.clone());
        let task = sample_task("legacy");
        state
            .set(
                &scope().task_key("legacy"),
                &serde_json::to_string(&task).unwrap(),
                None,
            )
            .await
            .unwrap();

        assert_eq!(
            e.get_task(&scope(), "legacy")
                .await
                .unwrap()
                .unwrap()
                .status
                .state,
            TaskState::Submitted
        );
        e.transition_task(&scope(), "legacy", TaskState::Working, None)
            .await
            .unwrap();

        let raw = state
            .get(&scope().task_key("legacy"))
            .await
            .unwrap()
            .unwrap();
        let stored: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(stored["recordVersion"], 1);
        assert_eq!(stored["task"]["status"]["state"], "working");
    }

    #[tokio::test]
    async fn durable_envelope_rejects_unknown_fields_and_versions() {
        let state = Arc::new(MemoryStateStore::new());
        let e = TaskEngine::new(state.clone());
        let key = scope().task_key("t1");
        let task = sample_task("t1");
        state
            .set(
                &key,
                &serde_json::json!({
                    "recordVersion": 1,
                    "task": task,
                    "delivery": {},
                    "unrecognized": true
                })
                .to_string(),
                None,
            )
            .await
            .unwrap();
        assert!(matches!(
            e.get_task(&scope(), "t1").await,
            Err(TaskEngineError::Serde(_))
        ));

        state
            .set(
                &key,
                &serde_json::json!({
                    "recordVersion": 2,
                    "task": sample_task("t1"),
                    "delivery": {}
                })
                .to_string(),
                None,
            )
            .await
            .unwrap();
        assert!(matches!(
            e.get_task(&scope(), "t1").await,
            Err(TaskEngineError::UnsupportedRecordVersion(2))
        ));
    }

    #[tokio::test]
    async fn transition_commits_the_protocol_event_in_the_same_task_record() {
        let state = Arc::new(MemoryStateStore::new());
        let e = TaskEngine::new(state.clone());
        e.create_task(sample_task("t1")).await.unwrap();
        let message = TaskMessage::text("working", TaskRole::Agent, "accepted");
        e.transition_task(&scope(), "t1", TaskState::Working, Some(message))
            .await
            .unwrap();

        let raw = state.get(&scope().task_key("t1")).await.unwrap().unwrap();
        let record = DurableTaskRecord::decode(&raw).unwrap();
        assert_eq!(record.task.status.state, TaskState::Working);
        assert_eq!(record.delivery.next_sequence, 1);
        assert_eq!(record.delivery.events.len(), 1);
        let event = &record.delivery.events[0];
        assert_eq!(event.sequence, 1);
        assert_eq!(event.payload["statusUpdate"]["taskId"], "t1");
        assert_eq!(
            event.payload["statusUpdate"]["status"]["state"],
            "TASK_STATE_WORKING"
        );
        assert_eq!(
            event.payload["statusUpdate"]["status"]["message"]["role"],
            "ROLE_AGENT"
        );
    }

    #[tokio::test]
    async fn delivery_log_backpressure_keeps_the_task_mutation_atomic() {
        let state = Arc::new(MemoryStateStore::new());
        let e = TaskEngine::new(state.clone());
        let task = sample_task("t1");
        let mut record = DurableTaskRecord::new(task.clone());
        record
            .delivery
            .callbacks
            .insert("callback".into(), test_callback(&task, 0));
        for _ in 0..MAX_DURABLE_TASK_EVENTS {
            record
                .delivery
                .append(
                    uuid::Uuid::now_v7().to_string(),
                    task.status.timestamp,
                    project_task_status_push_payload(
                        &task,
                        task.status.state,
                        task.status.timestamp,
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        state
            .set(&scope().task_key("t1"), &record.encode().unwrap(), None)
            .await
            .unwrap();

        assert!(matches!(
            e.transition_task(&scope(), "t1", TaskState::Working, None)
                .await,
            Err(TaskEngineError::DeliveryLogFull)
        ));
        let raw = state.get(&scope().task_key("t1")).await.unwrap().unwrap();
        let retained = DurableTaskRecord::decode(&raw).unwrap();
        assert_eq!(retained.task.status.state, TaskState::Submitted);
        assert_eq!(retained.delivery.events.len(), MAX_DURABLE_TASK_EVENTS);
        assert_eq!(
            retained.delivery.next_sequence,
            MAX_DURABLE_TASK_EVENTS as u64
        );
    }

    #[tokio::test]
    async fn acknowledged_delivery_prefix_prunes_before_the_next_atomic_append() {
        let state = Arc::new(MemoryStateStore::new());
        let e = TaskEngine::new(state.clone());
        let task = sample_task("t1");
        let mut record = DurableTaskRecord::new(task.clone());
        record
            .delivery
            .callbacks
            .insert("callback".into(), test_callback(&task, 0));
        for _ in 0..MAX_DURABLE_TASK_EVENTS {
            record
                .delivery
                .append(
                    uuid::Uuid::now_v7().to_string(),
                    task.status.timestamp,
                    project_task_status_push_payload(
                        &task,
                        task.status.state,
                        task.status.timestamp,
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        record
            .delivery
            .callbacks
            .get_mut("callback")
            .unwrap()
            .acknowledged_sequence = 128;
        state
            .set(&scope().task_key("t1"), &record.encode().unwrap(), None)
            .await
            .unwrap();

        e.transition_task(&scope(), "t1", TaskState::Working, None)
            .await
            .unwrap();
        let raw = state.get(&scope().task_key("t1")).await.unwrap().unwrap();
        let retained = DurableTaskRecord::decode(&raw).unwrap();
        assert_eq!(retained.delivery.next_sequence, 257);
        assert_eq!(retained.delivery.events.first().unwrap().sequence, 129);
        assert_eq!(retained.delivery.events.last().unwrap().sequence, 257);
    }

    #[tokio::test]
    async fn cas_conflict_retries_without_duplicating_the_delivery_event() {
        let inner = Arc::new(MemoryStateStore::new());
        let inner_store: Arc<dyn StateStore> = inner.clone();
        let faults = Arc::new(FaultStore::new(inner_store));
        let e = TaskEngine::new(faults.clone());
        e.create_task(sample_task("t1")).await.unwrap();
        let (read_reached, resume_read) = faults
            .pause_next_read(
                KeyKind::A2aTask,
                ReadOperation::GetVersioned,
                FaultTiming::After,
            )
            .unwrap();
        let transition = tokio::spawn({
            let e = e.clone();
            async move {
                e.transition_task(&scope(), "t1", TaskState::Working, None)
                    .await
            }
        });
        read_reached.await.unwrap();
        TaskEngine::new(inner.clone())
            .record_progress(&scope(), "t1")
            .await
            .unwrap();
        resume_read.send(()).unwrap();
        transition.await.unwrap().unwrap();

        let raw = inner.get(&scope().task_key("t1")).await.unwrap().unwrap();
        let record = DurableTaskRecord::decode(&raw).unwrap();
        assert_eq!(record.delivery.next_sequence, 1);
        assert_eq!(record.delivery.events.len(), 1);
        assert_eq!(record.delivery.events[0].sequence, 1);
    }

    #[tokio::test]
    async fn ordered_delivery_log_survives_engine_replacement() {
        let state = Arc::new(MemoryStateStore::new());
        let key = scope().task_key("t1");
        TaskEngine::new(state.clone())
            .create_task(sample_task("t1"))
            .await
            .unwrap();
        TaskEngine::new(state.clone())
            .transition_task(&scope(), "t1", TaskState::Working, None)
            .await
            .unwrap();
        let mut record =
            DurableTaskRecord::decode(&state.get(&key).await.unwrap().unwrap()).unwrap();
        record
            .delivery
            .callbacks
            .insert("callback".into(), test_callback(&record.task, 0));
        state
            .set(&key, &record.encode().unwrap(), None)
            .await
            .unwrap();

        TaskEngine::new(state.clone())
            .transition_task(&scope(), "t1", TaskState::Completed, None)
            .await
            .unwrap();
        let record = DurableTaskRecord::decode(&state.get(&key).await.unwrap().unwrap()).unwrap();
        assert_eq!(record.delivery.next_sequence, 2);
        assert_eq!(
            record
                .delivery
                .events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_ne!(
            record.delivery.events[0].event_id,
            record.delivery.events[1].event_id
        );
        assert_eq!(
            record.delivery.events[1].payload["statusUpdate"]["status"]["state"],
            "TASK_STATE_COMPLETED"
        );
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

    /// Phase 5 adversarial: a four-hop cycle that closes via the
    /// new edge. The two-hop case above proves direct-cycle
    /// detection works; this proves the BFS walks deep enough to
    /// catch a cycle that only forms when the chain closes back to
    /// its root through several intermediaries.
    #[tokio::test]
    async fn reference_four_hop_cycle_rejected() {
        let e = engine();
        // Build a chain a -> b -> c -> d (no cycle yet).
        create_with_ref(&e, "a", None).await;
        create_with_ref(&e, "b", Some("a")).await;
        create_with_ref(&e, "c", Some("b")).await;
        create_with_ref(&e, "d", Some("c")).await;
        // Append d -> a from a side, closing the loop a -> b -> c -> d -> a.
        let mut m = TaskMessage::text("a-loop4", Role::Agent, "close");
        m.reference_task_ids = vec!["d".into()];
        let err = e.append_history(&scope(), "a", m).await.unwrap_err();
        assert!(
            matches!(err, TaskEngineError::ReferenceCycle { .. }),
            "expected ReferenceCycle, got {err:?}"
        );
    }

    /// Phase 5 adversarial: a cycle that is reachable only via one
    /// branch of a branching reference graph. The first branch is
    /// acyclic; the second branch closes the loop. The walker must
    /// not declare the graph safe after seeing the first acyclic
    /// branch.
    #[tokio::test]
    async fn reference_branching_graph_with_cycle_rejected() {
        let e = engine();
        // Acyclic branch:  a -> x -> y
        create_with_ref(&e, "a", None).await;
        create_with_ref(&e, "x", Some("a")).await;
        create_with_ref(&e, "y", Some("x")).await;
        // Cycle branch:    a -> p -> a
        create_with_ref(&e, "p", Some("a")).await;
        // Now append a message on `a` that references BOTH y and p.
        // The walker has to discover that the p edge closes back to
        // a — the y edge is a safe distraction.
        let mut m = TaskMessage::text("a-branch", Role::Agent, "x");
        m.reference_task_ids = vec!["y".into(), "p".into()];
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

    /// Phase 5 adversarial: two reapers racing on the same stale
    /// task. Only one transition to Failed should land; the second
    /// caller must observe the row is no longer stale (because
    /// Failed is terminal) and return None, not double-fail.
    #[tokio::test]
    async fn fail_if_stale_idempotent_under_concurrent_reapers() {
        let e = Arc::new(engine());
        e.create_task(sample_task("zombie")).await.unwrap();
        e.transition_task(&scope(), "zombie", TaskState::Working, None)
            .await
            .unwrap();
        let future = Utc::now() + chrono::Duration::hours(1);
        // Two reaper attempts run in parallel against the same
        // task. The shared MemoryStateStore enforces CAS, so one
        // wins, the other re-reads and sees a terminal task — must
        // not error.
        let e1 = Arc::clone(&e);
        let e2 = Arc::clone(&e);
        let (r1, r2) = tokio::join!(
            async move { e1.fail_if_stale(&scope(), "zombie", future).await },
            async move { e2.fail_if_stale(&scope(), "zombie", future).await },
        );
        let r1 = r1.unwrap();
        let r2 = r2.unwrap();
        // Exactly one must return Some(reaped); the other Some-or-None
        // depending on race ordering, but a `Some` from the second
        // observer is *also* fine — it just means it read after the
        // first CAS commit but evaluated staleness on a still-stale
        // tick. The invariant the caller cares about is "the task
        // ends up Failed, and the second call did not blow up."
        let final_state = e
            .get_task(&scope(), "zombie")
            .await
            .unwrap()
            .unwrap()
            .status
            .state;
        assert_eq!(final_state, TaskState::Failed);
        // At least one observer must have reaped — sanity check that
        // the reaper did *something*.
        assert!(
            r1.is_some() || r2.is_some(),
            "at least one reaper attempt must have transitioned the task"
        );
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

    /// An audit backend that can be unavailable for a committed task
    /// transition, then restored for recovery.
    struct ToggleAudit {
        unavailable: AtomicBool,
        records: Mutex<HashMap<String, AuditRecord>>,
    }

    impl ToggleAudit {
        fn new() -> Self {
            Self {
                unavailable: AtomicBool::new(false),
                records: Mutex::new(HashMap::new()),
            }
        }

        fn set_unavailable(&self, unavailable: bool) {
            self.unavailable.store(unavailable, Ordering::SeqCst);
        }
    }

    #[async_trait::async_trait]
    impl AuditStore for ToggleAudit {
        async fn record(&self, entry: AuditRecord) -> Result<(), AuditError> {
            if self.unavailable.load(Ordering::SeqCst) {
                return Err(AuditError::Storage("synthetic audit outage".into()));
            }
            self.records.lock().unwrap().insert(entry.id.clone(), entry);
            Ok(())
        }

        async fn get_by_action_id(
            &self,
            action_id: &str,
        ) -> Result<Option<AuditRecord>, AuditError> {
            if self.unavailable.load(Ordering::SeqCst) {
                return Err(AuditError::Storage("synthetic audit outage".into()));
            }
            Ok(self
                .records
                .lock()
                .unwrap()
                .values()
                .find(|record| record.action_id == action_id)
                .cloned())
        }

        async fn get_by_id(&self, id: &str) -> Result<Option<AuditRecord>, AuditError> {
            if self.unavailable.load(Ordering::SeqCst) {
                return Err(AuditError::Storage("synthetic audit outage".into()));
            }
            Ok(self.records.lock().unwrap().get(id).cloned())
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
    async fn push_config_audit_records_are_sanitized() {
        let (e, audit) = audited_engine();
        let task = sample_task("t1");
        e.create_task(task.clone()).await.unwrap();
        let mut config = test_push_config(&task, "callback-1");
        config.url = "https://hooks.example/a2a?secret=url-secret".into();
        config.token = Some("bearer-secret".into());
        config.authentication = Some(acteon_core::PushAuthentication {
            schemes: vec!["Bearer".into()],
            credentials: Some("credential-secret".into()),
        });
        e.upsert_push_config(&scope(), "t1", config).await.unwrap();
        e.delete_push_config(&scope(), "t1", "callback-1")
            .await
            .unwrap();

        let records: Vec<_> = audit
            .records()
            .into_iter()
            .filter(|record| record.action_type == "a2a.task.push_config")
            .collect();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].outcome_details["operation"], "create");
        assert_eq!(records[1].outcome_details["operation"], "delete");
        for record in records {
            assert_eq!(record.action_id, "t1");
            assert_eq!(record.outcome_details["config_id"], "callback-1");
            let encoded = serde_json::to_string(&record).unwrap();
            for secret in ["url-secret", "bearer-secret", "credential-secret"] {
                assert!(!encoded.contains(secret), "audit leaked {secret}");
            }
        }
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

        let terminal = e
            .transition_task(&scope(), "t1", TaskState::Canceled, None)
            .await
            .unwrap();
        let records = audit.records();
        assert_eq!(records.len(), 3);
        let receipt = &records[2];
        assert_eq!(receipt.id, task_terminal_audit_id(&terminal));
        assert_eq!(receipt.outcome_details["from_state"], "working");
        assert_eq!(receipt.outcome_details["to_state"], "canceled");
        assert_eq!(receipt.outcome, "canceled");
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
    async fn terminal_task_audit_replays_after_outage_with_one_stable_receipt() {
        let state = Arc::new(MemoryStateStore::new());
        let audit = Arc::new(ToggleAudit::new());
        let engine =
            TaskEngine::new(state.clone()).with_audit(Arc::clone(&audit) as Arc<dyn AuditStore>);
        engine.create_task(sample_task("t1")).await.unwrap();
        engine
            .transition_task(&scope(), "t1", TaskState::Working, None)
            .await
            .unwrap();

        audit.set_unavailable(true);
        let terminal = engine
            .transition_task(&scope(), "t1", TaskState::Canceled, None)
            .await
            .unwrap();
        assert_eq!(terminal.status.state, TaskState::Canceled);
        let receipt_id = task_terminal_audit_id(&terminal);

        audit.set_unavailable(false);
        let recovered = TaskEngine::new(state)
            .with_audit(Arc::clone(&audit) as Arc<dyn AuditStore>)
            .reconcile_terminal_audits()
            .await
            .unwrap();
        assert_eq!(recovered, 1);
        let record = audit
            .get_by_id(&receipt_id)
            .await
            .unwrap()
            .expect("recovery writes the terminal task receipt");
        assert_eq!(record.outcome, "canceled");
        assert_eq!(record.outcome_details["operation"], "terminal_recovery");
        assert_eq!(engine.reconcile_terminal_audits().await.unwrap(), 0);
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

    fn authorization_requirement() -> TaskAuthorizationRequirement {
        TaskAuthorizationRequirement {
            verifier_id: "test-verifier".into(),
            verifier_revision: 1,
            authorization_request_id: "request-1".into(),
            recipient: acteon_core::PrincipalIdentity::new(
                "test-agent",
                acteon_core::PrincipalKind::Agent,
            )
            .unwrap(),
            credential_authority: "test-authority".into(),
            audience: "test-audience".into(),
            required_scopes: vec!["read".into()],
        }
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
        let prompt = task.status.message.as_ref().unwrap();
        assert_eq!(prompt.role, TaskRole::Agent);
        assert_eq!(prompt.task_id.as_deref(), Some("t1"));
        assert_eq!(prompt.parts[0].text.as_deref(), Some("clarify the date"));
        // The row is persisted, not just returned, and is well-formed.
        let stored = stored_approval(&e, &approval.approval_id).await.unwrap();
        assert_eq!(stored.kind, PauseKind::UserInput);
        stored.validate().unwrap();
    }

    #[tokio::test]
    async fn pause_for_authorization_auth_required() {
        let e = engine();
        working_task(&e, "t1").await;
        let (task, approval) = e
            .pause_for_authorization(&scope(), "t1", authorization_requirement(), None, None)
            .await
            .unwrap();
        assert_eq!(task.status.state, TaskState::AuthRequired);
        assert_eq!(approval.kind, PauseKind::UserAuth);
        assert_eq!(approval.task_id.as_deref(), Some("t1"));
        assert_eq!(
            approval
                .authorization_requirement
                .as_ref()
                .unwrap()
                .verifier_id,
            "test-verifier"
        );
    }

    #[tokio::test]
    async fn pause_for_human_refuses_unbound_auth() {
        let e = engine();
        working_task(&e, "t1").await;
        assert_eq!(
            e.pause_for_human(&scope(), "t1", PauseKind::UserAuth, None, None)
                .await
                .unwrap_err(),
            TaskEngineError::AuthorizationRequirementRequired
        );
        assert_eq!(approval_row_count(&e).await, 0);
        assert_eq!(
            e.get_task(&scope(), "t1")
                .await
                .unwrap()
                .unwrap()
                .status
                .state,
            TaskState::Working
        );
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
    async fn pause_for_human_recovers_a_lost_task_cas_acknowledgement() {
        let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
        let e = TaskEngine::new(faults.clone());
        working_task(&e, "t1").await;
        faults
            .fail_next(
                KeyKind::A2aTask,
                WriteOperation::CompareAndSwap,
                FaultTiming::After,
            )
            .unwrap();

        let (task, approval) = e
            .pause_for_human(&scope(), "t1", PauseKind::UserInput, None, None)
            .await
            .unwrap();
        assert_eq!(task.status.state, TaskState::InputRequired);
        assert_eq!(
            task.pending_approval_id.as_deref(),
            Some(approval.approval_id.as_str())
        );
        assert_eq!(
            stored_approval(&e, &approval.approval_id)
                .await
                .unwrap()
                .status,
            BusApprovalStatus::Pending
        );
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
            .pause_for_authorization("t1", authorization_requirement(), None, None)
            .await
            .unwrap();
        assert_eq!(task.status.state, TaskState::AuthRequired);
        assert_eq!(approval.kind, PauseKind::UserAuth);
    }

    fn input_response(task_id: &str, message_id: &str, text: &str) -> TaskMessage {
        let mut message = TaskMessage::text(message_id, TaskRole::User, text);
        message.task_id = Some(task_id.to_string());
        message
    }

    #[tokio::test]
    async fn resolve_input_resumes_task_and_closes_approval() {
        let e = engine();
        working_task(&e, "t1").await;
        let current = e.get_task(&scope(), "t1").await.unwrap().unwrap();
        e.upsert_push_config(&scope(), "t1", test_push_config(&current, "callback"))
            .await
            .unwrap();
        let (_, challenge) = e
            .pause_for_human(&scope(), "t1", PauseKind::UserInput, None, None)
            .await
            .unwrap();
        let ready_key = scope().push_ready_key("t1");
        e.state.delete(&ready_key).await.unwrap();
        let (task, approval) = e
            .resolve_input(
                &scope(),
                "t1",
                &challenge.approval_id,
                input_response("t1", "response-1", "2027-01-04"),
                "alice",
            )
            .await
            .unwrap();

        assert_eq!(task.status.state, TaskState::Working);
        assert!(task.pending_approval_id.is_none());
        assert_eq!(task.history.last().unwrap().message_id, "response-1");
        assert_eq!(approval.status, BusApprovalStatus::Approved);
        assert_eq!(approval.decided_by.as_deref(), Some("alice"));
        assert_eq!(
            approval.task_resolution.as_ref().unwrap().message_id,
            "response-1"
        );
        assert!(e.state.get(&ready_key).await.unwrap().is_some());
        let idx = StateKey::new(
            "agents",
            "demo",
            KeyKind::PendingBusApprovals,
            &challenge.approval_id,
        );
        assert!(e.state.get(&idx).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn resolve_input_retry_is_idempotent() {
        let e = engine();
        working_task(&e, "t1").await;
        let (_, challenge) = e
            .pause_for_human(&scope(), "t1", PauseKind::UserInput, None, None)
            .await
            .unwrap();
        let response = input_response("t1", "response-1", "same answer");
        e.resolve_input(
            &scope(),
            "t1",
            &challenge.approval_id,
            response.clone(),
            "alice",
        )
        .await
        .unwrap();
        let (task, approval) = e
            .resolve_input(&scope(), "t1", &challenge.approval_id, response, "alice")
            .await
            .unwrap();
        assert_eq!(
            task.history
                .iter()
                .filter(|message| message.message_id == "response-1")
                .count(),
            1
        );
        assert_eq!(approval.status, BusApprovalStatus::Approved);
    }

    #[tokio::test]
    async fn resolve_input_recovers_after_task_commit_before_approval_finalize() {
        let e = engine();
        working_task(&e, "t1").await;
        let (_, challenge) = e
            .pause_for_human(&scope(), "t1", PauseKind::UserInput, None, None)
            .await
            .unwrap();
        let response = input_response("t1", "response-1", "recover me");
        let resolution = TaskPauseResolution {
            message_id: response.message_id.clone(),
            content_digest: task_message_digest(&response).unwrap(),
        };
        e.claim_input_resolution(&scope(), "t1", &challenge.approval_id, &resolution, "alice")
            .await
            .unwrap();
        e.apply_input_resolution(
            &scope(),
            "t1",
            &challenge.approval_id,
            &resolution,
            response.clone(),
        )
        .await
        .unwrap();
        assert_eq!(
            stored_approval(&e, &challenge.approval_id)
                .await
                .unwrap()
                .status,
            BusApprovalStatus::Approving
        );

        let (task, approval) = e
            .resolve_input(&scope(), "t1", &challenge.approval_id, response, "alice")
            .await
            .unwrap();
        assert_eq!(task.status.state, TaskState::Working);
        assert_eq!(approval.status, BusApprovalStatus::Approved);
    }

    #[tokio::test]
    async fn resolve_input_closes_claim_if_task_is_canceled_before_commit() {
        let e = engine();
        working_task(&e, "t1").await;
        let (_, challenge) = e
            .pause_for_human(&scope(), "t1", PauseKind::UserInput, None, None)
            .await
            .unwrap();
        let response = input_response("t1", "response-1", "too late");
        let resolution = TaskPauseResolution {
            message_id: response.message_id.clone(),
            content_digest: task_message_digest(&response).unwrap(),
        };
        e.claim_input_resolution(&scope(), "t1", &challenge.approval_id, &resolution, "alice")
            .await
            .unwrap();
        e.transition_task(&scope(), "t1", TaskState::Canceled, None)
            .await
            .unwrap();

        let error = e
            .resolve_input(&scope(), "t1", &challenge.approval_id, response, "alice")
            .await
            .unwrap_err();
        assert!(matches!(error, TaskEngineError::ChallengeMismatch { .. }));
        assert_eq!(
            stored_approval(&e, &challenge.approval_id)
                .await
                .unwrap()
                .status,
            BusApprovalStatus::Rejected
        );
        let index = StateKey::new(
            "agents",
            "demo",
            KeyKind::PendingBusApprovals,
            &challenge.approval_id,
        );
        assert!(e.state.get(&index).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn resolve_input_rejects_conflicting_retry() {
        let e = engine();
        working_task(&e, "t1").await;
        let (_, challenge) = e
            .pause_for_human(&scope(), "t1", PauseKind::UserInput, None, None)
            .await
            .unwrap();
        e.resolve_input(
            &scope(),
            "t1",
            &challenge.approval_id,
            input_response("t1", "response-1", "first"),
            "alice",
        )
        .await
        .unwrap();
        let error = e
            .resolve_input(
                &scope(),
                "t1",
                &challenge.approval_id,
                input_response("t1", "response-1", "changed"),
                "alice",
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            TaskEngineError::ChallengeResponseConflict(_)
        ));
    }

    #[tokio::test]
    async fn resolve_input_refuses_auth_challenge() {
        let e = engine();
        working_task(&e, "t1").await;
        let (_, challenge) = e
            .pause_for_authorization(&scope(), "t1", authorization_requirement(), None, None)
            .await
            .unwrap();
        let error = e
            .resolve_input(
                &scope(),
                "t1",
                &challenge.approval_id,
                input_response("t1", "response-1", "secret"),
                "alice",
            )
            .await
            .unwrap_err();
        assert!(matches!(error, TaskEngineError::ChallengeKind { .. }));
        let task = e.get_task(&scope(), "t1").await.unwrap().unwrap();
        assert_eq!(task.status.state, TaskState::AuthRequired);
        assert_eq!(
            stored_approval(&e, &challenge.approval_id)
                .await
                .unwrap()
                .status,
            BusApprovalStatus::Pending
        );
    }

    #[tokio::test]
    async fn resolve_input_expires_challenge_without_resuming_task() {
        let clock = Arc::new(acteon_time::ManualClock::new(
            DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        ));
        let e = TaskEngine::new(Arc::new(MemoryStateStore::new())).with_clock(clock.clone());
        working_task(&e, "t1").await;
        let (_, challenge) = e
            .pause_for_human(
                &scope(),
                "t1",
                PauseKind::UserInput,
                None,
                Some(Duration::from_secs(1)),
            )
            .await
            .unwrap();
        clock.advance_to(Duration::from_secs(1)).unwrap();

        let error = e
            .resolve_input(
                &scope(),
                "t1",
                &challenge.approval_id,
                input_response("t1", "response-1", "too late"),
                "alice",
            )
            .await
            .unwrap_err();
        assert!(matches!(error, TaskEngineError::ChallengeExpired(_)));
        let task = e.get_task(&scope(), "t1").await.unwrap().unwrap();
        assert_eq!(task.status.state, TaskState::Failed);
        assert!(task.pending_approval_id.is_none());
        assert_eq!(
            stored_approval(&e, &challenge.approval_id)
                .await
                .unwrap()
                .status,
            BusApprovalStatus::Expired
        );
        let index = StateKey::new(
            "agents",
            "demo",
            KeyKind::PendingBusApprovals,
            &challenge.approval_id,
        );
        assert!(e.state.get(&index).await.unwrap().is_none());
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
        e.pause_for_authorization(&scope(), "t1", authorization_requirement(), None, None)
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
    #[tokio::test]
    async fn legacy_task_access_uses_durable_acceptance_even_without_display_metadata() {
        let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let trusted = TaskEngine::new(state.clone());
        let task = Task::new("governed", "prod", "acme");
        assert!(task.metadata.is_empty());
        trusted.create_task(task).await.unwrap();
        let journal = StateKey::new(
            "prod",
            "acme",
            KeyKind::Custom(crate::agent_runtime::ACCEPTANCE_KIND.into()),
            "governed",
        );
        state
            .set(&journal, "retained acceptance", None)
            .await
            .unwrap();
        let legacy = TaskEngine::new(state.clone()).with_legacy_task_access();
        let scope = TaskScope::new("prod", "acme");
        assert!(legacy.get_task(&scope, "governed").await.unwrap().is_none());
        assert!(legacy.list_tasks(&scope).await.unwrap().is_empty());
        assert!(matches!(
            legacy
                .append_history(
                    &scope,
                    "governed",
                    TaskMessage::text("message", TaskRole::User, "injected")
                )
                .await,
            Err(TaskEngineError::NotFound(_))
        ));
        assert!(matches!(
            legacy
                .transition_task(&scope, "governed", TaskState::Canceled, None)
                .await,
            Err(TaskEngineError::NotFound(_))
        ));
        let retained = trusted.get_task(&scope, "governed").await.unwrap().unwrap();
        assert_eq!(retained.status.state, TaskState::Submitted);
        assert!(retained.history.is_empty());
        // A damaged display ID cannot escape the scan's actual storage key.
        let mut damaged = retained;
        damaged.id = "display-decoy".into();
        state
            .set(
                &scope.task_key("governed"),
                &serde_json::to_string(&damaged).unwrap(),
                None,
            )
            .await
            .unwrap();
        assert!(legacy.list_tasks(&scope).await.unwrap().is_empty());
        // Losing the projection cannot permit legacy creation over accepted work.
        state.delete(&scope.task_key("governed")).await.unwrap();
        assert!(matches!(
            legacy
                .create_task(Task::new("governed", "prod", "acme"))
                .await,
            Err(TaskEngineError::NotFound(_))
        ));
    }
}
