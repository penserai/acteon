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
}
