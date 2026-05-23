use std::collections::HashMap;

use chrono::Utc;
use tracing::{error, info, warn};

use acteon_state::{KeyKind, StateKey};

use super::super::BackgroundProcessor;

impl BackgroundProcessor {
    /// Run the data retention reaper.
    ///
    /// Loads retention policies from the state store, then scans for completed
    /// chains and resolved events older than the configured TTLs and deletes them.
    /// This implementation is optimized to scan each kind only once.
    pub(crate) async fn run_retention_reaper(
        &mut self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Reload retention policies from the state store (hot-reload across instances).
        let entries = self
            .state
            .scan_keys_by_kind(KeyKind::Retention)
            .await
            .unwrap_or_default();

        let mut policies: HashMap<String, acteon_core::RetentionPolicy> = HashMap::new();
        for (key, value) in entries {
            // Skip index keys (format: idx:namespace:tenant).
            if key.contains(":retention:idx:") {
                continue;
            }
            if let Ok(policy) = serde_json::from_str::<acteon_core::RetentionPolicy>(&value)
                && policy.enabled
            {
                let key = format!("{}:{}", policy.namespace, policy.tenant);
                policies.insert(key, policy);
            }
        }
        self.retention_policies = policies;

        if self.retention_policies.is_empty() {
            return Ok(());
        }

        let mut total_deleted = 0u64;
        let mut total_skipped = 0u64;
        let mut total_errors = 0u64;

        // Check if any policy requires chain or event reaping (including
        // compliance-hold policies so the skip metric is tracked).
        let any_chain_reap = self
            .retention_policies
            .values()
            .any(|p| p.state_ttl_seconds.is_some() || p.compliance_hold);
        let any_event_reap = self
            .retention_policies
            .values()
            .any(|p| p.event_ttl_seconds.is_some() || p.compliance_hold);

        if any_chain_reap {
            match self.reap_chains_optimized().await {
                Ok((deleted, errors, skipped)) => {
                    total_deleted += deleted;
                    total_errors += errors;
                    total_skipped += skipped;
                }
                Err(e) => {
                    error!(error = %e, "retention reaper: failed to scan chains");
                    total_errors += 1;
                    self.metrics.increment_retention_errors();
                }
            }
        }

        if any_event_reap {
            match self.reap_events_optimized().await {
                Ok((deleted, errors, skipped)) => {
                    total_deleted += deleted;
                    total_errors += errors;
                    total_skipped += skipped;
                }
                Err(e) => {
                    error!(error = %e, "retention reaper: failed to scan events");
                    total_errors += 1;
                    self.metrics.increment_retention_errors();
                }
            }
        }

        if total_deleted > 0 || total_errors > 0 || total_skipped > 0 {
            info!(
                deleted = total_deleted,
                skipped_compliance = total_skipped,
                errors = total_errors,
                "retention reaper cycle complete"
            );
        }

        Ok(())
    }

