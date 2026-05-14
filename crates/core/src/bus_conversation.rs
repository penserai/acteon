//! Bus conversation — multi-agent thread with state and ordered
//! message log (Phase 5).
//!
//! Conversations are an Acteon-side state object that pairs with a
//! shared Kafka events topic. Messages addressed to a conversation
//! are produced with `key = conversation_id`, so Kafka's key-based
//! partitioner gives per-conversation FIFO ordering without any
//! per-conversation topic explosion. Participants are recorded
//! on the conversation itself rather than inferred from message
//! senders so an operator can ACL the thread before the first
//! message lands.
//!
//! State machine:
//!
//! ```text
//!   ┌─────────┐  resolve   ┌──────────┐  archive   ┌──────────┐
//!   │ Active  │───────────▶│ Resolved │───────────▶│ Archived │
//!   └─────────┘            └──────────┘            └──────────┘
//!         ▲                      │
//!         └─────── reopen ───────┘
//! ```
//!
//! Linear by design; `Active → Archived` requires going through
//! `Resolved` first. `Reopen` is allowed back to `Active` from
//! `Resolved` only — once `Archived`, the conversation is final.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::bus_task::{
    MAX_METADATA_VALUE_BYTES, Message as TaskMessage, Role as TaskRole, TaskValidationError,
};

/// Default events-topic suffix. Combined with `{namespace}.{tenant}.`
/// to form the shared Kafka topic that all conversations in that
/// tenant produce to.
pub const DEFAULT_CONVERSATIONS_EVENTS_SUFFIX: &str = "conversations-events";

/// Lifecycle state of a conversation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum ConversationState {
    /// Conversation is open; participants may post messages.
    #[default]
    Active,
    /// Conversation has been resolved. Posts are still allowed (a
    /// participant may need to add a follow-up note) but the operator
    /// has marked the work as complete.
    Resolved,
    /// Conversation is closed and read-only. Posts are rejected.
    Archived,
}

/// Allowed transitions between states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum ConversationTransition {
    /// `Active → Resolved`.
    Resolve,
    /// `Resolved → Active`. Mistakes happen.
    Reopen,
    /// `Resolved → Archived`. Final.
    Archive,
}

/// A multi-agent thread tracked by Acteon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Conversation {
    /// Stable identifier. Used as the Kafka partition key for
    /// every message in this thread.
    pub conversation_id: String,
    /// Namespace the conversation belongs to.
    pub namespace: String,
    /// Tenant that owns the conversation.
    pub tenant: String,
    /// Operator-supplied title.
    #[serde(default)]
    pub title: Option<String>,
    /// Current lifecycle state.
    #[serde(default)]
    pub state: ConversationState,
    /// Agent IDs allowed to post in this conversation. Empty means
    /// "any agent in the tenant" — operators tighten the ACL by
    /// listing specific participants.
    #[serde(default)]
    pub participants: Vec<String>,
    /// Override events topic (defaults to
    /// `{namespace}.{tenant}.conversations-events`).
    #[serde(default)]
    pub events_topic: Option<String>,
    /// Free-form operator labels.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last mutation timestamp (CRUD or transition).
    pub updated_at: DateTime<Utc>,
}

