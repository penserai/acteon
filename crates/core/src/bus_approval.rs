//! Bus pre-publish HITL approvals (Phase 6c).
//!
//! Some tool-calls shouldn't ride straight onto Kafka — sensitive
//! operations want a human-in-the-loop gate first. Phase 6c parks
//! the would-be envelope in Acteon state under a `BusApproval` row
//! with `status = pending`; the matching Kafka record only lands
//! after an operator approves. Reject (or expire) leaves no Kafka
//! footprint at all.
//!
//! V1 scope: tool-calls only. Stream chunks are excluded by
//! construction — gating each token of a stream defeats the point
//! of streaming.
//!
//! V1 trust model: the parked envelope is a state-store row, the
//! eventual Kafka record is a separate produce. The two writes are
//! not atomic. If the produce fails after a successful approval we
//! surface the error and leave the row in `pending` so an operator
//! can retry. If the produce succeeds but the row update fails the
//! idempotent producer + per-`call_id` uniqueness on the consumer
//! side keeps the topic clean. A future iteration can use a Kafka
//! transactional producer to close the window completely.
//!
//! ## A2A generalization (Phase 2)
//!
//! `BusApproval` is also the single "waiting on a human" row for A2A
//! Tasks — there is deliberately no parallel HITL table. A
//! [`PauseKind`] discriminates the three pause flavors:
//! [`PauseKind::OperatorApproval`] (the Phase 6c pre-publish gate
//! described above), [`PauseKind::UserAuth`], and
//! [`PauseKind::UserInput`] (an A2A Task paused in `AuthRequired` /
//! `InputRequired`). A task-pause row carries a `task_id` instead of
//! a parked `envelope` / `conversation_id`; [`BusApproval::validate`]
//! enforces the field shape for each kind.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::bus_task::TaskState;
use crate::bus_tool::ToolCall;

/// Lifecycle status for a pre-publish approval. Transitions:
///
/// ```text
/// Pending ──approve──▶ Approving ──produce ok──▶ Approved
///    │                     │
///    │                     └──produce error──▶ (stays Approving;
///    │                                          reconciler retries)
///    ├──reject────────────────────────────────▶ Rejected
///    └──ttl elapsed──────────────────────────▶ Expired
/// ```
///
/// Phase 10 introduced the `Approving` intermediate state.
///
/// V1 (Phase 6c) transitioned Pending → Approved in one step *after*
/// the Kafka produce, leaving a gap where a successful produce + failed
/// CAS looked like the row was still pending. The two-step state
/// machine, idempotent producer, and a reconciler that retries stuck
/// Approving rows close the gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum BusApprovalStatus {
    /// Awaiting an operator decision.
    Pending,
    /// Operator decided "approve"; produce to Kafka is in flight or
    /// has been retried but not yet observed succeeded. The envelope
    /// is no longer eligible for `reject`. The reconciler retries the
    /// produce until it succeeds (idempotent producer prevents
    /// duplicate Kafka records on retry) and then transitions the
    /// row to `Approved`.
    Approving,
    /// Approved; the parked envelope landed on Kafka and the
    /// produced offset is recorded on the row.
    Approved,
    /// Rejected; the parked envelope will never reach Kafka.
    Rejected,
    /// TTL elapsed before any decision — same outcome as `Rejected`
    /// but distinguishable for audit and UX purposes.
    Expired,
}

impl BusApprovalStatus {
    /// True iff the row has reached a final state: `Approved`,
    /// `Rejected`, or `Expired`. `Pending` and `Approving` are both
    /// non-terminal — `Pending` awaits a decision, `Approving` awaits
    /// produce confirmation.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            BusApprovalStatus::Approved | BusApprovalStatus::Rejected | BusApprovalStatus::Expired,
        )
    }
}

