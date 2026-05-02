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

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BusApproval {
    /// Stable identifier (server-generated UUID).
    pub approval_id: String,
    pub namespace: String,
    pub tenant: String,
    /// Conversation the parked envelope was destined for. The
    /// approve handler reads this to resolve the right events topic
    /// at produce time, including any `events_topic` override on
    /// the conversation record.
    pub conversation_id: String,
    /// Free-form rationale supplied by the requester. Bounded so a
    /// hostile caller can't bloat the row.
    #[serde(default)]
    pub reason: Option<String>,
    pub envelope: BusApprovalEnvelope,
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
    /// `None` until the approve handler commits to Kafka.
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
}

impl BusApproval {
    /// Validate identity + bounded fields. Run before persisting the
    /// row.
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
            conversation_id: "thread-1".into(),
            reason: Some("paid action".into()),
            envelope: BusApprovalEnvelope::ToolCall(call),
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
        match back.envelope {
            BusApprovalEnvelope::ToolCall(c) => assert_eq!(c.call_id, "call-1"),
        }
    }
}
