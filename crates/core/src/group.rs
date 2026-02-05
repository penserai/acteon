//! Event grouping types for batched notifications.
//!
//! Groups allow multiple related events to be batched together
//! before sending a single notification, reducing noise and
//! enabling better alert aggregation.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::ActionId;

/// A group of related events awaiting notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EventGroup {
    /// Unique identifier for this group.
    pub group_id: String,

    /// Key used to group events (hash of `group_by` field values).
    pub group_key: String,

    /// Common labels shared by all events in the group.
    #[serde(default)]
    pub labels: HashMap<String, String>,

    /// Events contained in this group.
    pub events: Vec<GroupedEvent>,

    /// When this group will be flushed and notification sent.
    pub notify_at: DateTime<Utc>,

    /// Current state of the group.
    pub state: GroupState,

    /// When this group was created.
    pub created_at: DateTime<Utc>,

    /// When this group was last updated.
    pub updated_at: DateTime<Utc>,
}

impl EventGroup {
    /// Create a new event group.
    #[must_use]
    pub fn new(
        group_id: impl Into<String>,
        group_key: impl Into<String>,
        notify_at: DateTime<Utc>,
    ) -> Self {
        let now = Utc::now();
        Self {
            group_id: group_id.into(),
            group_key: group_key.into(),
            labels: HashMap::new(),
            events: Vec::new(),
            notify_at,
            state: GroupState::Pending,
            created_at: now,
            updated_at: now,
        }
    }

    /// Add an event to this group.
    pub fn add_event(&mut self, event: GroupedEvent) {
        self.events.push(event);
        self.updated_at = Utc::now();
    }

    /// Get the number of events in this group.
    #[must_use]
    pub fn size(&self) -> usize {
        self.events.len()
    }

    /// Check if this group is ready to be flushed.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        matches!(self.state, GroupState::Pending) && Utc::now() >= self.notify_at
    }

    /// Set labels for this group.
    #[must_use]
    pub fn with_labels(mut self, labels: HashMap<String, String>) -> Self {
        self.labels = labels;
        self
    }
}

/// An individual event within a group.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct GroupedEvent {
    /// The original action ID.
    pub action_id: ActionId,

    /// Fingerprint of the event (if any).
    pub fingerprint: Option<String>,

    /// Status/state of the event.
    pub status: Option<String>,

    /// The event payload.
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub payload: serde_json::Value,

    /// When this event was received.
    pub received_at: DateTime<Utc>,
}

impl GroupedEvent {
    /// Create a new grouped event.
    #[must_use]
    pub fn new(action_id: ActionId, payload: serde_json::Value) -> Self {
        Self {
            action_id,
            fingerprint: None,
            status: None,
            payload,
            received_at: Utc::now(),
        }
    }

    /// Set the fingerprint.
    #[must_use]
    pub fn with_fingerprint(mut self, fingerprint: impl Into<String>) -> Self {
        self.fingerprint = Some(fingerprint.into());
        self
    }

    /// Set the status.
    #[must_use]
    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }
}

/// State of an event group.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum GroupState {
    /// Group is accumulating events, waiting to be flushed.
    #[default]
    Pending,

    /// Group has been flushed and notification sent.
    Notified,

    /// Group has been resolved/closed.
    Resolved,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_creation() {
        let notify_at = Utc::now() + chrono::Duration::seconds(60);
        let group = EventGroup::new("group-1", "key-abc", notify_at);

        assert_eq!(group.group_id, "group-1");
        assert_eq!(group.group_key, "key-abc");
        assert_eq!(group.size(), 0);
        assert_eq!(group.state, GroupState::Pending);
    }

    #[test]
    fn add_events() {
        let notify_at = Utc::now() + chrono::Duration::seconds(60);
        let mut group = EventGroup::new("group-1", "key-abc", notify_at);

        let event1 = GroupedEvent::new(
            ActionId::new("action-1".to_string()),
            serde_json::json!({"alert": "cpu_high"}),
        );
        let event2 = GroupedEvent::new(
            ActionId::new("action-2".to_string()),
            serde_json::json!({"alert": "memory_high"}),
        );

        group.add_event(event1);
        group.add_event(event2);

        assert_eq!(group.size(), 2);
    }

    #[test]
    fn group_ready_check() {
        // Group that should be ready (notify_at in the past)
        let past = Utc::now() - chrono::Duration::seconds(10);
        let group_ready = EventGroup::new("group-1", "key", past);
        assert!(group_ready.is_ready());

        // Group that should not be ready (notify_at in the future)
        let future = Utc::now() + chrono::Duration::seconds(60);
        let group_not_ready = EventGroup::new("group-2", "key", future);
        assert!(!group_not_ready.is_ready());
    }

    #[test]
    fn grouped_event_with_metadata() {
        let event = GroupedEvent::new(ActionId::new("action-1".to_string()), serde_json::json!({}))
            .with_fingerprint("fp-123")
            .with_status("firing");

        assert_eq!(event.fingerprint.as_deref(), Some("fp-123"));
        assert_eq!(event.status.as_deref(), Some("firing"));
    }

    #[test]
    fn group_state_serde() {
        let state = GroupState::Pending;
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, r#""pending""#);

        let back: GroupState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, GroupState::Pending);
    }
}