/// Why a [`BusApproval`] row exists — the taxonomy of "waiting on a
/// human."
///
/// Acteon's original HITL gate covers an *operator* approving an
/// outbound bus tool-call before it publishes. A2A adds two interrupt
/// states where the *end user* (not an operator) must act before a
/// Task can proceed. `PauseKind` is the single discriminator across
/// all three, so a paused A2A Task and an operator-gated tool-call
/// share one [`BusApproval`] type rather than fragmenting into
/// parallel "waiting on human" tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum PauseKind {
    /// An operator must approve an outbound bus tool-call before it
    /// publishes to Kafka — Acteon's original pre-publish HITL gate.
    /// The row carries the held-back `envelope` and its
    /// `conversation_id`; no A2A Task is involved.
    OperatorApproval,
    /// An A2A Task is paused in [`TaskState::AuthRequired`]: the user
    /// must supply a credential before the task can proceed. The row
    /// carries the `task_id`; there is no parked bus envelope.
    UserAuth,
    /// An A2A Task is paused in [`TaskState::InputRequired`]: the user
    /// must provide clarifying input before the task can proceed. The
    /// row carries the `task_id`; there is no parked bus envelope.
    UserInput,
}

impl PauseKind {
    /// The A2A [`TaskState`] a Task occupies while paused under this
    /// kind, or `None` for [`PauseKind::OperatorApproval`] — which
    /// gates a bus tool-call, not a Task, and so maps to no task
    /// state.
    #[must_use]
    pub fn task_state(self) -> Option<TaskState> {
        match self {
            PauseKind::OperatorApproval => None,
            PauseKind::UserAuth => Some(TaskState::AuthRequired),
            PauseKind::UserInput => Some(TaskState::InputRequired),
        }
    }

    /// True iff this kind pauses an A2A Task ([`PauseKind::UserAuth`]
    /// / [`PauseKind::UserInput`]) rather than gating a bus tool-call
    /// ([`PauseKind::OperatorApproval`]).
    #[must_use]
    pub fn is_task_pause(self) -> bool {
        self.task_state().is_some()
    }

    /// Stable `snake_case` wire value — matches the serde
    /// representation, handy for log / metric labels without a
    /// serialization round-trip.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            PauseKind::OperatorApproval => "operator_approval",
            PauseKind::UserAuth => "user_auth",
            PauseKind::UserInput => "user_input",
        }
    }
}

/// The bus envelope being held back. Tagged so the type system makes
/// it impossible to mix call/stream payloads into the same approval
/// row, and so a future expansion (e.g. `StreamEnd` for a guarded
/// terminal record) doesn't require a schema migration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum BusApprovalEnvelope {
    /// A tool-call awaiting publish approval. The bus stamps the
    /// usual `acteon.tool.call_id` / `acteon.correlation_id` headers
    /// at produce time, after the envelope is approved.
    ToolCall(ToolCall),
}

impl BusApprovalEnvelope {
    /// Return the underlying envelope's `call_id` / `stream_id` for
    /// audit + log correlation. Tool-calls expose `call_id`; future
    /// variants can return their own correlation token.
    #[must_use]
    pub fn correlation_token(&self) -> &str {
        match self {
            BusApprovalEnvelope::ToolCall(c) => &c.call_id,
        }
    }
}

