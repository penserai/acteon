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
///
/// Event groups support three notification timers:
///
/// - [`group_wait_seconds`](Self::group_wait_seconds): initial wait from
///   the first event to the first flush.
/// - [`group_interval_seconds`](Self::group_interval_seconds): wait
///   between successive flushes when new events arrive in a persistent
///   group.
/// - [`repeat_interval_seconds`](Self::repeat_interval_seconds):
///   optional. When set, forces a re-flush every N seconds even with
///   no new events. Presence of this field makes the group persistent
///   (it is not deleted after flush).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EventGroup {
    /// Unique identifier for this group.
    pub group_id: String,

    /// Key used to group events (hash of `group_by` field values).
    pub group_key: String,

    /// Namespace the first event was dispatched to. Used by the
    /// background flush worker to persist updated group state back
    /// to the state store without needing the original action.
    #[serde(default)]
    pub namespace: String,

    /// Tenant the first event was dispatched to.
    #[serde(default)]
    pub tenant: String,

    /// Common labels shared by all events in the group.
    #[serde(default)]
    pub labels: HashMap<String, String>,

    /// Events contained in this group, capped at [`max_group_size`](Self::max_group_size).
    /// When a new event arrives at capacity, the oldest event is dropped.
    pub events: Vec<GroupedEvent>,

    /// When this group will be flushed and notification sent.
    pub notify_at: DateTime<Utc>,

    /// When the group was last flushed, or `None` if never flushed.
    /// Used to compute the next `notify_at` after re-notification
    /// cycles.
    #[serde(default)]
    pub last_notified_at: Option<DateTime<Utc>>,

    /// Current state of the group.
    pub state: GroupState,

    /// When this group was created.
    pub created_at: DateTime<Utc>,

    /// When this group was last updated.
    pub updated_at: DateTime<Utc>,

    /// Initial wait window from the first event to the first flush,
    /// captured at group creation from the rule's `group_wait_seconds`.
    #[serde(default = "default_group_wait_seconds")]
    pub group_wait_seconds: u64,

    /// Wait window between successive flushes for a persistent group
    /// when new events arrive, captured at group creation from the
    /// rule's `group_interval_seconds`. Honored only when
    /// `repeat_interval_seconds` is set.
    #[serde(default = "default_group_interval_seconds")]
    pub group_interval_seconds: u64,

    /// Optional forced re-notification interval. When `Some`, the
    /// group is kept alive after flush and re-flushes every N seconds
    /// even with no new events. When `None`, the group is deleted
    /// after its first flush (ephemeral — Phase-1 behavior).
    #[serde(default)]
    pub repeat_interval_seconds: Option<u64>,

    /// Maximum number of events held in the group. When a new event
    /// arrives at capacity, the oldest event is dropped (FIFO).
    #[serde(default = "default_max_group_size")]
    pub max_group_size: usize,

    /// Number of consecutive re-notification flushes that fired with
    /// no new events since the previous flush. Reset to zero whenever
    /// a new event arrives. Used by the background worker to evict
    /// idle persistent groups — a group that has quietly re-fired
    /// [`MAX_IDLE_FLUSHES`] times in a row is considered resolved
    /// and removed from the cache.
    #[serde(default)]
    pub idle_flushes: u32,

    /// Captured trace context from the first event in the group.
    /// Used to link the batched notification back to the original trigger.
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(value_type = HashMap<String, String>))]
    pub trace_context: HashMap<String, String>,
}

/// Maximum number of consecutive idle re-flushes before a persistent
/// group is evicted from the in-memory cache.
///
/// An "idle flush" is a repeat-interval re-fire that happens with no
/// new events since the previous flush. After this many in a row,
/// the background worker assumes the underlying condition has
/// resolved and removes the group.
pub const MAX_IDLE_FLUSHES: u32 = 3;

fn default_group_wait_seconds() -> u64 {
    30
}

fn default_group_interval_seconds() -> u64 {
    300
}

fn default_max_group_size() -> usize {
    100
}

