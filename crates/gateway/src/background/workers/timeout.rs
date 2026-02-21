use chrono::Utc;
use tracing::{debug, info, warn};

use acteon_state::{KeyKind, StateKey};

use super::super::{BackgroundProcessor, TimeoutEvent};

impl BackgroundProcessor {
    /// Process state machine timeouts.
    ///
    /// Uses an indexed approach to efficiently find expired timeouts in O(log N + M)
    /// where M is the number of expired entries, instead of scanning all timeout keys.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn process_timeouts(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let now = Utc::now();
        let now_ms = now.timestamp_millis();

        // Get only the expired timeout keys using the efficient index query.
        let expired_keys = self.state.get_expired_timeouts(now_ms).await?;

        if expired_keys.is_empty() {
            return Ok(());
        }

        debug!(count = expired_keys.len(), "processing expired timeouts");

        for canonical_key in expired_keys {
            // Parse namespace and tenant from the key (format: namespace:tenant:kind:id)
            let key_parts: Vec<&str> = canonical_key.splitn(4, ':').collect();
            let (namespace, tenant, fingerprint) = if key_parts.len() >= 4 {
                (
                    key_parts[0].to_string(),
                    key_parts[1].to_string(),
                    key_parts[3].to_string(),
                )
            } else {
                warn!(key = %canonical_key, "invalid timeout key format");
                continue;
            };

            // Fetch the timeout data from the state store
            let timeout_key = StateKey::new(
                namespace.as_str(),
                tenant.as_str(),
                KeyKind::EventTimeout,
                &fingerprint,
            );

            let Some(value) = self.state.get(&timeout_key).await? else {
                // Timeout was already processed or deleted, remove from index
                self.state.remove_timeout_index(&timeout_key).await?;
                continue;
            };

            // Decrypt and parse the timeout entry.
            let decrypted_value = match self.decrypt_state_value(&value) {
                Ok(v) => v,
                Err(e) => {
                    warn!(key = %canonical_key, error = %e, "failed to decrypt timeout data");
                    continue;
                }
            };
            let Ok(timeout_data) = serde_json::from_str::<serde_json::Value>(&decrypted_value)
            else {
                warn!(key = %canonical_key, "failed to parse timeout data");
                continue;
            };

            // fingerprint is already parsed from the key above
            let state_machine_name = timeout_data
                .get("state_machine")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let current_state = timeout_data
                .get("current_state")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let transition_to = timeout_data
                .get("transition_to")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let trace_context: std::collections::HashMap<String, String> = timeout_data
                .get("trace_context")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();

            info!(
                fingerprint = %fingerprint,
                namespace = %namespace,
                tenant = %tenant,
                state_machine = %state_machine_name,
                from_state = %current_state,
                to_state = %transition_to,
                "processing expired timeout"
            );

            // Update the event state
            let state_key = StateKey::new(
                namespace.as_str(),
                tenant.as_str(),
                KeyKind::EventState,
                &fingerprint,
            );

            let new_state_value = serde_json::json!({
                "state": &transition_to,
                "fingerprint": &fingerprint,
                "updated_at": now.to_rfc3339(),
                "transitioned_by": "timeout",
            });

            let encrypted_state = match self.payload_encryptor {
                Some(ref enc) => enc
                    .encrypt_str(&new_state_value.to_string())
                    .unwrap_or_else(|_| new_state_value.to_string()),
                None => new_state_value.to_string(),
            };

            self.state.set(&state_key, &encrypted_state, None).await?;

            // Delete the processed timeout entry and remove from index
            self.state.delete(&timeout_key).await?;
            self.state.remove_timeout_index(&timeout_key).await?;

            // Send timeout event if channel is configured
            if let Some(ref tx) = self.timeout_tx {
                let event = TimeoutEvent {
                    fingerprint,
                    state_machine: state_machine_name,
                    previous_state: current_state,
                    new_state: transition_to,
                    fired_at: now,
                    trace_context,
                };
                if tx.send(event).await.is_err() {
                    warn!("timeout event channel closed");
                }
            }
        }

        Ok(())
    }
}
