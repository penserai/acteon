//! Group manager for batched event notifications.
//!
//! The group manager collects related events and batches them
//! for periodic notification, reducing alert noise.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use acteon_core::{Action, EventGroup, GroupState, GroupedEvent, compute_fingerprint};
use acteon_crypto::PayloadEncryptor;
use acteon_state::{KeyKind, StateKey, StateStore};

use crate::error::GatewayError;

/// Manages event groups for batched notifications.
#[derive(Debug, Default)]
pub struct GroupManager {
    /// In-memory cache of active groups (for fast access).
    /// Key is `group_key` (hash of `group_by` fields).
    pub(crate) groups: Arc<RwLock<HashMap<String, EventGroup>>>,
}

impl GroupManager {
    /// Create a new group manager.
    #[must_use]
    pub fn new() -> Self {
        Self {
            groups: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Recover pending groups from the state store on startup.
    ///
    /// This method scans the state store for pending group entries and
    /// repopulates the in-memory cache. Should be called during gateway
    /// initialization to prevent data loss after restarts.
    ///
    /// If an encryptor is provided, group metadata is decrypted before parsing.
    pub async fn recover_groups(
        &self,
        state: &dyn StateStore,
        namespace: &str,
        tenant: &str,
        encryptor: Option<&PayloadEncryptor>,
    ) -> Result<usize, GatewayError> {
        let entries = state
            .scan_keys(namespace, tenant, KeyKind::Group, None)
            .await?;

        let mut recovered = 0;
        let mut groups = self.groups.write();

        for (key, raw_value) in entries {
            // Decrypt if encrypted, then parse the group metadata from stored JSON.
            let value = if let Some(enc) = encryptor {
                enc.decrypt_str(&raw_value).unwrap_or(raw_value)
            } else {
                raw_value
            };
            if let Ok(metadata) = serde_json::from_str::<serde_json::Value>(&value) {
                let group_id = metadata
                    .get("group_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let group_key = metadata
                    .get("group_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let notify_at = metadata
                    .get("notify_at")
                    .and_then(|v| v.as_str())
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map_or_else(Utc::now, |dt| dt.with_timezone(&Utc));
                let trace_context: HashMap<String, String> = metadata
                    .get("trace_context")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();

                // Restore events and labels if present (backward compatible:
                // old entries without these keys get empty defaults).
                let events: Vec<GroupedEvent> = metadata
                    .get("events")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                let labels: HashMap<String, String> = metadata
                    .get("labels")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();

                // Only recover if not already in memory
                if !groups.contains_key(&group_key) {
                    let event_count = events.len();
                    let mut group = EventGroup::new(&group_id, &group_key, notify_at);
                    group.trace_context = trace_context;
                    group.labels = labels;
                    for event in events {
                        group.add_event(event);
                    }
                    groups.insert(group_key.clone(), group);
                    recovered += 1;
                    tracing::info!(
                        group_id = %group_id,
                        group_key = %group_key,
                        event_count,
                        "recovered pending group from state store"
                    );
                }
            } else {
                tracing::warn!(key = %key, "failed to parse group metadata");
            }
        }

        Ok(recovered)
    }

    /// Add an event to a group based on the `group_by` fields.
    ///
    /// Returns a tuple of (group ID, group key, current group size, notify at time).
    /// If an encryptor is provided, group metadata is encrypted before storage.
    ///
    /// ## Scheduling
    ///
    /// The `notify_at` timestamp is computed differently depending on
    /// the group's current state:
    ///
    /// - **New group**: `notify_at = now + group_wait_seconds`
    /// - **Existing Pending group**: `notify_at` is unchanged — the
    ///   group is already waiting its `group_wait` window.
    /// - **Existing Notified group** (persistent, has `repeat_interval`):
    ///   the group transitions back to Pending. `notify_at` is set to
    ///   `max(last_notified_at + group_interval_seconds, now)`, so the
    ///   next flush respects the batching window since the last flush.
    ///
    /// The `max_group_size` cap is enforced by
    /// [`EventGroup::add_event`], which drops the oldest event when
    /// the group is at capacity.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub async fn add_to_group(
        &self,
        action: &Action,
        group_by: &[String],
        group_wait_seconds: u64,
        group_interval_seconds: u64,
        repeat_interval_seconds: Option<u64>,
        max_group_size: usize,
        state: &dyn StateStore,
        encryptor: Option<&PayloadEncryptor>,
    ) -> Result<(String, String, usize, DateTime<Utc>), GatewayError> {
        // Compute group key from action fields
        let group_key = compute_group_key(action, group_by);

        // Create grouped event from action
        let grouped_event = GroupedEvent::new(action.id.clone(), action.payload.clone())
            .with_fingerprint(
                action
                    .fingerprint
                    .clone()
                    .unwrap_or_else(|| compute_fingerprint(action, group_by)),
            );
        let grouped_event = if let Some(status) = &action.status {
            grouped_event.with_status(status)
        } else {
            grouped_event
        };

        // Check if group exists or create new one.
        // We capture events + labels snapshots while holding the lock so
        // they can be persisted (encrypted) alongside the metadata.
        let (group_id, group_size, notify_at, events_snapshot, labels_snapshot, trace_ctx) = {
            let mut groups = self.groups.write();

            if let Some(group) = groups.get_mut(&group_key) {
                // Existing group: handle the Notified → Pending
                // transition for persistent groups, then append the
                // event (dropping oldest if at capacity).
                let now = Utc::now();
                if matches!(group.state, GroupState::Notified) {
                    // Only persistent groups (with repeat_interval) can
                    // be in Notified state at this point; ephemeral
                    // groups are deleted after flush. Resurrect with
                    // the next flush scheduled group_interval from
                    // last_notified (or now if that's in the past).
                    group.state = GroupState::Pending;
                    #[allow(clippy::cast_possible_wrap)]
                    let interval = chrono::Duration::seconds(group.group_interval_seconds as i64);
                    let candidate = group
                        .last_notified_at
                        .map_or_else(|| now + interval, |t| t + interval);
                    group.notify_at = candidate.max(now);
                }
                group.add_event(grouped_event);
                let events = group.events.clone();
                let labels = group.labels.clone();
                let trace = group.trace_context.clone();
                (
                    group.group_id.clone(),
                    group.size(),
                    group.notify_at,
                    events,
                    labels,
                    trace,
                )
            } else {
                // New group. Record timing params from the rule so
                // future transitions don't need to re-plumb them.
                let group_id = Uuid::new_v4().to_string();
                #[allow(clippy::cast_possible_wrap)]
                let notify_at = Utc::now() + chrono::Duration::seconds(group_wait_seconds as i64);
                let mut group = EventGroup::new(&group_id, &group_key, notify_at).with_timing(
                    group_wait_seconds,
                    group_interval_seconds,
                    repeat_interval_seconds,
                    max_group_size,
                );

                // Capture trace context from the first event in the group
                group.trace_context.clone_from(&action.trace_context);

                // Extract common labels from action metadata
                let mut labels = HashMap::new();
                for field in group_by {
                    if let Some(value) = extract_field_value(action, field) {
                        labels.insert(field.clone(), value);
                    }
                }
                group = group.with_labels(labels);
                group.add_event(grouped_event);

                let size = group.size();
                let notify = group.notify_at;
                let events = group.events.clone();
                let labels = group.labels.clone();
                let trace = group.trace_context.clone();
                groups.insert(group_key.clone(), group);

                (group_id, size, notify, events, labels, trace)
            }
        };

        // Persist group state (encrypted if encryptor is provided).
        // Events and labels are included so they survive crash recovery.
        let state_key = StateKey::new(
            action.namespace.as_str(),
            action.tenant.as_str(),
            KeyKind::Group,
            &group_key,
        );
        let group_value = serde_json::json!({
            "group_id": &group_id,
            "group_key": &group_key,
            "size": group_size,
            "notify_at": notify_at.to_rfc3339(),
            "trace_context": &trace_ctx,
            "labels": &labels_snapshot,
            "events": &events_snapshot,
        });
        let group_value_str = if let Some(enc) = encryptor {
            enc.encrypt_str(&group_value.to_string()).map_err(|e| {
                GatewayError::Configuration(format!("group metadata encryption failed: {e}"))
            })?
        } else {
            group_value.to_string()
        };
        state.set(&state_key, &group_value_str, None).await?;

        // Add to pending groups index
        let pending_key = StateKey::new(
            action.namespace.as_str(),
            action.tenant.as_str(),
            KeyKind::PendingGroups,
            &group_key,
        );
        state
            .set(&pending_key, &notify_at.to_rfc3339(), None)
            .await?;

        Ok((group_id, group_key, group_size, notify_at))
    }

    /// Get a group by its key.
    #[must_use]
    pub fn get_group(&self, group_key: &str) -> Option<EventGroup> {
        self.groups.read().get(group_key).cloned()
    }

    /// List all pending groups.
    #[must_use]
    pub fn list_pending_groups(&self) -> Vec<EventGroup> {
        self.groups
            .read()
            .values()
            .filter(|g| matches!(g.state, GroupState::Pending))
            .cloned()
            .collect()
    }

    /// Get all groups that are ready to be flushed.
    ///
    /// A group is ready if `notify_at <= now` and it is either in
    /// `Pending` state (normal flush) or in `Notified` state with a
    /// `repeat_interval_seconds` set (forced re-notification of a
    /// persistent group that has had no new events).
    #[must_use]
    pub fn get_ready_groups(&self) -> Vec<EventGroup> {
        let now = Utc::now();
        self.groups
            .read()
            .values()
            .filter(|g| {
                if g.notify_at > now {
                    return false;
                }
                match g.state {
                    GroupState::Pending => true,
                    GroupState::Notified => g.is_persistent(),
                    GroupState::Resolved => false,
                }
            })
            .cloned()
            .collect()
    }

    /// Flush a group — mark as notified and return the current event snapshot.
    ///
    /// For persistent groups (`repeat_interval_seconds` set), the next
    /// `notify_at` is scheduled at `now + repeat_interval_seconds` so
    /// the group continues to fire periodic reminders with no new
    /// events. For ephemeral groups, the state machine still
    /// transitions to `Notified`, but the caller (see
    /// `flush_ready_groups` in the background worker) is expected to
    /// delete the group shortly after.
    ///
    /// Returns the group snapshot if it was ready for flush, or
    /// `None` if the group no longer exists or is in a state that
    /// rejects flushing.
    pub fn flush_group(&self, group_key: &str) -> Option<EventGroup> {
        let mut groups = self.groups.write();
        let group = groups.get_mut(group_key)?;

        let flushable = matches!(group.state, GroupState::Pending)
            || (matches!(group.state, GroupState::Notified) && group.is_persistent());
        if !flushable {
            return None;
        }

        let now = Utc::now();
        group.state = GroupState::Notified;
        group.last_notified_at = Some(now);
        group.updated_at = now;

        // Schedule the next re-notification if this is a persistent group.
        // Ephemeral groups have their notify_at left unchanged; they are
        // deleted by the background worker after this call.
        if let Some(repeat) = group.repeat_interval_seconds {
            #[allow(clippy::cast_possible_wrap)]
            let next = now + chrono::Duration::seconds(repeat as i64);
            group.notify_at = next;
        }

        Some(group.clone())
    }

    /// Remove a group after it has been fully processed.
    pub fn remove_group(&self, group_key: &str) -> Option<EventGroup> {
        self.groups.write().remove(group_key)
    }

    /// Get the number of active groups.
    #[must_use]
    pub fn active_group_count(&self) -> usize {
        self.groups.read().len()
    }
}

/// Compute a group key from action fields.
fn compute_group_key(action: &Action, group_by: &[String]) -> String {
    let mut hasher = Sha256::new();

    // Include namespace and tenant in the key
    hasher.update(action.namespace.as_str().as_bytes());
    hasher.update(b":");
    hasher.update(action.tenant.as_str().as_bytes());
    hasher.update(b":");

    for field in group_by {
        if let Some(value) = extract_field_value(action, field) {
            hasher.update(field.as_bytes());
            hasher.update(b"=");
            hasher.update(value.as_bytes());
            hasher.update(b";");
        }
    }

    hex::encode(hasher.finalize())
}

/// Extract a field value from an action by path.
fn extract_field_value(action: &Action, field: &str) -> Option<String> {
    match field {
        "namespace" => Some(action.namespace.as_str().to_string()),
        "tenant" => Some(action.tenant.as_str().to_string()),
        "provider" => Some(action.provider.as_str().to_string()),
        "action_type" => Some(action.action_type.clone()),
        "status" => action.status.clone(),
        path if path.starts_with("metadata.") => {
            let key = &path[9..];
            action.metadata.labels.get(key).cloned()
        }
        path if path.starts_with("payload.") => {
            let json_path = &path[8..];
            extract_json_value(&action.payload, json_path)
        }
        _ => None,
    }
}

/// Extract a value from JSON by dot-separated path.
fn extract_json_value(value: &serde_json::Value, path: &str) -> Option<String> {
    let mut current = value;

    for part in path.split('.') {
        match current {
            serde_json::Value::Object(map) => {
                current = map.get(part)?;
            }
            serde_json::Value::Array(arr) => {
                let idx = part.parse::<usize>().ok()?;
                current = arr.get(idx)?;
            }
            _ => return None,
        }
    }

    match current {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        serde_json::Value::Null => None,
        _ => Some(current.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_core::ActionMetadata;
    use acteon_state_memory::MemoryStateStore;

    fn test_action() -> Action {
        let mut labels = HashMap::new();
        labels.insert("cluster".to_string(), "prod-1".to_string());
        labels.insert("severity".to_string(), "critical".to_string());

        Action::new(
            "notifications",
            "tenant-1",
            "email",
            "alert",
            serde_json::json!({"host": "server-1"}),
        )
        .with_metadata(ActionMetadata { labels })
    }

    #[test]
    fn compute_group_key_basic() {
        let action = test_action();
        let key = compute_group_key(&action, &["action_type".to_string()]);
        assert!(!key.is_empty());

        // Same action, same fields should produce same key
        let key2 = compute_group_key(&action, &["action_type".to_string()]);
        assert_eq!(key, key2);
    }

    #[test]
    fn compute_group_key_with_metadata() {
        let action = test_action();
        let key = compute_group_key(
            &action,
            &[
                "action_type".to_string(),
                "metadata.cluster".to_string(),
                "metadata.severity".to_string(),
            ],
        );
        assert!(!key.is_empty());
    }

    #[tokio::test]
    async fn add_to_group_creates_new_group() {
        let manager = GroupManager::new();
        let state = MemoryStateStore::new();
        let action = test_action();

        let (group_id, _group_key, size, notify_at) = manager
            .add_to_group(
                &action,
                &["metadata.cluster".to_string()],
                60,
                300,
                None,
                100,
                &state,
                None,
            )
            .await
            .unwrap();

        assert!(!group_id.is_empty());
        assert_eq!(size, 1);
        assert!(notify_at > Utc::now());
        assert_eq!(manager.active_group_count(), 1);
    }

    #[tokio::test]
    async fn add_to_group_adds_to_existing() {
        let manager = GroupManager::new();
        let state = MemoryStateStore::new();
        let action1 = test_action();
        let action2 = test_action();

        let (group_id1, _, size1, _) = manager
            .add_to_group(
                &action1,
                &["metadata.cluster".to_string()],
                60,
                300,
                None,
                100,
                &state,
                None,
            )
            .await
            .unwrap();

        let (group_id2, _, size2, _) = manager
            .add_to_group(
                &action2,
                &["metadata.cluster".to_string()],
                60,
                300,
                None,
                100,
                &state,
                None,
            )
            .await
            .unwrap();

        assert_eq!(group_id1, group_id2);
        assert_eq!(size1, 1);
        assert_eq!(size2, 2);
        assert_eq!(manager.active_group_count(), 1);
    }

    #[test]
    fn list_pending_groups() {
        let manager = GroupManager::new();
        let notify_at = Utc::now() + chrono::Duration::seconds(60);
        let mut group = EventGroup::new("group-1", "key-1", notify_at);
        group.add_event(GroupedEvent::new(
            acteon_core::types::ActionId::new("action-1".to_string()),
            serde_json::json!({}),
        ));

        manager.groups.write().insert("key-1".to_string(), group);

        let pending = manager.list_pending_groups();
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn flush_group() {
        let manager = GroupManager::new();
        let notify_at = Utc::now() - chrono::Duration::seconds(10);
        let mut group = EventGroup::new("group-1", "key-1", notify_at);
        group.add_event(GroupedEvent::new(
            acteon_core::types::ActionId::new("action-1".to_string()),
            serde_json::json!({}),
        ));

        manager.groups.write().insert("key-1".to_string(), group);

        let flushed = manager.flush_group("key-1");
        assert!(flushed.is_some());
        assert!(matches!(flushed.unwrap().state, GroupState::Notified));

        // Second flush should return None — ephemeral group cannot
        // be re-flushed once Notified.
        let flushed2 = manager.flush_group("key-1");
        assert!(flushed2.is_none());
    }

    // =========================================================================
    // Phase 2: group_interval, repeat_interval, max_group_size
    // =========================================================================

    #[tokio::test]
    async fn persistent_group_survives_flush() {
        // A group with repeat_interval set is kept alive after flush
        // so it can re-fire with the same or additional events later.
        let manager = GroupManager::new();
        let state = MemoryStateStore::new();
        let action = test_action();

        let (_, _, _, _) = manager
            .add_to_group(
                &action,
                &["metadata.cluster".to_string()],
                60,         // group_wait
                300,        // group_interval
                Some(3600), // repeat_interval → persistent
                100,
                &state,
                None,
            )
            .await
            .unwrap();

        let group_key = manager
            .groups
            .read()
            .keys()
            .next()
            .cloned()
            .expect("group created");

        let flushed = manager.flush_group(&group_key).expect("flushable");
        assert!(matches!(flushed.state, GroupState::Notified));
        assert!(flushed.is_persistent());

        // Group is still in the manager after flush — ready to re-fire.
        assert!(manager.get_group(&group_key).is_some());
    }

    #[tokio::test]
    async fn persistent_flush_advances_notify_at_by_repeat_interval() {
        let manager = GroupManager::new();
        let state = MemoryStateStore::new();
        let action = test_action();
        let before = Utc::now();

        manager
            .add_to_group(
                &action,
                &["metadata.cluster".to_string()],
                60,
                300,
                Some(1_800), // 30 minutes
                100,
                &state,
                None,
            )
            .await
            .unwrap();

        let group_key = manager.groups.read().keys().next().cloned().unwrap();
        let flushed = manager.flush_group(&group_key).unwrap();

        // last_notified_at set to ~now, notify_at = last_notified + 1800s.
        let last = flushed.last_notified_at.expect("last_notified populated");
        assert!(last >= before);
        assert!(flushed.notify_at >= last + chrono::Duration::seconds(1_800));
        assert!(flushed.notify_at < last + chrono::Duration::seconds(1_801));
    }

    #[tokio::test]
    async fn notified_persistent_group_reopens_on_new_event() {
        // After a flush, a new event on the same group key transitions
        // the group back to Pending and recomputes notify_at based on
        // `last_notified + group_interval` (bounded to at least now).
        let manager = GroupManager::new();
        let state = MemoryStateStore::new();
        let action = test_action();

        manager
            .add_to_group(
                &action,
                &["metadata.cluster".to_string()],
                60,
                120, // 2-minute group_interval
                Some(3_600),
                100,
                &state,
                None,
            )
            .await
            .unwrap();

        let group_key = manager.groups.read().keys().next().cloned().unwrap();
        let flushed = manager.flush_group(&group_key).unwrap();
        assert!(matches!(flushed.state, GroupState::Notified));
        let last_notified = flushed.last_notified_at.unwrap();

        // New event arrives on the same group.
        let action2 = test_action();
        let (_, _, size, notify_at) = manager
            .add_to_group(
                &action2,
                &["metadata.cluster".to_string()],
                60,
                120,
                Some(3_600),
                100,
                &state,
                None,
            )
            .await
            .unwrap();

        // Group size is now 2 (original 1 + new one, no cap hit).
        assert_eq!(size, 2);

        // State is back to Pending.
        let group = manager.get_group(&group_key).unwrap();
        assert!(matches!(group.state, GroupState::Pending));

        // notify_at is at least last_notified + group_interval.
        let expected = last_notified + chrono::Duration::seconds(120);
        assert!(
            notify_at >= expected - chrono::Duration::seconds(1),
            "notify_at should be >= last_notified + group_interval"
        );
    }

    #[tokio::test]
    async fn ephemeral_group_state_unchanged() {
        // Backward compat: a group without repeat_interval behaves
        // exactly as Phase 1 — single flush and the caller deletes.
        let manager = GroupManager::new();
        let state = MemoryStateStore::new();
        let action = test_action();

        manager
            .add_to_group(
                &action,
                &["metadata.cluster".to_string()],
                60,
                300,
                None, // no repeat_interval → ephemeral
                100,
                &state,
                None,
            )
            .await
            .unwrap();

        let group_key = manager.groups.read().keys().next().cloned().unwrap();
        let flushed = manager.flush_group(&group_key).unwrap();
        assert!(!flushed.is_persistent());
        assert!(matches!(flushed.state, GroupState::Notified));

        // Second flush returns None — ephemeral groups cannot re-fire.
        assert!(manager.flush_group(&group_key).is_none());
    }

    #[tokio::test]
    async fn max_group_size_drops_oldest_event() {
        // Filling a group beyond its max_group_size drops the oldest
        // event, not the newest. This matches Alertmanager's cap
        // semantics and is important for long-running persistent
        // groups whose event list would otherwise grow unbounded.
        let manager = GroupManager::new();
        let state = MemoryStateStore::new();

        // Fill to exactly max_group_size=3 with 5 events.
        for i in 0..5 {
            let mut action = test_action();
            action.id = acteon_core::types::ActionId::new(format!("action-{i}"));
            manager
                .add_to_group(
                    &action,
                    &["metadata.cluster".to_string()],
                    60,
                    300,
                    Some(3_600),
                    3, // max_group_size
                    &state,
                    None,
                )
                .await
                .unwrap();
        }

        let group_key = manager.groups.read().keys().next().cloned().unwrap();
        let group = manager.get_group(&group_key).unwrap();
        assert_eq!(group.size(), 3, "size should be capped at max_group_size");

        // The retained events should be the LAST three (action-2, -3, -4).
        let retained_ids: Vec<String> = group
            .events
            .iter()
            .map(|e| e.action_id.as_str().to_string())
            .collect();
        assert_eq!(
            retained_ids,
            vec!["action-2", "action-3", "action-4"],
            "oldest events should be dropped"
        );
    }

    #[tokio::test]
    async fn get_ready_groups_picks_up_persistent_at_repeat_time() {
        // A persistent group in Notified state should be picked up
        // by get_ready_groups once its notify_at (= last_notified +
        // repeat_interval) arrives.
        let manager = GroupManager::new();
        let now = Utc::now();

        // Manually insert a persistent group in Notified state with
        // notify_at already in the past, simulating a repeat_interval
        // that just elapsed.
        let mut group = EventGroup::new("group-1", "key-1", now - chrono::Duration::seconds(5))
            .with_timing(60, 300, Some(600), 100);
        group.state = GroupState::Notified;
        group.last_notified_at = Some(now - chrono::Duration::seconds(605));
        group.add_event(GroupedEvent::new(
            acteon_core::types::ActionId::new("a".to_string()),
            serde_json::json!({}),
        ));
        manager.groups.write().insert("key-1".to_string(), group);

        let ready = manager.get_ready_groups();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].group_key, "key-1");
    }

    #[tokio::test]
    async fn get_ready_groups_ignores_ephemeral_in_notified_state() {
        // An ephemeral group that somehow remained in Notified state
        // (e.g., the flush worker was interrupted between flush and
        // remove) should NOT be re-picked by get_ready_groups —
        // they fire once and only once.
        let manager = GroupManager::new();
        let now = Utc::now();

        let mut group = EventGroup::new("group-1", "key-1", now - chrono::Duration::seconds(5));
        // Ephemeral: no repeat_interval.
        group.state = GroupState::Notified;
        manager.groups.write().insert("key-1".to_string(), group);

        let ready = manager.get_ready_groups();
        assert!(
            ready.is_empty(),
            "ephemeral Notified group must not re-fire"
        );
    }
}