impl EventGroup {
    /// Create a new event group with Phase-1 defaults (ephemeral, no
    /// repeat interval, default max size). Prefer
    /// [`with_timing`](Self::with_timing) for full control.
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
            namespace: String::new(),
            tenant: String::new(),
            labels: HashMap::new(),
            events: Vec::new(),
            notify_at,
            last_notified_at: None,
            state: GroupState::Pending,
            created_at: now,
            updated_at: now,
            group_wait_seconds: default_group_wait_seconds(),
            group_interval_seconds: default_group_interval_seconds(),
            repeat_interval_seconds: None,
            max_group_size: default_max_group_size(),
            idle_flushes: 0,
            trace_context: HashMap::new(),
        }
    }

    /// Set the `(namespace, tenant)` routing pair for this group.
    #[must_use]
    pub fn with_scope(mut self, namespace: impl Into<String>, tenant: impl Into<String>) -> Self {
        self.namespace = namespace.into();
        self.tenant = tenant.into();
        self
    }

    /// Set the timing parameters and max size on this group. Typically
    /// called immediately after [`new`](Self::new) before the first
    /// event is added.
    #[must_use]
    pub fn with_timing(
        mut self,
        group_wait_seconds: u64,
        group_interval_seconds: u64,
        repeat_interval_seconds: Option<u64>,
        max_group_size: usize,
    ) -> Self {
        self.group_wait_seconds = group_wait_seconds;
        self.group_interval_seconds = group_interval_seconds;
        self.repeat_interval_seconds = repeat_interval_seconds;
        self.max_group_size = max_group_size.max(1);
        self
    }

    /// Add an event to this group. When the group is at capacity, the
    /// oldest event is dropped before the new one is appended. This
    /// enforces the [`max_group_size`](Self::max_group_size) cap for
    /// persistent groups that re-flush over time.
    pub fn add_event(&mut self, event: GroupedEvent) {
        if self.max_group_size > 0 && self.events.len() >= self.max_group_size {
            // Drop the oldest event to make room.
            self.events.remove(0);
        }
        self.events.push(event);
        self.updated_at = Utc::now();
    }

    /// Get the number of events in this group.
    #[must_use]
    pub fn size(&self) -> usize {
        self.events.len()
    }

    /// Check if this group is ready to be flushed.
    ///
    /// Ready conditions:
    /// - The group is in [`GroupState::Pending`] (has unflushed events)
    ///   **or** it has been [`GroupState::Notified`] and its repeat
    ///   interval is due, AND
    /// - `now >= notify_at`.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        if Utc::now() < self.notify_at {
            return false;
        }
        matches!(self.state, GroupState::Pending | GroupState::Notified)
    }

    /// Set labels for this group.
    #[must_use]
    pub fn with_labels(mut self, labels: HashMap<String, String>) -> Self {
        self.labels = labels;
        self
    }

    /// Whether this group should be kept alive after flush (for
    /// periodic re-notification) or deleted.
    #[must_use]
    pub fn is_persistent(&self) -> bool {
        self.repeat_interval_seconds.is_some()
    }

    /// Whether this persistent group has been idle (re-firing with no
    /// new events) long enough to be evicted from the cache.
    ///
    /// Returns `false` for ephemeral groups — they are deleted by the
    /// flush worker immediately after their single flush and never
    /// reach the eviction path.
    #[must_use]
    pub fn should_evict(&self) -> bool {
        self.is_persistent() && self.idle_flushes >= MAX_IDLE_FLUSHES
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
    fn add_event_enforces_max_size_drop_oldest() {
        let notify_at = Utc::now() + chrono::Duration::seconds(60);
        let mut group =
            EventGroup::new("group-1", "key-abc", notify_at).with_timing(60, 300, Some(3_600), 2);

        for i in 0..4 {
            group.add_event(GroupedEvent::new(
                ActionId::new(format!("action-{i}")),
                serde_json::json!({}),
            ));
        }

        assert_eq!(group.size(), 2);
        let ids: Vec<&str> = group.events.iter().map(|e| e.action_id.as_str()).collect();
        assert_eq!(ids, vec!["action-2", "action-3"]);
    }

    #[test]
    fn is_persistent_reflects_repeat_interval() {
        let notify_at = Utc::now() + chrono::Duration::seconds(60);
        let ephemeral = EventGroup::new("g", "k", notify_at);
        assert!(!ephemeral.is_persistent());

        let persistent =
            EventGroup::new("g", "k", notify_at).with_timing(60, 300, Some(3_600), 100);
        assert!(persistent.is_persistent());
    }

    #[test]
    fn is_ready_accepts_notified_state() {
        let past = Utc::now() - chrono::Duration::seconds(5);
        let mut group = EventGroup::new("g", "k", past).with_timing(60, 300, Some(3_600), 100);
        group.state = GroupState::Notified;
        assert!(group.is_ready(), "notified persistent groups can re-fire");
    }

    #[test]
    fn is_ready_rejects_resolved_state() {
        let past = Utc::now() - chrono::Duration::seconds(5);
        let mut group = EventGroup::new("g", "k", past);
        group.state = GroupState::Resolved;
        assert!(!group.is_ready());
    }

    #[test]
    fn backward_compat_deserialize_old_group_record() {
        // An old EventGroup JSON without Phase-2 timing fields should
        // deserialize with defaults, keeping backward compatibility
        // for state-store records written by the pre-Phase-2 server.
        let old_json = serde_json::json!({
            "group_id": "g1",
            "group_key": "k1",
            "events": [],
            "notify_at": "2026-04-01T00:00:00Z",
            "state": "pending",
            "created_at": "2026-04-01T00:00:00Z",
            "updated_at": "2026-04-01T00:00:00Z",
        });
        let parsed: EventGroup = serde_json::from_value(old_json).expect("should deserialize");
        assert_eq!(parsed.group_wait_seconds, 30);
        assert_eq!(parsed.group_interval_seconds, 300);
        assert_eq!(parsed.repeat_interval_seconds, None);
        assert_eq!(parsed.max_group_size, 100);
        assert_eq!(parsed.last_notified_at, None);
        assert!(!parsed.is_persistent());
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
