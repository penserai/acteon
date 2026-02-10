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
    pub async fn recover_groups(
        &self,
        state: &dyn StateStore,
        namespace: &str,
        tenant: &str,
    ) -> Result<usize, GatewayError> {
        let entries = state
            .scan_keys(namespace, tenant, KeyKind::Group, None)
            .await?;

        let mut recovered = 0;
        let mut groups = self.groups.write();

        for (key, value) in entries {
            // Parse the group metadata from stored JSON
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

                // Only recover if not already in memory
                if !groups.contains_key(&group_key) {
                    let mut group = EventGroup::new(&group_id, &group_key, notify_at);
                    group.trace_context = trace_context;
                    groups.insert(group_key.clone(), group);
                    recovered += 1;
                    tracing::info!(
                        group_id = %group_id,
                        group_key = %group_key,
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
    /// Returns a tuple of (group ID, current group size, notify at time).
    pub async fn add_to_group(
        &self,
        action: &Action,
        group_by: &[String],
        group_wait_seconds: u64,
        state: &dyn StateStore,
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

        // Check if group exists or create new one
        let (group_id, group_size, notify_at) = {
            let mut groups = self.groups.write();

            if let Some(group) = groups.get_mut(&group_key) {
                // Add to existing group
                group.add_event(grouped_event);
                (group.group_id.clone(), group.size(), group.notify_at)
            } else {
                // Create new group
                let group_id = Uuid::new_v4().to_string();
                #[allow(clippy::cast_possible_wrap)]
                let notify_at = Utc::now() + chrono::Duration::seconds(group_wait_seconds as i64);
                let mut group = EventGroup::new(&group_id, &group_key, notify_at);

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
                groups.insert(group_key.clone(), group);

                (group_id, size, notify)
            }
        };

        // Persist group state
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
            "trace_context": &action.trace_context,
        });
        state
            .set(&state_key, &group_value.to_string(), None)
            .await?;

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
    #[must_use]
    pub fn get_ready_groups(&self) -> Vec<EventGroup> {
        let now = Utc::now();
        self.groups
            .read()
            .values()
            .filter(|g| matches!(g.state, GroupState::Pending) && g.notify_at <= now)
            .cloned()
            .collect()
    }

    /// Flush a group (mark as notified and return events).
    ///
    /// Returns the group if it was pending, None if already processed.
    pub fn flush_group(&self, group_key: &str) -> Option<EventGroup> {
        let mut groups = self.groups.write();
        if let Some(group) = groups.get_mut(group_key)
            && matches!(group.state, GroupState::Pending)
        {
            group.state = GroupState::Notified;
            group.updated_at = Utc::now();
            return Some(group.clone());
        }
        None
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
            .add_to_group(&action, &["metadata.cluster".to_string()], 60, &state)
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
            .add_to_group(&action1, &["metadata.cluster".to_string()], 60, &state)
            .await
            .unwrap();

        let (group_id2, _, size2, _) = manager
            .add_to_group(&action2, &["metadata.cluster".to_string()], 60, &state)
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

        // Second flush should return None
        let flushed2 = manager.flush_group("key-1");
        assert!(flushed2.is_none());
    }
}