    /// Optimized chain reaping: scan once and process all policies.
    async fn reap_chains_optimized(
        &self,
    ) -> Result<(u64, u64, u64), Box<dyn std::error::Error + Send + Sync>> {
        let now = Utc::now();
        let entries = self.state.scan_keys_by_kind(KeyKind::Chain).await?;
        let mut deleted = 0u64;
        let mut errors = 0u64;
        let mut skipped = 0u64;

        for (key, raw_value) in entries {
            // Key format: {namespace}:{tenant}:chain:{id}
            let parts: Vec<&str> = key.splitn(4, ':').collect();
            if parts.len() < 4 {
                continue;
            }

            let namespace = parts[0];
            let tenant = parts[1];
            let policy_key = format!("{namespace}:{tenant}");

            let Some(policy) = self.retention_policies.get(&policy_key) else {
                continue;
            };

            if policy.compliance_hold {
                skipped += 1;
                self.metrics.increment_retention_skipped_compliance();
                continue;
            }

            if policy.state_ttl_seconds.is_none() {
                continue;
            }

            let ttl_seconds = policy.state_ttl_seconds.unwrap();
            #[allow(clippy::cast_possible_wrap)]
            let cutoff = now - chrono::Duration::seconds(ttl_seconds as i64);

            let Ok(value) = self.decrypt_state_value(&raw_value) else {
                continue;
            };

            let Ok(chain_data) = serde_json::from_str::<serde_json::Value>(&value) else {
                continue;
            };

            // Only delete terminal chains.
            let status = chain_data
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if !matches!(status, "completed" | "failed" | "cancelled" | "timed_out") {
                continue;
            }

            // Check age via started_at or updated_at.
            let timestamp_str = chain_data
                .get("started_at")
                .or_else(|| chain_data.get("updated_at"))
                .and_then(|v| v.as_str());
            let Some(ts_str) = timestamp_str else {
                continue;
            };
            let Ok(ts) = chrono::DateTime::parse_from_rfc3339(ts_str) else {
                continue;
            };

            if ts.with_timezone(&Utc) < cutoff {
                let state_key = StateKey::new(namespace, tenant, KeyKind::Chain, parts[3]);
                match self.state.delete(&state_key).await {
                    Ok(_) => {
                        deleted += 1;
                        self.metrics.increment_retention_deleted_state();
                    }
                    Err(e) => {
                        warn!(
                            namespace = %namespace,
                            tenant = %tenant,
                            key = %key,
                            error = %e,
                            "retention reaper: error deleting chain"
                        );
                        errors += 1;
                        self.metrics.increment_retention_errors();
                    }
                }
            }
        }

        Ok((deleted, errors, skipped))
    }

    /// Optimized event reaping: scan once and process all policies.
    async fn reap_events_optimized(
        &self,
    ) -> Result<(u64, u64, u64), Box<dyn std::error::Error + Send + Sync>> {
        let now = Utc::now();
        let entries = self.state.scan_keys_by_kind(KeyKind::EventState).await?;
        let mut deleted = 0u64;
        let mut errors = 0u64;
        let mut skipped = 0u64;

        for (key, raw_value) in entries {
            // Key format: {namespace}:{tenant}:event_state:{id}
            let parts: Vec<&str> = key.splitn(4, ':').collect();
            if parts.len() < 4 {
                continue;
            }

            let namespace = parts[0];
            let tenant = parts[1];
            let policy_key = format!("{namespace}:{tenant}");

            let Some(policy) = self.retention_policies.get(&policy_key) else {
                continue;
            };

            if policy.compliance_hold {
                skipped += 1;
                self.metrics.increment_retention_skipped_compliance();
                continue;
            }

            if policy.event_ttl_seconds.is_none() {
                continue;
            }

            let ttl_seconds = policy.event_ttl_seconds.unwrap();
            #[allow(clippy::cast_possible_wrap)]
            let cutoff = now - chrono::Duration::seconds(ttl_seconds as i64);

            let Ok(value) = self.decrypt_state_value(&raw_value) else {
                continue;
            };

            let Ok(event_data) = serde_json::from_str::<serde_json::Value>(&value) else {
                continue;
            };

            // Only delete resolved events.
            let state = event_data
                .get("state")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if state != "resolved" {
                continue;
            }

            // Check age via updated_at.
            let timestamp_str = event_data.get("updated_at").and_then(|v| v.as_str());
            let Some(ts_str) = timestamp_str else {
                continue;
            };
            let Ok(ts) = chrono::DateTime::parse_from_rfc3339(ts_str) else {
                continue;
            };

            if ts.with_timezone(&Utc) < cutoff {
                let state_key = StateKey::new(namespace, tenant, KeyKind::EventState, parts[3]);
                match self.state.delete(&state_key).await {
                    Ok(_) => {
                        deleted += 1;
                        self.metrics.increment_retention_deleted_state();
                    }
                    Err(e) => {
                        warn!(
                            namespace = %namespace,
                            tenant = %tenant,
                            key = %key,
                            error = %e,
                            "retention reaper: error deleting event state"
                        );
                        errors += 1;
                        self.metrics.increment_retention_errors();
                    }
                }
            }
        }

        Ok((deleted, errors, skipped))
    }
}
