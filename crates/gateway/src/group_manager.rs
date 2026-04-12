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
            // Try to deserialize the full EventGroup first (Phase 2+
            // write format). Fall back to the old partial-JSON format
            // for backward compatibility with records written by the
            // pre-Phase-2 server — that format did not include the
            // `state`, `created_at`, or `updated_at` fields at the
            // top level so direct deserialization fails.
            let group: Option<EventGroup> = serde_json::from_str::<EventGroup>(&value)
                .ok()
                .or_else(|| parse_legacy_group(&value));

            match group {
                Some(group) => {
                    let group_key_local = group.group_key.clone();
                    if !groups.contains_key(&group_key_local) {
                        let event_count = group.size();
                        let group_id_local = group.group_id.clone();
                        groups.insert(group_key_local.clone(), group);
                        recovered += 1;
                        tracing::info!(
                            group_id = %group_id_local,
                            group_key = %group_key_local,
                            event_count,
                            "recovered pending group from state store"
                        );
                    }
                }
                None => {
                    tracing::warn!(key = %key, "failed to parse group metadata");
                }
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
        // We capture a full snapshot of the updated group while holding
        // the lock so the entire record can be persisted — including
        // the Phase-2 timing fields (`last_notified_at`, `idle_flushes`,
        // timing parameters, etc.) that would otherwise be lost on restart.
        let group_snapshot: EventGroup = {
            let mut groups = self.groups.write();

            if let Some(group) = groups.get_mut(&group_key) {
                // Existing group: handle the Notified → Pending
                // transition for persistent groups, then append the
                // event (dropping oldest if at capacity).
                let now = Utc::now();
                if matches!(group.state, GroupState::Notified) {
                    // Only persistent groups (with repeat_interval) can
                    // be in Notified state at this point; ephemeral
                    // groups are deleted after flush by the worker.
                    //
                    // `group_interval_seconds` is the quiet period
                    // that begins when a NEW event arrives on an
                    // already-flushed group, not the minimum time
                    // between successive flushes. So the next flush
                    // fires at `now + group_interval` regardless of
                    // how long ago the last flush was.
                    group.state = GroupState::Pending;
                    #[allow(clippy::cast_possible_wrap)]
                    let interval = chrono::Duration::seconds(group.group_interval_seconds as i64);
                    group.notify_at = now + interval;
                    // Reset the idle-flush counter; this group just
                    // saw activity.
                    group.idle_flushes = 0;
                }
                group.add_event(grouped_event);
                group.clone()
            } else {
                // New group. Record timing params + scope from the
                // action so future transitions / restarts don't need
                // to re-plumb them.
                let group_id = Uuid::new_v4().to_string();
                #[allow(clippy::cast_possible_wrap)]
                let notify_at = Utc::now() + chrono::Duration::seconds(group_wait_seconds as i64);
                let mut group = EventGroup::new(&group_id, &group_key, notify_at)
                    .with_timing(
                        group_wait_seconds,
                        group_interval_seconds,
                        repeat_interval_seconds,
                        max_group_size,
                    )
                    .with_scope(action.namespace.as_str(), action.tenant.as_str());

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

                let snapshot = group.clone();
                groups.insert(group_key.clone(), group);
                snapshot
            }
        };

        let group_id = group_snapshot.group_id.clone();
        let group_size = group_snapshot.size();
        let notify_at = group_snapshot.notify_at;

        // Persist the full EventGroup (encrypted if encryptor is
        // provided). Writing the entire record — not just a subset of
        // fields — ensures that `last_notified_at`, `idle_flushes`,
        // and the Phase-2 timing fields survive a gateway restart.
        persist_group(&group_snapshot, state, encryptor).await?;

        // Update the pending-groups index so the recover path can
        // find this group on startup.
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

    /// Rebuild the in-memory cache from the state store.
    ///
    /// Called periodically from the background processor (controlled
    /// by `enable_group_sync`) so that changes made by peer gateway
    /// instances become visible locally. Required for HA deployments
    /// to prevent split-brain behavior where instance A flushes a
    /// group but instance B's local cache still shows it as pending.
    ///
    /// Returns the number of groups loaded.
    ///
    /// # Errors
    ///
    /// Returns [`GatewayError`] if the state store scan fails.
    /// Individual records that fail to parse are logged and skipped.
    pub async fn sync_groups_from_store(
        &self,
        state: &dyn StateStore,
        encryptor: Option<&PayloadEncryptor>,
    ) -> Result<usize, GatewayError> {
        let entries = state
            .scan_keys_by_kind(KeyKind::Group)
            .await
            .map_err(|e| GatewayError::Configuration(format!("group sync scan failed: {e}")))?;

        let mut new_cache: HashMap<String, EventGroup> = HashMap::new();

        for (_key, raw_value) in entries {
            let value = if let Some(enc) = encryptor {
                enc.decrypt_str(&raw_value).unwrap_or(raw_value)
            } else {
                raw_value
            };

            // Try full EventGroup JSON first; fall back to legacy
            // partial-JSON format for pre-Phase-2 records.
            let group: Option<EventGroup> = serde_json::from_str::<EventGroup>(&value)
                .ok()
                .or_else(|| parse_legacy_group(&value));

            match group {
                Some(g) => {
                    new_cache.insert(g.group_key.clone(), g);
                }
                None => {
                    tracing::warn!("group sync: failed to parse group record");
                }
            }
        }

        let count = new_cache.len();
        *self.groups.write() = new_cache;
        Ok(count)
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

        let was_already_notified = matches!(group.state, GroupState::Notified);
        let flushable = matches!(group.state, GroupState::Pending)
            || (was_already_notified && group.is_persistent());
        if !flushable {
            return None;
        }

        let now = Utc::now();
        group.state = GroupState::Notified;
        group.last_notified_at = Some(now);
        group.updated_at = now;

        // If this flush was triggered by the repeat interval (group
        // was already in Notified state), increment the idle-flush
        // counter. The background worker will evict the group once
        // this exceeds MAX_IDLE_FLUSHES, preventing persistent groups
        // with dynamic keys from leaking forever.
        //
        // Ephemeral groups never reach this branch because they are
        // in Pending state until their single flush; ephemeral groups
        // stuck in Notified are rejected above.
        if was_already_notified {
            group.idle_flushes = group.idle_flushes.saturating_add(1);
        }

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

/// Parse a pre-Phase-2 legacy partial-JSON group record.
///
/// Old records looked like:
/// ```json
/// {
///   "group_id": "...",
///   "group_key": "...",
///   "size": 3,
///   "notify_at": "2026-04-01T00:00:00Z",
///   "trace_context": {...},
///   "labels": {...},
///   "events": [...]
/// }
/// ```
///
/// They lack the Phase-2 timing fields, `state`, `created_at`,
/// `updated_at`, etc. This helper reconstructs an [`EventGroup`]
/// with sensible defaults for the missing fields — the group is
/// restored as ephemeral (no `repeat_interval`), in Pending state,
/// which is the safest assumption since we don't know the original
/// rule's timing parameters.
fn parse_legacy_group(raw: &str) -> Option<EventGroup> {
    let metadata: serde_json::Value = serde_json::from_str(raw).ok()?;
    let group_id = metadata.get("group_id")?.as_str()?.to_string();
    let group_key = metadata.get("group_key")?.as_str()?.to_string();
    let notify_at = metadata
        .get("notify_at")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map_or_else(Utc::now, |dt| dt.with_timezone(&Utc));

    let trace_context: HashMap<String, String> = metadata
        .get("trace_context")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let events: Vec<GroupedEvent> = metadata
        .get("events")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let labels: HashMap<String, String> = metadata
        .get("labels")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let mut group = EventGroup::new(&group_id, &group_key, notify_at);
    group.trace_context = trace_context;
    group.labels = labels;
    for event in events {
        group.add_event(event);
    }
    Some(group)
}

/// Persist a full [`EventGroup`] to the state store.
///
/// This is the shared write path used by [`GroupManager::add_to_group`]
/// and the background flush worker after calling
/// [`GroupManager::flush_group`]. Persisting the full record (not a
/// subset of fields) ensures that Phase-2 timing state
/// (`last_notified_at`, `idle_flushes`, etc.) survives a gateway
/// restart, so a persistent group with `repeat_interval_seconds: 3600`
/// does not fire immediately after a restart just because the reloaded
/// `last_notified_at` was `None`.
///
/// The group's `namespace` and `tenant` fields must be set — they
/// determine the state-store key. [`EventGroup::with_scope`] sets both.
///
/// # Errors
///
/// Returns [`GatewayError`] if JSON serialization, encryption, or the
/// state store write fails.
pub async fn persist_group(
    group: &EventGroup,
    state: &dyn StateStore,
    encryptor: Option<&PayloadEncryptor>,
) -> Result<(), GatewayError> {
    let state_key = StateKey::new(
        group.namespace.as_str(),
        group.tenant.as_str(),
        KeyKind::Group,
        &group.group_key,
    );
    let group_json = serde_json::to_string(group)
        .map_err(|e| GatewayError::Configuration(format!("group serialize failed: {e}")))?;
    let value = if let Some(enc) = encryptor {
        enc.encrypt_str(&group_json).map_err(|e| {
            GatewayError::Configuration(format!("group metadata encryption failed: {e}"))
        })?
    } else {
        group_json
    };
    state.set(&state_key, &value, None).await?;
    Ok(())
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
        // the group back to Pending. notify_at is set to
        // `now + group_interval_seconds` — the quiet window begins
        // when the new event arrives.
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

        // New event arrives on the same group.
        let before_add = Utc::now();
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

        // notify_at is `now_when_event_arrived + group_interval`.
        // Allow a small fudge for the computation delay.
        let expected_min = before_add + chrono::Duration::seconds(120);
        let expected_max = Utc::now() + chrono::Duration::seconds(121);
        assert!(
            notify_at >= expected_min && notify_at <= expected_max,
            "notify_at should be roughly `now + group_interval`; got {notify_at}, expected around {expected_min}..={expected_max}"
        );
    }

    #[tokio::test]
    async fn group_interval_uses_new_event_arrival_time_not_last_notified() {
        // Regression test for issue #5: previously `notify_at` was
        // computed from `last_notified + group_interval`, which meant
        // that if the group had been quiescent longer than
        // `group_interval`, a new event would fire *immediately*
        // (notify_at in the past). That defeats the batching purpose
        // of `group_interval`.
        //
        // With the fix, a new event always gets a fresh
        // `group_interval` batch window regardless of how long ago
        // the last flush was.
        let manager = GroupManager::new();
        let state = MemoryStateStore::new();

        // Set up a persistent group and flush it, then rewrite
        // last_notified_at to something far in the past to simulate
        // a long-quiescent group.
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
        manager.flush_group(&group_key).unwrap();

        // Rewrite last_notified_at to 1 hour ago.
        {
            let mut groups = manager.groups.write();
            let group = groups.get_mut(&group_key).unwrap();
            group.last_notified_at = Some(Utc::now() - chrono::Duration::hours(1));
            group.notify_at = Utc::now() - chrono::Duration::minutes(55);
        }

        // New event arrives. Without the fix, notify_at would be
        // `last_notified + group_interval` = "58 minutes ago" →
        // bounded to "now" → immediate flush with zero batching.
        // With the fix, notify_at = now + 120s.
        let before_add = Utc::now();
        let action2 = test_action();
        let (_, _, _, notify_at) = manager
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

        let min_expected = before_add + chrono::Duration::seconds(119);
        assert!(
            notify_at >= min_expected,
            "new event should get fresh group_interval batching window; notify_at={notify_at}, min_expected={min_expected}"
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
        // event, not the newest. This bounds memory on long-running
        // persistent groups whose event list would otherwise grow
        // unbounded.
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

    // =========================================================================
    // Review-follow-up regression tests
    // =========================================================================

    #[tokio::test]
    async fn idle_flushes_increment_on_repeat_fire() {
        // A persistent group that re-fires on its repeat interval
        // with no new events should see idle_flushes increment on
        // each subsequent flush.
        let manager = GroupManager::new();
        let state = MemoryStateStore::new();
        let action = test_action();

        manager
            .add_to_group(
                &action,
                &["metadata.cluster".to_string()],
                60,
                300,
                Some(3_600),
                100,
                &state,
                None,
            )
            .await
            .unwrap();
        let group_key = manager.groups.read().keys().next().cloned().unwrap();

        // First flush transitions Pending → Notified. Does NOT count
        // as idle — the group had unflushed events.
        let f1 = manager.flush_group(&group_key).unwrap();
        assert_eq!(f1.idle_flushes, 0);

        // Second flush: group was already Notified, no new events.
        // This is a repeat-interval fire → idle_flushes increments.
        let f2 = manager.flush_group(&group_key).unwrap();
        assert_eq!(f2.idle_flushes, 1);

        // Third flush → idle_flushes = 2.
        let f3 = manager.flush_group(&group_key).unwrap();
        assert_eq!(f3.idle_flushes, 2);
    }

    #[tokio::test]
    async fn idle_flushes_reset_on_new_event() {
        // A new event on an idle persistent group resets the counter.
        let manager = GroupManager::new();
        let state = MemoryStateStore::new();
        let action = test_action();

        manager
            .add_to_group(
                &action,
                &["metadata.cluster".to_string()],
                60,
                300,
                Some(3_600),
                100,
                &state,
                None,
            )
            .await
            .unwrap();
        let group_key = manager.groups.read().keys().next().cloned().unwrap();

        // Flush + re-fire to accumulate idle flushes.
        manager.flush_group(&group_key).unwrap();
        manager.flush_group(&group_key).unwrap();
        manager.flush_group(&group_key).unwrap();
        let before = manager.get_group(&group_key).unwrap();
        assert_eq!(before.idle_flushes, 2);

        // New event arrives → counter should reset.
        let action2 = test_action();
        manager
            .add_to_group(
                &action2,
                &["metadata.cluster".to_string()],
                60,
                300,
                Some(3_600),
                100,
                &state,
                None,
            )
            .await
            .unwrap();

        let after = manager.get_group(&group_key).unwrap();
        assert_eq!(after.idle_flushes, 0);
    }

    #[tokio::test]
    async fn should_evict_fires_after_max_idle_flushes() {
        use acteon_core::group::MAX_IDLE_FLUSHES;

        let manager = GroupManager::new();
        let state = MemoryStateStore::new();
        let action = test_action();

        manager
            .add_to_group(
                &action,
                &["metadata.cluster".to_string()],
                60,
                300,
                Some(3_600),
                100,
                &state,
                None,
            )
            .await
            .unwrap();
        let group_key = manager.groups.read().keys().next().cloned().unwrap();

        // Initial flush is NOT idle.
        let first = manager.flush_group(&group_key).unwrap();
        assert!(!first.should_evict());

        // Subsequent idle re-fires accumulate — should_evict becomes
        // true after MAX_IDLE_FLUSHES additional flushes.
        for i in 1..=MAX_IDLE_FLUSHES {
            let flushed = manager.flush_group(&group_key).unwrap();
            assert_eq!(flushed.idle_flushes, i);
        }
        let final_state = manager.get_group(&group_key).unwrap();
        assert!(
            final_state.should_evict(),
            "should_evict should fire after {MAX_IDLE_FLUSHES} idle flushes"
        );
    }

    #[tokio::test]
    async fn persisted_group_preserves_timing_fields_across_reload() {
        // Regression test for issue #3: the persist path must write
        // `last_notified_at`, `idle_flushes`, and the Phase-2 timing
        // fields so a restart doesn't reset them.
        let manager = GroupManager::new();
        let state = MemoryStateStore::new();
        let action = test_action();

        manager
            .add_to_group(
                &action,
                &["metadata.cluster".to_string()],
                60,
                120,
                Some(1_800),
                50,
                &state,
                None,
            )
            .await
            .unwrap();
        let group_key = manager.groups.read().keys().next().cloned().unwrap();
        let flushed = manager.flush_group(&group_key).unwrap();
        let original_last_notified = flushed.last_notified_at.unwrap();
        let original_notify_at = flushed.notify_at;

        // Simulate a restart by persisting then recovering into a
        // fresh manager. persist_group mirrors what the background
        // flush worker does after a successful flush.
        persist_group(&flushed, &state, None).await.unwrap();

        let fresh = GroupManager::new();
        fresh
            .recover_groups(&state, "notifications", "tenant-1", None)
            .await
            .unwrap();
        let reloaded = fresh.get_group(&group_key).expect("group recovered");

        assert_eq!(reloaded.last_notified_at, Some(original_last_notified));
        assert_eq!(reloaded.notify_at, original_notify_at);
        assert_eq!(reloaded.group_wait_seconds, 60);
        assert_eq!(reloaded.group_interval_seconds, 120);
        assert_eq!(reloaded.repeat_interval_seconds, Some(1_800));
        assert_eq!(reloaded.max_group_size, 50);
        assert!(reloaded.is_persistent());
    }

    #[tokio::test]
    async fn recover_groups_backward_compat_with_legacy_format() {
        // The pre-Phase-2 server wrote a partial JSON format. This
        // test simulates finding one of those records in the state
        // store after an upgrade and verifies it still reloads
        // cleanly (as an ephemeral group with default timings).
        let state = MemoryStateStore::new();
        let legacy_json = serde_json::json!({
            "group_id": "legacy-1",
            "group_key": "legacy-key",
            "size": 2,
            "notify_at": (Utc::now() + chrono::Duration::seconds(60)).to_rfc3339(),
            "trace_context": {},
            "labels": {"cluster": "prod"},
            "events": [],
        });
        state
            .set(
                &StateKey::new("notifications", "tenant-1", KeyKind::Group, "legacy-key"),
                &legacy_json.to_string(),
                None,
            )
            .await
            .unwrap();

        let manager = GroupManager::new();
        let recovered = manager
            .recover_groups(&state, "notifications", "tenant-1", None)
            .await
            .unwrap();
        assert_eq!(recovered, 1);
        let reloaded = manager.get_group("legacy-key").unwrap();
        assert_eq!(reloaded.group_id, "legacy-1");
        // Legacy records restore as ephemeral (no repeat_interval).
        assert!(!reloaded.is_persistent());
        assert_eq!(reloaded.max_group_size, 100); // default applied
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
