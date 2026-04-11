use std::time::Duration;

use chrono::Utc;
use tracing::{debug, info, warn};

use acteon_state::{KeyKind, StateKey};

use crate::group_manager::persist_group;

use super::super::{BackgroundProcessor, GroupFlushEvent};

/// TTL for the per-flush CAS claim key. Long enough to survive a slow
/// notification dispatch, short enough that a crashed flush worker
/// on instance A releases the claim for instance B on the next tick.
const FLUSH_CLAIM_TTL: Duration = Duration::from_secs(60);

impl BackgroundProcessor {
    /// Flush all groups that are ready.
    ///
    /// In HA deployments, multiple gateway instances may all see the
    /// same group as "ready" when its `notify_at` arrives. To prevent
    /// double-fire (two instances dispatching the same notification
    /// at the same time), each flush attempt is gated by a per-flush
    /// CAS claim keyed by `(group_key, notify_at_rfc3339)`. The first
    /// instance to write the claim wins; all others skip the group.
    /// The `notify_at` component of the key guarantees that the next
    /// scheduled re-flush (with a new `notify_at`) gets a fresh
    /// claim, so persistent groups can still re-fire across
    /// subsequent intervals.
    pub(crate) async fn flush_ready_groups(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let ready_groups = self.group_manager.get_ready_groups();

        if ready_groups.is_empty() {
            return Ok(());
        }

        debug!(count = ready_groups.len(), "flushing ready groups");

        for group in ready_groups {
            let group_key = group.group_key.clone();

            // Acquire the per-flush CAS claim before any side effects.
            // The claim key rotates per `notify_at`, so persistent
            // re-fires across subsequent intervals each get a new lock.
            let claim_key = StateKey::new(
                group.namespace.as_str(),
                group.tenant.as_str(),
                KeyKind::Custom("group_flush_claim".to_string()),
                format!("{}:{}", group.group_key, group.notify_at.to_rfc3339()).as_str(),
            );
            let acquired = match self
                .state
                .check_and_set(&claim_key, "claimed", Some(FLUSH_CLAIM_TTL))
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    warn!(error = %e, "group flush claim CAS failed; skipping (fail-closed)");
                    continue;
                }
            };
            if !acquired {
                debug!(
                    group_key = %group_key,
                    "group flush claim already held by another instance; skipping"
                );
                continue;
            }

            // Flush the group (marks it as notified, advances timing)
            if let Some(flushed_group) = self.group_manager.flush_group(&group_key) {
                let flushed_at = Utc::now();

                info!(
                    group_id = %flushed_group.group_id,
                    group_key = %group_key,
                    event_count = flushed_group.size(),
                    persistent = flushed_group.is_persistent(),
                    idle_flushes = flushed_group.idle_flushes,
                    "group flushed"
                );

                // Persist the updated group to the state store so
                // that `last_notified_at`, `idle_flushes`, and the new
                // `notify_at` survive a gateway restart. Without this
                // the reloaded group would reset to `last_notified_at
                // = None` and fire immediately after restart.
                if let Err(e) = persist_group(
                    &flushed_group,
                    self.state.as_ref(),
                    self.payload_encryptor.as_deref(),
                )
                .await
                {
                    warn!(error = %e, "failed to persist flushed group state");
                }

                // Send flush event if channel is configured
                if let Some(ref tx) = self.group_flush_tx {
                    let event = GroupFlushEvent {
                        group: flushed_group.clone(),
                        flushed_at,
                    };
                    if tx.send(event).await.is_err() {
                        warn!("group flush event channel closed");
                    }
                }

                // Ephemeral groups (no repeat_interval) are deleted
                // after their single flush, matching the pre-Phase-2
                // behavior. Persistent groups are kept alive so they
                // can continue to re-batch new events and re-fire on
                // the repeat interval — UNLESS they have been idle
                // for `MAX_IDLE_FLUSHES` consecutive re-fires, in
                // which case we assume the underlying condition has
                // resolved and evict the group to prevent unbounded
                // memory growth from dynamic group keys.
                let should_delete = !flushed_group.is_persistent() || flushed_group.should_evict();
                if should_delete {
                    if flushed_group.is_persistent() {
                        info!(
                            group_id = %flushed_group.group_id,
                            group_key = %group_key,
                            idle_flushes = flushed_group.idle_flushes,
                            "evicting idle persistent group"
                        );
                    }
                    self.group_manager.remove_group(&group_key);
                    // Also remove from the state store so peer
                    // instances' periodic sync picks up the eviction.
                    let group_state_key = StateKey::new(
                        flushed_group.namespace.as_str(),
                        flushed_group.tenant.as_str(),
                        KeyKind::Group,
                        &group_key,
                    );
                    if let Err(e) = self.state.delete(&group_state_key).await {
                        warn!(error = %e, "failed to delete flushed group from state store");
                    }
                }
            }
        }

        Ok(())
    }
}