/// Parked approval row. Lives at `KeyKind::BusApproval` keyed by
/// `approval_id`.
///
/// The field shape depends on [`BusApproval::kind`]:
/// [`PauseKind::OperatorApproval`] rows carry `envelope` +
/// `conversation_id`; task-pause rows carry `task_id`.
/// [`BusApproval::validate`] rejects any mixed shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BusApproval {
    /// Stable identifier (server-generated UUID).
    pub approval_id: String,
    pub namespace: String,
    pub tenant: String,
    /// Why this row exists. [`PauseKind::OperatorApproval`] gates a
    /// bus tool-call (`envelope` + `conversation_id` set);
    /// [`PauseKind::UserAuth`] / [`PauseKind::UserInput`] pause an
    /// A2A Task (`task_id` set).
    pub kind: PauseKind,
    /// Conversation the parked envelope was destined for. The approve
    /// handler reads this to resolve the right events topic at
    /// produce time, including any `events_topic` override on the
    /// conversation record. `Some` for [`PauseKind::OperatorApproval`];
    /// `None` for task-pause kinds, which have no bus conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// Free-form rationale supplied by the requester. Bounded so a
    /// hostile caller can't bloat the row.
    #[serde(default)]
    pub reason: Option<String>,
    /// The held-back bus envelope. `Some` for
    /// [`PauseKind::OperatorApproval`]; `None` for task-pause kinds,
    /// where the paused A2A Task itself is the held work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<BusApprovalEnvelope>,
    /// The A2A Task this row pauses. `Some` for the task-pause kinds
    /// ([`PauseKind::UserAuth`] / [`PauseKind::UserInput`]); `None`
    /// for [`PauseKind::OperatorApproval`]. The A2A Task Engine
    /// stamps this id onto `Task.pending_approval_id`, so a paused
    /// Task and its approval point at each other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub status: BusApprovalStatus,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// Set when the approval transitions out of `Pending`.
    #[serde(default)]
    pub decided_by: Option<String>,
    #[serde(default)]
    pub decided_at: Option<DateTime<Utc>>,
    /// Operator-supplied note attached to the decision.
    #[serde(default)]
    pub decision_note: Option<String>,
    /// Bus coordinates set after a successful produce on approval.
    /// `None` until the approve handler commits to Kafka, and always
    /// `None` for task-pause kinds (which never produce to Kafka).
    #[serde(default)]
    pub produced_partition: Option<i32>,
    #[serde(default)]
    pub produced_offset: Option<i64>,
    #[serde(default)]
    pub produced_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
}

/// Default TTL for a parked approval. Tunable per-request via
/// `approval_ttl_ms`; capped at [`MAX_APPROVAL_TTL_MS`].
pub const DEFAULT_APPROVAL_TTL_MS: u64 = 24 * 60 * 60 * 1000; // 24h

/// Hard cap on approval TTL. A pending row sits in state until it
/// decays; without a ceiling, an operator could write a row that
/// never expires and bloats the listing.
pub const MAX_APPROVAL_TTL_MS: u64 = 7 * 24 * 60 * 60 * 1000; // 7d

/// Cap on the `reason` and `decision_note` fields. Same rationale as
/// the `error_message` cap on `StreamEnd` / `ToolResult`: a single
/// malicious caller could otherwise bloat the row to MB.
pub const MAX_APPROVAL_NOTE_BYTES: usize = 4096;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BusApprovalValidationError {
    #[error("approval_id must not be empty")]
    EmptyId,
    #[error("approval_id exceeds 120 characters")]
    IdTooLong,
    #[error("approval_id '{0}' contains characters outside [a-zA-Z0-9._-]")]
    InvalidIdChar(String),
    #[error("approval_ttl_ms {0} exceeds the {MAX_APPROVAL_TTL_MS}ms cap")]
    TtlTooLong(u64),
    #[error("approval_ttl_ms must be > 0")]
    TtlZero,
    #[error("{field} exceeds {MAX_APPROVAL_NOTE_BYTES} bytes")]
    NoteTooLong { field: &'static str },
    #[error(
        "an operator-approval row requires `envelope` and `conversation_id` and must not carry `task_id`"
    )]
    OperatorApprovalShape,
    #[error(
        "a task-pause row requires a non-empty `task_id` and must not carry `envelope` or `conversation_id`"
    )]
    TaskPauseShape,
}

