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

            // Flush the group (marks it as notified)
            if let Some(flushed_group) = self.group_manager.flush_group(&group_key) {
                let flushed_at = Utc::now();

                // Remove from pending groups index
                // Note: We need namespace/tenant from the group labels or stored metadata
                // For now, we'll just clean up the in-memory state

                info!(
                    group_id = %flushed_group.group_id,
                    group_key = %group_key,
                    event_count = flushed_group.size(),
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

                // Remove the group from memory after processing
                self.group_manager.remove_group(&group_key);
            }
        }

        Ok(())
    }
}
