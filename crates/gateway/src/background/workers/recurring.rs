use chrono::Utc;
use tracing::{debug, info, warn};

use acteon_state::{KeyKind, StateKey};

use super::super::{BackgroundProcessor, RecurringActionDueEvent};

impl BackgroundProcessor {
    /// Process recurring actions that are due for dispatch.
    ///
    /// Uses the timeout index to efficiently find expired `PendingRecurring`
    /// keys, loads the corresponding `RecurringAction` data, validates it is
    /// still active, and emits dispatch events.
    ///
    /// Uses an atomic claim key (`check_and_set`) to prevent double-dispatch
    /// when multiple server instances poll concurrently.
    ///
    /// After dispatch, computes the next occurrence and re-indexes the action.
    /// If the action has expired (`ends_at` in the past) or is disabled, it is
    /// removed from the pending index without dispatching.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn process_recurring_actions(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let Some(ref tx) = self.recurring_action_tx else {
            return Ok(());
        };

        let now = Utc::now();
        let now_ms = now.timestamp_millis();

        let expired_keys = self.state.get_expired_timeouts(now_ms).await?;

        let due_keys: Vec<String> = expired_keys
            .into_iter()
            .filter(|k| k.contains(":pending_recurring:"))
            .collect();

        if due_keys.is_empty() {
            return Ok(());
        }

        let mut dispatched = 0u32;
        let mut skipped = 0u32;

        for key in due_keys {
            // Parse namespace:tenant:pending_recurring:recurring_id
            let parts: Vec<&str> = key.splitn(4, ':').collect();
            if parts.len() < 4 {
                warn!(key = %key, "invalid pending recurring key format");
                continue;
            }
            let namespace = parts[0];
            let tenant = parts[1];
            let recurring_id = parts[3];

            // Atomically claim this recurring action to prevent double-dispatch.
            let claim_key = StateKey::new(
                namespace,
                tenant,
                KeyKind::RecurringAction,
                format!("{recurring_id}:claim"),
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
                debug!(recurring_id = %recurring_id, "recurring action already claimed by another instance");
                continue;
            }

            // Load the recurring action data.
            let rec_key = StateKey::new(namespace, tenant, KeyKind::RecurringAction, recurring_id);
            let Some(raw_str) = self.state.get(&rec_key).await? else {
                // Already deleted, clean up pending key.
                let pending_key =
                    StateKey::new(namespace, tenant, KeyKind::PendingRecurring, recurring_id);
                self.state.delete(&pending_key).await?;
                self.state.remove_timeout_index(&pending_key).await?;
                continue;
            };

            let data_str = match self.decrypt_state_value(&raw_str) {
                Ok(v) => v,
                Err(e) => {
                    warn!(recurring_id = %recurring_id, error = %e, "failed to decrypt recurring action data");
                    continue;
                }
            };

            let Ok(recurring) = serde_json::from_str::<acteon_core::RecurringAction>(&data_str)
            else {
                warn!(recurring_id = %recurring_id, "failed to deserialize recurring action");
                continue;
            };

            // Validate the action is still eligible for dispatch.
            let should_skip =
                !recurring.enabled || recurring.ends_at.is_some_and(|ends| ends <= now);

            if should_skip {
                debug!(
                    recurring_id = %recurring_id,
                    enabled = recurring.enabled,
                    "skipping ineligible recurring action"
                );
                // Remove from pending index so it won't be re-polled.
                let pending_key =
                    StateKey::new(namespace, tenant, KeyKind::PendingRecurring, recurring_id);
                self.state.delete(&pending_key).await?;
                self.state.remove_timeout_index(&pending_key).await?;
                skipped += 1;
                continue;
            }

            // Idempotency check: if last_executed_at is very close to now,
            // skip to avoid double-dispatch (addresses security review R1).
            if let Some(last) = recurring.last_executed_at {
                let gap = (now - last).num_seconds();
                if gap < 5 {
                    debug!(
                        recurring_id = %recurring_id,
                        last_executed_secs_ago = gap,
                        "skipping recently-executed recurring action"
                    );
                    skipped += 1;
                    continue;
                }
            }

            info!(
                recurring_id = %recurring_id,
                namespace = %namespace,
                tenant = %tenant,
                cron_expr = %recurring.cron_expr,
                "dispatching recurring action"
            );

            let event = RecurringActionDueEvent {
                namespace: namespace.to_string(),
                tenant: tenant.to_string(),
                recurring_id: recurring_id.to_string(),
                recurring_action: recurring,
            };

            if tx.send(event).await.is_err() {
                warn!("recurring action event channel closed");
                return Ok(());
            }
            dispatched += 1;
        }

        if dispatched > 0 || skipped > 0 {
            debug!(dispatched, skipped, "processed recurring actions");
        }

        Ok(())
    }
}