impl BusApproval {
    /// Build a task-pause approval row ([`PauseKind::UserAuth`] /
    /// [`PauseKind::UserInput`]). The A2A Task Engine calls this when
    /// a Task enters [`TaskState::AuthRequired`] /
    /// [`TaskState::InputRequired`]. The row starts
    /// [`BusApprovalStatus::Pending`].
    ///
    /// `kind` is taken verbatim; pass a task-pause kind
    /// ([`PauseKind::is_task_pause`]). [`BusApproval::validate`]
    /// rejects the row if `kind` is [`PauseKind::OperatorApproval`].
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new_task_pause(
        approval_id: impl Into<String>,
        namespace: impl Into<String>,
        tenant: impl Into<String>,
        kind: PauseKind,
        task_id: impl Into<String>,
        reason: Option<String>,
        created_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Self {
        Self {
            approval_id: approval_id.into(),
            namespace: namespace.into(),
            tenant: tenant.into(),
            kind,
            conversation_id: None,
            reason,
            envelope: None,
            task_id: Some(task_id.into()),
            status: BusApprovalStatus::Pending,
            created_at,
            expires_at,
            decided_by: None,
            decided_at: None,
            decision_note: None,
            produced_partition: None,
            produced_offset: None,
            produced_at: None,
            labels: HashMap::new(),
        }
    }

    /// Correlation token for logs and operator views: the parked
    /// envelope's token for an operator approval, the `task_id` for a
    /// task-pause row. Empty only for a malformed row that
    /// [`BusApproval::validate`] would already reject.
    #[must_use]
    pub fn correlation_token(&self) -> &str {
        match &self.envelope {
            Some(e) => e.correlation_token(),
            None => self.task_id.as_deref().unwrap_or(""),
        }
    }

    /// Validate identity, bounded fields, and the `kind`-dependent
    /// field shape. Run before persisting the row.
    pub fn validate(&self) -> Result<(), BusApprovalValidationError> {
        validate_approval_id(&self.approval_id)?;
        if let Some(r) = &self.reason
            && r.len() > MAX_APPROVAL_NOTE_BYTES
        {
            return Err(BusApprovalValidationError::NoteTooLong { field: "reason" });
        }
        if let Some(n) = &self.decision_note
            && n.len() > MAX_APPROVAL_NOTE_BYTES
        {
            return Err(BusApprovalValidationError::NoteTooLong {
                field: "decision_note",
            });
        }
        // `kind` fixes which subject fields must be present: an
        // operator approval parks a bus envelope, a task pause points
        // at an A2A Task. Reject any mixed shape so a row can't claim
        // to be both — or neither.
        match self.kind {
            PauseKind::OperatorApproval => {
                if self.envelope.is_none()
                    || self.conversation_id.is_none()
                    || self.task_id.is_some()
                {
                    return Err(BusApprovalValidationError::OperatorApprovalShape);
                }
            }
            PauseKind::UserAuth | PauseKind::UserInput => {
                if self.task_id.as_deref().is_none_or(str::is_empty)
                    || self.envelope.is_some()
                    || self.conversation_id.is_some()
                {
                    return Err(BusApprovalValidationError::TaskPauseShape);
                }
            }
        }
        Ok(())
    }
}

/// Shared id-validation. Mirrors the rule used across the rest of
/// the bus: alphanumeric + `[._-]`, 1..=120 bytes.
pub fn validate_approval_id(s: &str) -> Result<(), BusApprovalValidationError> {
    if s.is_empty() {
        return Err(BusApprovalValidationError::EmptyId);
    }
    if s.len() > 120 {
        return Err(BusApprovalValidationError::IdTooLong);
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(BusApprovalValidationError::InvalidIdChar(s.to_string()));
    }
    Ok(())
}

/// Validate a caller-supplied TTL against [`MAX_APPROVAL_TTL_MS`].
pub fn validate_approval_ttl(ttl_ms: u64) -> Result<(), BusApprovalValidationError> {
    if ttl_ms == 0 {
        return Err(BusApprovalValidationError::TtlZero);
    }
    if ttl_ms > MAX_APPROVAL_TTL_MS {
        return Err(BusApprovalValidationError::TtlTooLong(ttl_ms));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> BusApproval {
        let mut call = ToolCall::new("call-1", "calendar.list", json!({}));
        call.sender = Some("planner-1".into());
        let now = Utc::now();
        BusApproval {
            approval_id: "appr-1".into(),
            namespace: "agents".into(),
            tenant: "demo".into(),
            kind: PauseKind::OperatorApproval,
            conversation_id: Some("thread-1".into()),
            reason: Some("paid action".into()),
            envelope: Some(BusApprovalEnvelope::ToolCall(call)),
            task_id: None,
            status: BusApprovalStatus::Pending,
            created_at: now,
            expires_at: now + chrono::Duration::hours(1),
            decided_by: None,
            decided_at: None,
            decision_note: None,
            produced_partition: None,
            produced_offset: None,
            produced_at: None,
            labels: HashMap::new(),
        }
    }

    fn sample_task_pause(kind: PauseKind) -> BusApproval {
        let now = Utc::now();
        BusApproval::new_task_pause(
            "appr-tp-1",
            "agents",
            "demo",
            kind,
            "task-9",
            Some("needs an API key".into()),
            now,
            now + chrono::Duration::hours(1),
        )
    }

    #[test]
    fn validate_basic() {
        sample().validate().unwrap();
    }

    #[test]
    fn rejects_empty_id() {
        let mut a = sample();
        a.approval_id = String::new();
        assert_eq!(a.validate(), Err(BusApprovalValidationError::EmptyId));
    }

    #[test]
    fn rejects_long_id() {
        let mut a = sample();
        a.approval_id = "x".repeat(121);
        assert_eq!(a.validate(), Err(BusApprovalValidationError::IdTooLong));
    }

    #[test]
    fn rejects_bad_id_char() {
        let mut a = sample();
        a.approval_id = "appr/1".into();
        assert!(matches!(
            a.validate(),
            Err(BusApprovalValidationError::InvalidIdChar(_))
        ));
    }

    #[test]
    fn caps_reason_length() {
        let mut a = sample();
        a.reason = Some("x".repeat(MAX_APPROVAL_NOTE_BYTES + 1));
        assert_eq!(
            a.validate(),
            Err(BusApprovalValidationError::NoteTooLong { field: "reason" })
        );
    }

    #[test]
    fn caps_decision_note_length() {
        let mut a = sample();
        a.decision_note = Some("x".repeat(MAX_APPROVAL_NOTE_BYTES + 1));
        assert_eq!(
            a.validate(),
            Err(BusApprovalValidationError::NoteTooLong {
                field: "decision_note"
            })
        );
    }

    #[test]
    fn ttl_zero_rejected() {
        assert_eq!(
            validate_approval_ttl(0),
            Err(BusApprovalValidationError::TtlZero)
        );
    }

    #[test]
    fn ttl_above_cap_rejected() {
        assert!(matches!(
            validate_approval_ttl(MAX_APPROVAL_TTL_MS + 1),
            Err(BusApprovalValidationError::TtlTooLong(_))
        ));
    }

    #[test]
    fn ttl_at_cap_accepted() {
        validate_approval_ttl(MAX_APPROVAL_TTL_MS).unwrap();
    }

    #[test]
    fn status_terminal_helpers() {
        assert!(!BusApprovalStatus::Pending.is_terminal());
        // Approving is mid-flight — not terminal even though the
        // operator has decided. The reconciler is still allowed to
        // retry the produce.
        assert!(!BusApprovalStatus::Approving.is_terminal());
        assert!(BusApprovalStatus::Approved.is_terminal());
        assert!(BusApprovalStatus::Rejected.is_terminal());
        assert!(BusApprovalStatus::Expired.is_terminal());
    }

    #[test]
    fn envelope_correlation_token() {
        let env = BusApprovalEnvelope::ToolCall(ToolCall::new("call-77", "x", json!({})));
        assert_eq!(env.correlation_token(), "call-77");
    }

    #[test]
    fn roundtrip_serde() {
        let a = sample();
        let j = serde_json::to_string(&a).unwrap();
        let back: BusApproval = serde_json::from_str(&j).unwrap();
        assert_eq!(back.approval_id, a.approval_id);
        assert_eq!(back.kind, PauseKind::OperatorApproval);
        match back.envelope {
            Some(BusApprovalEnvelope::ToolCall(c)) => assert_eq!(c.call_id, "call-1"),
            None => panic!("operator-approval row must round-trip an envelope"),
        }
    }

    #[test]
    fn pause_kind_task_state_mapping() {
        assert_eq!(PauseKind::OperatorApproval.task_state(), None);
        assert_eq!(
            PauseKind::UserAuth.task_state(),
            Some(TaskState::AuthRequired)
        );
        assert_eq!(
            PauseKind::UserInput.task_state(),
            Some(TaskState::InputRequired)
        );
        assert!(!PauseKind::OperatorApproval.is_task_pause());
        assert!(PauseKind::UserAuth.is_task_pause());
        assert!(PauseKind::UserInput.is_task_pause());
    }

    #[test]
    fn pause_kind_serde_snake_case() {
        assert_eq!(
            serde_json::to_string(&PauseKind::OperatorApproval).unwrap(),
            "\"operator_approval\""
        );
        assert_eq!(
            serde_json::to_string(&PauseKind::UserInput).unwrap(),
            "\"user_input\""
        );
        let back: PauseKind = serde_json::from_str("\"user_auth\"").unwrap();
        assert_eq!(back, PauseKind::UserAuth);
        assert_eq!(PauseKind::UserAuth.as_str(), "user_auth");
    }

    #[test]
    fn task_pause_constructor_is_valid() {
        let a = sample_task_pause(PauseKind::UserAuth);
        a.validate().unwrap();
        assert_eq!(a.kind, PauseKind::UserAuth);
        assert_eq!(a.task_id.as_deref(), Some("task-9"));
        assert!(a.envelope.is_none());
        assert!(a.conversation_id.is_none());
        assert_eq!(a.status, BusApprovalStatus::Pending);
        assert_eq!(a.correlation_token(), "task-9");
    }

    #[test]
    fn task_pause_roundtrip_serde() {
        let a = sample_task_pause(PauseKind::UserInput);
        let j = serde_json::to_string(&a).unwrap();
        let back: BusApproval = serde_json::from_str(&j).unwrap();
        assert_eq!(back.kind, PauseKind::UserInput);
        assert_eq!(back.task_id.as_deref(), Some("task-9"));
        assert!(back.envelope.is_none());
        back.validate().unwrap();
    }

    #[test]
    fn validate_rejects_operator_row_without_envelope() {
        let mut a = sample();
        a.envelope = None;
        assert_eq!(
            a.validate(),
            Err(BusApprovalValidationError::OperatorApprovalShape)
        );
    }

    #[test]
    fn validate_rejects_operator_row_with_task_id() {
        let mut a = sample();
        a.task_id = Some("task-1".into());
        assert_eq!(
            a.validate(),
            Err(BusApprovalValidationError::OperatorApprovalShape)
        );
    }

    #[test]
    fn validate_rejects_task_pause_with_envelope() {
        let mut a = sample_task_pause(PauseKind::UserAuth);
        a.envelope = Some(BusApprovalEnvelope::ToolCall(ToolCall::new(
            "c",
            "x",
            json!({}),
        )));
        assert_eq!(
            a.validate(),
            Err(BusApprovalValidationError::TaskPauseShape)
        );
    }

    #[test]
    fn validate_rejects_task_pause_without_task_id() {
        let mut a = sample_task_pause(PauseKind::UserInput);
        a.task_id = None;
        assert_eq!(
            a.validate(),
            Err(BusApprovalValidationError::TaskPauseShape)
        );
    }

    #[test]
    fn validate_rejects_task_pause_with_conversation_id() {
        let mut a = sample_task_pause(PauseKind::UserAuth);
        a.conversation_id = Some("thread-1".into());
        assert_eq!(
            a.validate(),
            Err(BusApprovalValidationError::TaskPauseShape)
        );
    }
}
