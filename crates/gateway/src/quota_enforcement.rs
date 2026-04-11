//! Tenant quota enforcement methods on [`Gateway`].
//!
//! Extracted from the main gateway module to keep `gateway.rs` focused on
//! dispatch orchestration.

use chrono::Utc;
use tracing::{info, instrument, warn};

use acteon_core::{Action, ActionOutcome};

use crate::error::GatewayError;
use crate::gateway::{CachedPolicy, Gateway};

/// Per-policy state tracked during a single `check_quota` call so
/// that the winner can be picked after every counter has been
/// incremented atomically.
struct Incremented {
    policy: acteon_core::QuotaPolicy,
    counter_key: acteon_state::StateKey,
    window_ttl: Option<std::time::Duration>,
    used: u64,
}

impl Gateway {
    /// Check whether the action's tenant has exceeded any applicable quota.
    ///
    /// Since Phase 3, a `(namespace, tenant)` pair may hold several
    /// quota policies — one generic catch-all plus any number of
    /// provider-scoped caps. All policies whose scope matches the
    /// outgoing provider are evaluated; each maintains its own
    /// counter so a burst on one provider does not consume another
    /// provider's budget. If **any** applicable policy blocks the
    /// action, the block wins and every counter that was incremented
    /// during this call is rolled back so the blocked request does
    /// not consume a slot. Non-block outcomes (warn/degrade/notify)
    /// leave counters advanced but still surface to the caller —
    /// degrade beats warn/notify as the strictest.
    ///
    /// Returns `None` when the action is within quota or no policy
    /// applies. Returns `Some(ActionOutcome::QuotaExceeded { .. })`
    /// when the action should be blocked or degraded.
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

        let bucket_key = format!("{}:{}", action.namespace, action.tenant);
        let now = Utc::now();

        // 1. Check in-memory cache with a 60-second TTL to ensure we eventually
        //    see updates made on other instances.
        let cached = {
            let map = self.quota_policies.read();
            map.get(&bucket_key).cloned()
        };

        const CACHE_TTL_SECS: i64 = 60;

        let bucket_policies: Vec<acteon_core::QuotaPolicy> = if let Some(c) = cached
            && (now - c.cached_at).num_seconds() < CACHE_TTL_SECS
        {
            c.policies
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

            if found.is_empty() {
                // Empty cache entry — still mark as warmed so we don't
                // hammer the state store for every dispatch.
                self.quota_policies.write().insert(
                    bucket_key.clone(),
                    CachedPolicy {
                        policies: Vec::new(),
                        cached_at: now,
                    },
                );
                return Ok(None);
            }

            let cached = CachedPolicy {
                policies: found.clone(),
                cached_at: now,
            };
            self.quota_policies
                .write()
                .insert(bucket_key.clone(), cached);
            found
        };

        // Filter to policies that actually apply to this dispatch.
        let applicable: Vec<acteon_core::QuotaPolicy> = bucket_policies
            .into_iter()
            .filter(|p| p.enabled && p.applies_to_provider(&action.provider))
            .collect();

        if applicable.is_empty() {
            return Ok(None);
        }

