use chrono::Utc;
use tracing::{debug, info, warn};

use super::super::{BackgroundProcessor, GroupFlushEvent};

impl BackgroundProcessor {
    /// Flush all groups that are ready.
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

            // Flush the group (marks it as notified, advances timing)
            if let Some(flushed_group) = self.group_manager.flush_group(&group_key) {
                let flushed_at = Utc::now();

                info!(
                    group_id = %flushed_group.group_id,
                    group_key = %group_key,
                    event_count = flushed_group.size(),
                    persistent = flushed_group.is_persistent(),
                    "group flushed"
                );

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
                // the repeat interval.
                if !flushed_group.is_persistent() {
                    self.group_manager.remove_group(&group_key);
                }
            }
        }

        Ok(())
    }
}
