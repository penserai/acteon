use chrono::Utc;
use tracing::{debug, warn};

use super::super::{BackgroundProcessor, ChainAdvanceEvent};

impl BackgroundProcessor {
    /// Scan for pending chains and emit advance events.
    ///
    /// Uses the chain-ready index for efficient O(log N + M) lookups instead
    /// of scanning all pending chain keys.
    pub(crate) async fn advance_pending_chains(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let Some(ref tx) = self.chain_advance_tx else {
            return Ok(());
        };

        let now_ms = Utc::now().timestamp_millis();
        let ready_keys = self.state.get_ready_chains(now_ms).await?;

        if ready_keys.is_empty() {
            return Ok(());
        }

        debug!(count = ready_keys.len(), "checking ready chains");

        for key in ready_keys {
            // Parse namespace:tenant:pending_chains:chain_id from the canonical key.
            let parts: Vec<&str> = key.splitn(4, ':').collect();
            if parts.len() < 4 {
                warn!(key = %key, "invalid chain ready key format");
                continue;
            }

            let event = ChainAdvanceEvent {
                namespace: parts[0].to_string(),
                tenant: parts[1].to_string(),
                chain_id: parts[3].to_string(),
            };

            if tx.send(event).await.is_err() {
                warn!("chain advance event channel closed");
                return Ok(());
            }
        }

        Ok(())
    }
}