        self.enforce_quota_policies(action, applicable, &now).await
    }

    /// Evaluate every applicable quota policy for a dispatch and
    /// return the strictest outcome.
    ///
    /// Each policy increments its own counter first (atomic to avoid
    /// races). If any block, every counter touched in this call is
    /// rolled back. Otherwise the strictest non-block outcome wins,
    /// with counters left advanced so warn/degrade/notify still
    /// reflect true usage.
    async fn enforce_quota_policies(
        &self,
        action: &Action,
        policies: Vec<acteon_core::QuotaPolicy>,
        now: &chrono::DateTime<Utc>,
    ) -> Result<Option<ActionOutcome>, GatewayError> {
        // Fail-open: if any counter write fails the helper rolls
        // back and returns `Err(())`; fall through to `Ok(None)`
        // so we do not penalize dispatches during state-store
        // outages.
        let Ok(incremented) = self
            .increment_all_quota_counters(action, policies, now)
            .await
        else {
            return Ok(None);
        };

        let winner_idx = Self::pick_winning_quota(&incremented);
        let Some((idx, is_block)) = winner_idx else {
            return Ok(None);
        };

        if is_block {
            // Roll back every counter this call incremented so the
            // blocked action does not consume any tenant budget.
            for other in &incremented {
                let _ = self
                    .state
                    .increment(&other.counter_key, -1, other.window_ttl)
                    .await;
            }
        }

        let inc = &incremented[idx];
        Ok(self.apply_overage_behavior(action, &inc.policy, inc.used))
    }

    /// Increment the counter for every applicable policy, returning
    /// the per-policy state. On any state-store failure this rolls
    /// back everything it has already incremented and returns `Err`
    /// so the caller can fail-open.
    async fn increment_all_quota_counters(
        &self,
        action: &Action,
        policies: Vec<acteon_core::QuotaPolicy>,
        now: &chrono::DateTime<Utc>,
    ) -> Result<Vec<Incremented>, ()> {
        let mut incremented: Vec<Incremented> = Vec::with_capacity(policies.len());
        for policy in policies {
            let counter_id = acteon_core::quota_counter_key(
                &action.namespace,
                &action.tenant,
                policy.provider.as_deref(),
                &policy.window,
                now,
            );
            let counter_key = acteon_state::StateKey::new(
                action.namespace.as_str(),
                action.tenant.as_str(),
                acteon_state::KeyKind::QuotaUsage,
                &counter_id,
            );
            let window_ttl = Some(std::time::Duration::from_secs(
                policy.window.duration_seconds(),
            ));

            let new_count = match self.state.increment(&counter_key, 1, window_ttl).await {
                Ok(c) => c,
                Err(e) => {
                    warn!(error = %e, "quota increment failed (fail-open)");
                    for inc in &incremented {
                        let _ = self
                            .state
                            .increment(&inc.counter_key, -1, inc.window_ttl)
                            .await;
                    }
                    return Err(());
                }
            };
            #[allow(clippy::cast_sign_loss)]
            let used = new_count as u64;
            incremented.push(Incremented {
                policy,
                counter_key,
                window_ttl,
                used,
            });
        }
        Ok(incremented)
    }

    /// Pick the index of the strictest exceeded policy, returning a
    /// tuple of `(index, is_block)`. Precedence: Block > Degrade >
    /// Warn > Notify. When no policy is exceeded, returns `None`.
    fn pick_winning_quota(incremented: &[Incremented]) -> Option<(usize, bool)> {
        let mut winning_block: Option<usize> = None;
        let mut winning_degrade: Option<usize> = None;
        let mut winning_warn: Option<usize> = None;
        let mut winning_notify: Option<usize> = None;
        for (i, inc) in incremented.iter().enumerate() {
            if inc.used <= inc.policy.max_actions {
                continue;
            }
            match &inc.policy.overage_behavior {
                acteon_core::OverageBehavior::Block if winning_block.is_none() => {
                    winning_block = Some(i);
                }
                acteon_core::OverageBehavior::Degrade { .. } if winning_degrade.is_none() => {
                    winning_degrade = Some(i);
                }
                acteon_core::OverageBehavior::Warn if winning_warn.is_none() => {
                    winning_warn = Some(i);
                }
                acteon_core::OverageBehavior::Notify { .. } if winning_notify.is_none() => {
                    winning_notify = Some(i);
                }
                _ => {}
            }
        }
        if let Some(i) = winning_block {
            Some((i, true))
        } else {
            winning_degrade
                .or(winning_warn)
                .or(winning_notify)
                .map(|i| (i, false))
        }
    }

    /// Apply the configured overage behavior when a tenant exceeds their quota.
    ///
    /// Separated from [`check_quota`](Self::check_quota) to keep each method
    /// under the clippy line-count limit. Counter rollback for blocked
    /// dispatches is handled by
    /// [`enforce_quota_policies`](Self::enforce_quota_policies) across
    /// every applicable policy, so this helper only records metrics
    /// and shapes the outcome.
    fn apply_overage_behavior(
        &self,
        action: &Action,
        policy: &acteon_core::QuotaPolicy,
        used: u64,
    ) -> Option<ActionOutcome> {
        match &policy.overage_behavior {
            acteon_core::OverageBehavior::Block => {
                self.metrics.increment_quota_exceeded();
                info!(
                    tenant = %action.tenant,
                    limit = policy.max_actions,
                    used,
                    "quota exceeded — blocking action"
                );
                Some(ActionOutcome::QuotaExceeded {
                    tenant: action.tenant.to_string(),
                    limit: policy.max_actions,
                    used,
                    overage_behavior: "block".into(),
                })
            }
            acteon_core::OverageBehavior::Warn => {
                self.metrics.increment_quota_warned();
                warn!(
                    tenant = %action.tenant,
                    limit = policy.max_actions,
                    used,
                    "quota exceeded — warning, allowing action"
                );
                None
            }
            acteon_core::OverageBehavior::Degrade { fallback_provider } => {
                self.metrics.increment_quota_degraded();
                info!(
                    tenant = %action.tenant,
                    fallback = %fallback_provider,
                    "quota exceeded — degrading to fallback provider"
                );
                Some(ActionOutcome::QuotaExceeded {
                    tenant: action.tenant.to_string(),
                    limit: policy.max_actions,
                    used,
                    overage_behavior: format!("degrade:{fallback_provider}"),
                })
            }
            acteon_core::OverageBehavior::Notify { target } => {
                self.metrics.increment_quota_notified();
                warn!(
                    tenant = %action.tenant,
                    target = %target,
                    "quota exceeded — notifying admin, allowing action"
                );
                None
            }
        }
    }

    /// Load every quota policy registered for `namespace:tenant`
    /// from the state store.
    ///
    /// Cold-path fallback used by [`check_quota`](Self::check_quota)
    /// when no in-memory policies are found, enabling cross-instance
    /// visibility without requiring a restart.
    ///
    /// The index key `idx:{namespace}:{tenant}` stores a JSON array
    /// of policy IDs so a single get + N gets loads the whole bucket
    /// without scanning the store. For backward compatibility with
    /// pre-Phase-3 records, a bare (non-JSON) UUID is also accepted
    /// and treated as a single-element array.
    async fn load_quota_from_state_store(
        &self,
        namespace: &str,
        tenant: &str,
    ) -> Result<Vec<acteon_core::QuotaPolicy>, GatewayError> {
        let idx_suffix = format!("idx:{namespace}:{tenant}");
        let idx_key = acteon_state::StateKey::new(
            "_system",
            "_quotas",
            acteon_state::KeyKind::Quota,
            &idx_suffix,
        );
        let Some(raw) = self.state.get(&idx_key).await? else {
            return Ok(Vec::new());
        };

        let policy_ids: Vec<String> = match serde_json::from_str::<Vec<String>>(&raw) {
            Ok(ids) => ids,
            Err(_) => vec![raw], // legacy: bare policy ID
        };

        let mut policies = Vec::with_capacity(policy_ids.len());
        for id in policy_ids {
            let policy_key = acteon_state::StateKey::new(
                "_system",
                "_quotas",
                acteon_state::KeyKind::Quota,
                &id,
            );
            if let Some(data) = self.state.get(&policy_key).await? {
                let policy = serde_json::from_str::<acteon_core::QuotaPolicy>(&data)
                    .map_err(|e| GatewayError::Configuration(e.to_string()))?;
                policies.push(policy);
            }
        }
        Ok(policies)
    }
}
