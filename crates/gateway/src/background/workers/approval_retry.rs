use chrono::Utc;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use acteon_state::KeyKind;

use crate::gateway::ApprovalRecord;

use super::super::{ApprovalRetryEvent, BackgroundProcessor};

impl BackgroundProcessor {
    /// Run periodic cleanup tasks, including approval notification retry sweep.
    pub(crate) async fn run_cleanup(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Clean up resolved/notified groups that are no longer needed
        let groups = self.group_manager.list_pending_groups();
        debug!(pending_groups = groups.len(), "cleanup: checking groups");

        // Sweep for pending approvals that need notification retry.
        if self.config.enable_approval_retry
            && let Some(ref tx) = self.approval_retry_tx
        {
            self.sweep_approval_retries(tx).await;
        }

        Ok(())
    }

    /// Scan for pending approvals with `notification_sent == false` and emit retry events.
    async fn sweep_approval_retries(&self, tx: &mpsc::Sender<ApprovalRetryEvent>) {
        let entries = match self.state.scan_keys_by_kind(KeyKind::Approval).await {
            Ok(entries) => entries,
            Err(e) => {
                warn!(error = %e, "failed to scan approval keys for retry sweep");
                return;
            }
        };

        let now = Utc::now();
        let mut retry_count = 0u32;

        for (key, raw_value) in entries {
            // Skip claim keys (format: namespace:tenant:approval:id:claim)
            if key.ends_with(":claim") {
                continue;
            }

            let Ok(value) = self.decrypt_state_value(&raw_value) else {
                continue;
            };

            let record: ApprovalRecord = match serde_json::from_str(&value) {
                Ok(r) => r,
                Err(_) => continue,
            };

            // Only retry pending, unsent, non-expired approvals
            if record.status != "pending" || record.notification_sent || record.expires_at <= now {
                continue;
            }

            // Parse namespace and tenant from key (format: namespace:tenant:approval:id)
            let parts: Vec<&str> = key.splitn(4, ':').collect();
            if parts.len() < 4 {
                continue;
            }
            let namespace = parts[0].to_string();
            let tenant = parts[1].to_string();

            let event = ApprovalRetryEvent {
                namespace,
                tenant,
                approval_id: record.token.clone(),
                record,
            };

            if tx.send(event).await.is_err() {
                warn!("approval retry event channel closed");
                return;
            }
            retry_count += 1;
        }

        if retry_count > 0 {
            debug!(count = retry_count, "emitted approval retry events");
        }
    }
}
