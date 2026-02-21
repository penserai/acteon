use chrono::Utc;
use tracing::{debug, info, warn};

use acteon_state::{KeyKind, StateKey};

use super::super::{BackgroundProcessor, ScheduledActionDueEvent};

impl BackgroundProcessor {
    /// Process scheduled actions that are due for dispatch.
    ///
    /// Uses the timeout index for efficient O(log N + M) lookups of expired
    /// `PendingScheduled` keys, loads the corresponding `ScheduledAction` data,
    /// and emits dispatch events.
    ///
    /// Uses an atomic claim key (`check_and_set`) to prevent double-dispatch
    /// when multiple server instances poll concurrently.
    pub(crate) async fn process_scheduled_actions(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let Some(ref tx) = self.scheduled_action_tx else {
            return Ok(());
        };

        let now_ms = Utc::now().timestamp_millis();

        // Use the timeout index for efficient O(log N + M) queries instead of
        // scanning all PendingScheduled keys (which would be O(N)).
        let expired_keys = self.state.get_expired_timeouts(now_ms).await?;

        // Filter to only PendingScheduled keys (the timeout index is shared
        // with EventTimeout keys).
        let due_keys: Vec<String> = expired_keys
            .into_iter()
            .filter(|k| k.contains(":pending_scheduled:"))
            .collect();

        if due_keys.is_empty() {
            return Ok(());
        }

        let mut dispatched = 0u32;

        for key in due_keys {
            // Parse namespace:tenant:pending_scheduled:action_id
            let parts: Vec<&str> = key.splitn(4, ':').collect();
            if parts.len() < 4 {
                warn!(key = %key, "invalid pending scheduled key format");
                continue;
            }
            let namespace = parts[0];
            let tenant = parts[1];
            let action_id = parts[3];

            // Atomically claim this scheduled action to prevent double-dispatch.
            // If another instance already claimed it, `check_and_set` returns false.
            let claim_key = StateKey::new(
                namespace,
                tenant,
                KeyKind::ScheduledAction,
                format!("{action_id}:claim"),
            );
            let claimed = self
                .state
                .check_and_set(
                    &claim_key,
                    "claimed",
                    Some(std::time::Duration::from_secs(60)),
                )
                .await?;
            if !claimed {
                debug!(action_id = %action_id, "scheduled action already claimed by another instance");
                continue;
            }

            // Load the scheduled action data.
            let sched_key = StateKey::new(namespace, tenant, KeyKind::ScheduledAction, action_id);
            let Some(raw_str) = self.state.get(&sched_key).await? else {
                // Already processed, clean up pending key.
                let pending_key =
                    StateKey::new(namespace, tenant, KeyKind::PendingScheduled, action_id);
                self.state.delete(&pending_key).await?;
                self.state.remove_timeout_index(&pending_key).await?;
                continue;
            };

            let data_str = match self.decrypt_state_value(&raw_str) {
                Ok(v) => v,
                Err(e) => {
                    warn!(action_id = %action_id, error = %e, "failed to decrypt scheduled action data");
                    continue;
                }
            };

            let Ok(data) = serde_json::from_str::<serde_json::Value>(&data_str) else {
                warn!(action_id = %action_id, "failed to parse scheduled action data");
                continue;
            };

            let Some(action_val) = data.get("action") else {
                warn!(action_id = %action_id, "scheduled action missing action field");
                continue;
            };

            let Ok(action) = serde_json::from_value::<acteon_core::Action>(action_val.clone())
            else {
                warn!(action_id = %action_id, "failed to deserialize scheduled action");
                continue;
            };

            // Clean up the pending/index keys so the background processor
            // won't re-poll this action. The action data key is intentionally
            // kept alive until the consumer confirms successful dispatch
            // (at-least-once semantics). If the server crashes between here
            // and consumer cleanup, the claim key expires (60s TTL) and a
            // subsequent poll will re-deliver the action.
            let pending_key =
                StateKey::new(namespace, tenant, KeyKind::PendingScheduled, action_id);
            self.state.delete(&pending_key).await?;
            self.state.remove_timeout_index(&pending_key).await?;

            info!(
                action_id = %action_id,
                namespace = %namespace,
                tenant = %tenant,
                "dispatching scheduled action"
            );

            let event = ScheduledActionDueEvent {
                namespace: namespace.to_string(),
                tenant: tenant.to_string(),
                action_id: action_id.to_string(),
                action,
            };

            if tx.send(event).await.is_err() {
                warn!("scheduled action event channel closed");
                return Ok(());
            }
            dispatched += 1;
        }

        if dispatched > 0 {
            debug!(count = dispatched, "dispatched due scheduled actions");
        }

        Ok(())
    }
}
