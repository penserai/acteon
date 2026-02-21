//! Tenant quota enforcement methods on [`Gateway`].
//!
//! Extracted from the main gateway module to keep `gateway.rs` focused on
//! dispatch orchestration.

use chrono::Utc;
use tracing::{info, instrument, warn};

use acteon_core::{Action, ActionOutcome};

use crate::error::GatewayError;
use crate::gateway::{CachedPolicy, Gateway};

impl Gateway {
    /// Check whether the action's tenant has exceeded their quota.
    ///
    /// Uses an atomic `increment()` to avoid read-then-write races between
    /// concurrent actions for the same tenant.  The counter is always
    /// incremented first; if the new value exceeds the limit the configured
    /// [`OverageBehavior`](acteon_core::OverageBehavior) determines the outcome.
    ///
    /// Returns `None` when the action is within quota or no policy exists.
    /// Returns `Some(ActionOutcome::QuotaExceeded { .. })` when the action
    /// should be blocked or degraded.
    #[instrument(name = "gateway.check_quota", skip_all)]
    pub(crate) async fn check_quota(
        &self,
        action: &Action,
    ) -> Result<Option<ActionOutcome>, GatewayError> {
        // Skip quota for internal re-dispatches (scheduled, recurring, groups)
        // to avoid double-counting. The action was already counted when it
        // first entered the gateway.
        if action
            .payload
            .get("_scheduled_dispatch")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
            || action
                .payload
                .get("_recurring_dispatch")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            || action
                .payload
                .get("_group_dispatch")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
        {
            return Ok(None);
        }

        let policy_key = format!("{}:{}", action.namespace, action.tenant);
        let now = Utc::now();

        // 1. Check in-memory cache with a 60-second TTL to ensure we eventually
        //    see updates made on other instances.
        let cached = {
            let map = self.quota_policies.read();
            map.get(&policy_key).cloned()
        };

        const CACHE_TTL_SECS: i64 = 60;

        let policy = if let Some(c) = cached
            && (now - c.cached_at).num_seconds() < CACHE_TTL_SECS
        {
            c.policy
        } else {
            // Cold path: fetch from state store. We fail-open if the store
            // is down to protect system availability.
            let found = match self
                .load_quota_from_state_store(&action.namespace, &action.tenant)
                .await
            {
                Ok(f) => f,
                Err(e) => {
                    warn!(error = %e, "quota policy lookup failed (fail-open)");
                    return Ok(None);
                }
            };

            match found {
                Some(p) => {
                    let cached = CachedPolicy {
                        policy: p.clone(),
                        cached_at: now,
                    };
                    self.quota_policies
                        .write()
                        .insert(policy_key.clone(), cached);
                    p
                }
                None => return Ok(None),
            }
        };

        if !policy.enabled {
            return Ok(None);
        }

        let counter_id =
            acteon_core::quota_counter_key(&action.namespace, &action.tenant, &policy.window, &now);
        let counter_key = acteon_state::StateKey::new(
            action.namespace.as_str(),
            action.tenant.as_str(),
            acteon_state::KeyKind::QuotaUsage,
            &counter_id,
        );

        let window_ttl = Some(std::time::Duration::from_secs(
            policy.window.duration_seconds(),
        ));

        // 2. Increment usage counter. Fail-open on state store errors.
        let new_count = match self.state.increment(&counter_key, 1, window_ttl).await {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "quota increment failed (fail-open)");
                return Ok(None);
            }
        };

        #[allow(clippy::cast_sign_loss)]
        let used = new_count as u64;

        if used <= policy.max_actions {
            return Ok(None);
        }

        // Quota exceeded — apply behavior.
        self.apply_overage_behavior(action, &policy, used, &counter_key, window_ttl)
            .await
    }

    /// Apply the configured overage behavior when a tenant exceeds their quota.
    ///
    /// Separated from [`check_quota`](Self::check_quota) to keep each method
    /// under the clippy line-count limit.
    async fn apply_overage_behavior(
        &self,
        action: &Action,
        policy: &acteon_core::QuotaPolicy,
        used: u64,
        counter_key: &acteon_state::StateKey,
        window_ttl: Option<std::time::Duration>,
    ) -> Result<Option<ActionOutcome>, GatewayError> {
        match &policy.overage_behavior {
            acteon_core::OverageBehavior::Block => {
                self.metrics.increment_quota_exceeded();
                // Roll back the increment so the blocked request doesn't
                // consume a slot.
                let _ = self.state.increment(counter_key, -1, window_ttl).await;
                info!(
                    tenant = %action.tenant,
                    limit = policy.max_actions,
                    used,
                    "quota exceeded — blocking action"
                );
                Ok(Some(ActionOutcome::QuotaExceeded {
                    tenant: action.tenant.to_string(),
                    limit: policy.max_actions,
                    used,
                    overage_behavior: "block".into(),
                }))
            }
            acteon_core::OverageBehavior::Warn => {
                self.metrics.increment_quota_warned();
                warn!(
                    tenant = %action.tenant,
                    limit = policy.max_actions,
                    used,
                    "quota exceeded — warning, allowing action"
                );
                Ok(None)
            }
            acteon_core::OverageBehavior::Degrade { fallback_provider } => {
                self.metrics.increment_quota_degraded();
                info!(
                    tenant = %action.tenant,
                    fallback = %fallback_provider,
                    "quota exceeded — degrading to fallback provider"
                );
                Ok(Some(ActionOutcome::QuotaExceeded {
                    tenant: action.tenant.to_string(),
                    limit: policy.max_actions,
                    used,
                    overage_behavior: format!("degrade:{fallback_provider}"),
                }))
            }
            acteon_core::OverageBehavior::Notify { target } => {
                self.metrics.increment_quota_notified();
                warn!(
                    tenant = %action.tenant,
                    target = %target,
                    "quota exceeded — notifying admin, allowing action"
                );
                Ok(None)
            }
        }
    }

    /// Try to load a quota policy for `namespace:tenant` from the state store.
    ///
    /// This is the cold-path fallback used by [`check_quota`](Self::check_quota)
    /// when no in-memory policy is found, enabling cross-instance visibility
    /// without requiring a restart.
    ///
    /// Uses a two-step O(1) lookup via the `idx:{namespace}:{tenant}` index
    /// key written by the API layer, avoiding a full `scan_keys_by_kind`.
    async fn load_quota_from_state_store(
        &self,
        namespace: &str,
        tenant: &str,
    ) -> Result<Option<acteon_core::QuotaPolicy>, GatewayError> {
        // Step 1: look up the index key to get the policy ID.
        let idx_suffix = format!("idx:{namespace}:{tenant}");
        let idx_key = acteon_state::StateKey::new(
            "_system",
            "_quotas",
            acteon_state::KeyKind::Quota,
            &idx_suffix,
        );
        let Some(policy_id) = self.state.get(&idx_key).await? else {
            return Ok(None);
        };

        // Step 2: look up the policy by ID.
        let policy_key = acteon_state::StateKey::new(
            "_system",
            "_quotas",
            acteon_state::KeyKind::Quota,
            &policy_id,
        );
        match self.state.get(&policy_key).await? {
            Some(data) => {
                let policy = serde_json::from_str::<acteon_core::QuotaPolicy>(&data)
                    .map_err(|e| GatewayError::Configuration(e.to_string()))?;
                Ok(Some(policy))
            }
            None => Ok(None),
        }
    }
}