impl Conversation {
    /// Construct a fresh conversation with sensible defaults.
    #[must_use]
    pub fn new(
        conversation_id: impl Into<String>,
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            conversation_id: conversation_id.into(),
            namespace: namespace.into(),
            tenant: tenant.into(),
            title: None,
            state: ConversationState::default(),
            participants: Vec::new(),
            events_topic: None,
            labels: HashMap::new(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Canonical Kafka events-topic name for this conversation. Falls
    /// back to the tenant-shared default when `events_topic` is unset.
    #[must_use]
    pub fn effective_events_topic(&self) -> String {
        self.events_topic.clone().unwrap_or_else(|| {
            format!(
                "{}.{}.{}",
                self.namespace, self.tenant, DEFAULT_CONVERSATIONS_EVENTS_SUFFIX
            )
        })
    }

    /// State-store id. The `conversation_id` is unique within a
    /// `(namespace, tenant)` so the parent scope already disambiguates.
    #[must_use]
    pub fn id(&self) -> String {
        self.conversation_id.clone()
    }

    /// Apply a transition, returning the resulting state. Rejects
    /// illegal transitions explicitly so the API can surface a 409
    /// instead of silently writing nothing.
    pub fn apply_transition(
        &mut self,
        transition: ConversationTransition,
    ) -> Result<ConversationState, ConversationValidationError> {
        let next = match (self.state, transition) {
            (ConversationState::Active, ConversationTransition::Resolve) => {
                ConversationState::Resolved
            }
            (ConversationState::Resolved, ConversationTransition::Reopen) => {
                ConversationState::Active
            }
            (ConversationState::Resolved, ConversationTransition::Archive) => {
                ConversationState::Archived
            }
            // Everything else (e.g. Resolve from Archived, Archive
            // from Active without going through Resolved) is illegal.
            (from, t) => {
                return Err(ConversationValidationError::IllegalTransition {
                    from,
                    transition: t,
                });
            }
        };
        self.state = next;
        self.updated_at = Utc::now();
        Ok(next)
    }

    /// Whether the conversation accepts new messages. `Archived` is
    /// read-only by design; `Active` and `Resolved` both allow posts
    /// (a follow-up after resolution is a common pattern).
    #[must_use]
    pub fn accepts_messages(&self) -> bool {
        !matches!(self.state, ConversationState::Archived)
    }

    /// Validate identity + ACL fields end-to-end.
    pub fn validate(&self) -> Result<(), ConversationValidationError> {
        Self::validate_id(&self.conversation_id)?;
        Self::validate_fragment(&self.namespace)?;
        Self::validate_fragment(&self.tenant)?;
        for p in &self.participants {
            Self::validate_id(p)
                .map_err(|_| ConversationValidationError::InvalidParticipant(p.clone()))?;
        }
        Ok(())
    }

    /// Conversation IDs follow the agent-ID alphabet —
    /// `[a-zA-Z0-9._-]`, max 120 chars — so they're safe in URLs,
    /// state keys, and Kafka headers.
    pub fn validate_id(s: &str) -> Result<(), ConversationValidationError> {
        if s.is_empty() {
            return Err(ConversationValidationError::EmptyId);
        }
        if s.len() > 120 {
            return Err(ConversationValidationError::IdTooLong);
        }
        if !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return Err(ConversationValidationError::InvalidIdChar(s.to_string()));
        }
        Ok(())
    }

    /// Namespace/tenant rules — same as topics so the generated
    /// events-topic name is always valid.
    pub fn validate_fragment(s: &str) -> Result<(), ConversationValidationError> {
        if s.is_empty() {
            return Err(ConversationValidationError::EmptyFragment);
        }
        if s.len() > 80 {
            return Err(ConversationValidationError::FragmentTooLong);
        }
        if !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(ConversationValidationError::InvalidFragmentChar(
                s.to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConversationValidationError {
    #[error("conversation_id must not be empty")]
    EmptyId,
    #[error("conversation_id exceeds 120 characters")]
    IdTooLong,
    #[error("conversation_id '{0}' contains characters outside [a-zA-Z0-9._-]")]
    InvalidIdChar(String),
    #[error("namespace/tenant fragment must not be empty")]
    EmptyFragment,
    #[error("namespace/tenant fragment exceeds 80 characters")]
    FragmentTooLong,
    #[error("namespace/tenant fragment '{0}' contains characters outside [a-zA-Z0-9_-]")]
    InvalidFragmentChar(String),
    #[error("participant '{0}' is not a valid agent_id")]
    InvalidParticipant(String),
    #[error("illegal transition {transition:?} from state {from:?}")]
    IllegalTransition {
        from: ConversationState,
        transition: ConversationTransition,
    },
    #[error("conversation message body is invalid: {0}")]
    InvalidMessage(#[from] TaskValidationError),
    #[error("conversation_id '{record}' on the envelope does not match parent '{parent}'")]
    ConversationIdMismatch { parent: String, record: String },
    #[error(
        "conversation message metadata value for key '{key}' exceeds {MAX_METADATA_VALUE_BYTES} bytes"
    )]
    MessageMetadataTooLong { key: String },
    #[error("conversation message metadata key must not be empty")]
    EmptyMessageMetadataKey,
    #[error("conversation message metadata value is not serializable JSON")]
    MessageMetadataInvalid,
}

// ---------------------------------------------------------------------
// ConversationMessage — A2A Message/Part-aligned envelope
// ---------------------------------------------------------------------

/// A message produced into a conversation's events topic, wrapping an
/// A2A [`TaskMessage`] (alias for [`crate::bus_task::Message`]) so the
/// wire format is directly consumable by A2A clients.
///
/// **Convergence goal:** every message that rides on the
/// `{namespace}.{tenant}.conversations-events` topic is shaped as
/// `ConversationMessage`. Subscribers parse the inner
/// [`TaskMessage`] and get an A2A-compliant `Message` with no
/// translation. Acteon-side routing metadata (sender agent ID,
/// produce timestamp, optional sender-supplied envelope kind for
/// pre-Task tool envelopes) sits *outside* the inner message so it
/// never collides with A2A wire fields.
///
/// **Relationship to [`crate::bus_tool`] envelopes:** the existing
/// `ToolCall` / `ToolResult` envelopes (Phase 6a) are pre-A2A
/// conversation messages with envelope kinds. In Phase 2 the Task
/// Engine projects them into A2A semantics (a Task in `Working` with
/// a tool-call `Message` in `history`). The `envelope_kind` field
/// below preserves the legacy routing tag during the transition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ConversationMessage {
    /// Conversation this message belongs to. The bus also stamps this
    /// as the Kafka partition key so consumers get per-conversation
    /// FIFO ordering.
    pub conversation_id: String,
    /// A2A [`TaskMessage`] body (role + parts + reference task ids).
    /// This is the part an A2A client deserializes directly.
    pub message: TaskMessage,
    /// `agent_id` of the producer. Carried on the envelope (in
    /// addition to whatever the inner message's `role` says) so the
    /// audit trail and routing layer don't need to deserialize the
    /// inner message to know who sent it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
    /// Optional envelope-kind routing tag — e.g. `"text"`,
    /// `"tool_call"`, `"tool_result"`, `"task_status"`,
    /// `"task_artifact"`. Stamped as `acteon.envelope.kind` on the
    /// Kafka header so subscribers can filter without parsing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope_kind: Option<String>,
    /// Envelope construction time (distinct from broker-stamped
    /// `produced_at`).
    pub created_at: DateTime<Utc>,
    /// Free-form envelope-level metadata. Distinct from
    /// `message.metadata` so Acteon-side trace IDs don't pollute the
    /// A2A wire payload.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl ConversationMessage {
    /// Construct a conversation message wrapping a text body from a
    /// specific role. Convenience for the common case.
    #[must_use]
    pub fn text(
        conversation_id: impl Into<String>,
        message_id: impl Into<String>,
        role: TaskRole,
        text: impl Into<String>,
    ) -> Self {
        let conversation_id = conversation_id.into();
        let mut message = TaskMessage::text(message_id, role, text);
        message.context_id = Some(conversation_id.clone());
        Self {
            conversation_id,
            message,
            sender: None,
            envelope_kind: Some("text".to_string()),
            created_at: Utc::now(),
            metadata: HashMap::new(),
        }
    }

    /// Validate identity, inner message, and bounded metadata. If the
    /// inner message's `contextId` is set, it must match
    /// `conversation_id` — otherwise an A2A consumer would see a
    /// `contextId` pointing at a different conversation than the one
    /// the envelope addresses.
    pub fn validate(&self) -> Result<(), ConversationValidationError> {
        Conversation::validate_id(&self.conversation_id)?;
        if let Some(s) = &self.sender {
            Conversation::validate_id(s)
                .map_err(|_| ConversationValidationError::InvalidParticipant(s.clone()))?;
        }
        if let Some(ctx) = &self.message.context_id
            && ctx != &self.conversation_id
        {
            return Err(ConversationValidationError::ConversationIdMismatch {
                parent: self.conversation_id.clone(),
                record: ctx.clone(),
            });
        }
        self.message.validate()?;
        for (k, v) in &self.metadata {
            if k.is_empty() {
                return Err(ConversationValidationError::EmptyMessageMetadataKey);
            }
            let encoded = serde_json::to_vec(v)
                .map_err(|_| ConversationValidationError::MessageMetadataInvalid)?;
            if encoded.len() > MAX_METADATA_VALUE_BYTES {
                return Err(ConversationValidationError::MessageMetadataTooLong { key: k.clone() });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_events_topic_defaults_when_unset() {
        let c = Conversation::new("c1", "agents", "demo");
        assert_eq!(
            c.effective_events_topic(),
            "agents.demo.conversations-events"
        );
    }

    #[test]
    fn effective_events_topic_honors_override() {
        let mut c = Conversation::new("c1", "agents", "demo");
        c.events_topic = Some("agents.demo.priority-thread".into());
        assert_eq!(c.effective_events_topic(), "agents.demo.priority-thread");
    }

    #[test]
    fn default_state_is_active() {
        let c = Conversation::new("c1", "ns", "t");
        assert_eq!(c.state, ConversationState::Active);
        assert!(c.accepts_messages());
    }

    #[test]
    fn transitions_resolve_then_archive() {
        let mut c = Conversation::new("c1", "ns", "t");
        c.apply_transition(ConversationTransition::Resolve).unwrap();
        assert_eq!(c.state, ConversationState::Resolved);
        assert!(c.accepts_messages());

        c.apply_transition(ConversationTransition::Archive).unwrap();
        assert_eq!(c.state, ConversationState::Archived);
        assert!(!c.accepts_messages());
    }

    #[test]
    fn transition_reopen_from_resolved() {
        let mut c = Conversation::new("c1", "ns", "t");
        c.apply_transition(ConversationTransition::Resolve).unwrap();
        c.apply_transition(ConversationTransition::Reopen).unwrap();
        assert_eq!(c.state, ConversationState::Active);
    }

    #[test]
    fn cannot_archive_directly_from_active() {
        let mut c = Conversation::new("c1", "ns", "t");
        let err = c
            .apply_transition(ConversationTransition::Archive)
            .unwrap_err();
        assert!(matches!(
            err,
            ConversationValidationError::IllegalTransition { .. }
        ));
    }

    #[test]
    fn cannot_reopen_from_archived() {
        let mut c = Conversation::new("c1", "ns", "t");
        c.apply_transition(ConversationTransition::Resolve).unwrap();
        c.apply_transition(ConversationTransition::Archive).unwrap();
        assert!(c.apply_transition(ConversationTransition::Reopen).is_err());
    }

    #[test]
    fn validate_rejects_invalid_participant() {
        let mut c = Conversation::new("c1", "ns", "t");
        c.participants.push("bad/agent".into());
        let err = c.validate().unwrap_err();
        assert!(matches!(
            err,
            ConversationValidationError::InvalidParticipant(_)
        ));
    }

    #[test]
    fn validate_accepts_dotted_id() {
        let c = Conversation::new("thread.42", "ns", "t");
        c.validate().unwrap();
    }

    #[test]
    fn roundtrip_serde() {
        let mut c = Conversation::new("c1", "ns", "t");
        c.title = Some("planning meeting".into());
        c.participants = vec!["planner-1".into(), "ocr-svc".into()];
        c.labels.insert("project".into(), "alpha".into());
        let j = serde_json::to_string(&c).unwrap();
        let back: Conversation = serde_json::from_str(&j).unwrap();
        assert_eq!(back.title, c.title);
        assert_eq!(back.participants, c.participants);
        assert_eq!(back.labels, c.labels);
    }

    // --- ConversationMessage ---

    #[test]
    fn conversation_message_text_validates() {
        ConversationMessage::text("conv-1", "msg-1", TaskRole::User, "hello")
            .validate()
            .unwrap();
    }

    #[test]
    fn conversation_message_sets_inner_context_id() {
        let cm = ConversationMessage::text("conv-1", "msg-1", TaskRole::User, "hi");
        assert_eq!(cm.message.context_id.as_deref(), Some("conv-1"));
    }

    #[test]
    fn conversation_message_rejects_mismatched_context_id() {
        let mut cm = ConversationMessage::text("conv-1", "msg-1", TaskRole::User, "hi");
        cm.message.context_id = Some("conv-2".into());
        assert!(matches!(
            cm.validate(),
            Err(ConversationValidationError::ConversationIdMismatch { .. })
        ));
    }

    #[test]
    fn conversation_message_rejects_bad_sender() {
        let mut cm = ConversationMessage::text("conv-1", "msg-1", TaskRole::User, "hi");
        cm.sender = Some("bad/sender".into());
        assert!(matches!(
            cm.validate(),
            Err(ConversationValidationError::InvalidParticipant(_))
        ));
    }

    #[test]
    fn conversation_message_propagates_message_validation_errors() {
        let mut cm = ConversationMessage::text("conv-1", "msg-1", TaskRole::User, "hi");
        cm.message.parts.clear();
        assert!(matches!(
            cm.validate(),
            Err(ConversationValidationError::InvalidMessage(_))
        ));
    }

    #[test]
    fn conversation_message_serializes_camel_case() {
        let mut cm = ConversationMessage::text("conv-1", "msg-1", TaskRole::Agent, "ok");
        cm.sender = Some("planner-1".into());
        let v = serde_json::to_value(&cm).unwrap();
        assert!(v.get("conversationId").is_some());
        assert!(v.get("createdAt").is_some());
        assert!(v.get("envelopeKind").is_some());
        // The inner Message is itself camelCase.
        let msg = v.get("message").unwrap();
        assert!(msg.get("messageId").is_some());
        assert!(msg.get("contextId").is_some());
    }

    #[test]
    fn conversation_message_roundtrip_serde() {
        let cm = ConversationMessage::text("conv-1", "msg-1", TaskRole::Agent, "answer");
        let j = serde_json::to_string(&cm).unwrap();
        let back: ConversationMessage = serde_json::from_str(&j).unwrap();
        assert_eq!(back.conversation_id, cm.conversation_id);
        assert_eq!(back.message.message_id, "msg-1");
        assert_eq!(back.envelope_kind.as_deref(), Some("text"));
    }

    #[test]
    fn conversation_message_caps_metadata_value_size() {
        let mut cm = ConversationMessage::text("conv-1", "msg-1", TaskRole::User, "x");
        cm.metadata
            .insert("k".into(), serde_json::Value::String("x".repeat(5000)));
        assert!(matches!(
            cm.validate(),
            Err(ConversationValidationError::MessageMetadataTooLong { .. })
        ));
    }
}
