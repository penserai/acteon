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
        self.check_quota_inner(action, false).await
    }

    /// Re-check quota after a degrade-driven provider swap.
    ///
    /// Semantics: the generic (catch-all) policy was already
    /// charged and produced the degrade verdict on the previous
    /// pass, so re-enforcing it here would double-charge every
    /// degraded dispatch against the tenant-wide budget. Instead,
    /// only provider-scoped policies targeting the new provider
    /// are evaluated — closing the "degrade-to-bypass" hole where
    /// a fallback provider's own rate limit would otherwise be
    /// silently ignored.
    #[instrument(name = "gateway.check_quota_fallback", skip_all)]
    pub(crate) async fn check_quota_fallback(
        &self,
        action: &Action,
    ) -> Result<Option<ActionOutcome>, GatewayError> {
        self.check_quota_inner(action, true).await
    }

    async fn check_quota_inner(
        &self,
        action: &Action,
        only_provider_scoped: bool,
    ) -> Result<Option<ActionOutcome>, GatewayError> {
        const CACHE_TTL_SECS: i64 = 60;

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
        // In fallback mode (called after a degrade swap), the
        // generic catch-all is skipped because it was already
        // enforced on the original-provider pass — re-counting it
        // would double-charge the tenant-wide budget.
        let applicable: Vec<acteon_core::QuotaPolicy> = bucket_policies
            .into_iter()
            .filter(|p| {
                p.enabled
                    && p.applies_to_provider(&action.provider)
                    && (!only_provider_scoped || p.provider.is_some())
            })
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
            // Roll back every counter this call incremented, in
            // parallel, so the blocked action does not consume any
            // tenant budget. Rollback is best-effort:
            // compensating decrements that fail leave the counter
            // slightly inflated ("ghost consumption"), which can
            // cause a tenant to be blocked earlier than strictly
            // warranted. We log the errors but do not surface them
            // — a transient state-store blip should not turn into
            // a request-level failure on top of the legitimate
            // block.
            let rollbacks = incremented.iter().map(|other| {
                let state = self.state.clone();
                let key = other.counter_key.clone();
                let ttl = other.window_ttl;
                async move { state.increment(&key, -1, ttl).await }
            });
            let results = futures::future::join_all(rollbacks).await;
            for r in results {
                if let Err(e) = r {
                    warn!(error = %e, "quota rollback decrement failed (ghost consumption possible)");
                }
            }
        }

        let inc = &incremented[idx];
        Ok(self.apply_overage_behavior(action, &inc.policy, inc.used))
    }

    /// Increment the counter for every applicable policy, returning
    /// the per-policy state. Increments are issued concurrently via
    /// `join_all` so latency is O(1) round-trips rather than
    /// O(policies). On any state-store failure the helper rolls
    /// back every counter that did succeed (also in parallel) and
    /// returns `Err` so the caller can fail-open.
    ///
    /// Policies whose identifiers fail
    /// [`acteon_core::quota_counter_key`] validation (e.g., colon
    /// injection, zero window) are silently skipped with a warning
    /// log — the validation layers at the API, builder, and
    /// cold-path loader should prevent this from ever happening in
    /// steady state, but skipping is safer than panicking on a
    /// corrupt record.
    async fn increment_all_quota_counters(
        &self,
        action: &Action,
        policies: Vec<acteon_core::QuotaPolicy>,
        now: &chrono::DateTime<Utc>,
    ) -> Result<Vec<Incremented>, ()> {
        // Build per-policy (policy, counter_key, ttl) triples up
        // front, skipping any whose key cannot be constructed
        // safely. We need the triples both to issue the increments
        // concurrently AND to know what to roll back on failure.
        struct Prepared {
            policy: acteon_core::QuotaPolicy,
            counter_key: acteon_state::StateKey,
            window_ttl: Option<std::time::Duration>,
        }
        let mut prepared: Vec<Prepared> = Vec::with_capacity(policies.len());
        for policy in policies {
            let Some(counter_id) = acteon_core::quota_counter_key(
                &action.namespace,
                &action.tenant,
                policy.provider.as_deref(),
                &policy.window,
                now,
            ) else {
                warn!(
                    namespace = %action.namespace,
                    tenant = %action.tenant,
                    policy_id = %policy.id,
                    "skipping quota policy with invalid scope or zero window (fail-closed for this policy)"
                );
                continue;
            };
            let counter_key = acteon_state::StateKey::new(
                action.namespace.as_str(),
                action.tenant.as_str(),
                acteon_state::KeyKind::QuotaUsage,
                &counter_id,
            );
            let window_ttl = Some(std::time::Duration::from_secs(
                policy.window.duration_seconds(),
            ));
            prepared.push(Prepared {
                policy,
                counter_key,
                window_ttl,
            });
        }

        // Issue all increments concurrently. `join_all` yields
        // results in the same order as the input so we can zip
        // them back against `prepared` and preserve per-policy
        // attribution.
        let increments = prepared.iter().map(|p| {
            let state = self.state.clone();
            let key = p.counter_key.clone();
            let ttl = p.window_ttl;
            async move { state.increment(&key, 1, ttl).await }
        });
        let results = futures::future::join_all(increments).await;

        let mut incremented: Vec<Incremented> = Vec::with_capacity(prepared.len());
        let mut failure: Option<String> = None;
        for (prep, res) in prepared.into_iter().zip(results) {
            match res {
                Ok(new_count) => {
                    #[allow(clippy::cast_sign_loss)]
                    let used = new_count as u64;
                    incremented.push(Incremented {
                        policy: prep.policy,
                        counter_key: prep.counter_key,
                        window_ttl: prep.window_ttl,
                        used,
                    });
                }
                Err(e) => {
                    if failure.is_none() {
                        failure = Some(e.to_string());
                    }
                }
            }
        }

        if let Some(err) = failure {
            warn!(error = %err, "quota increment failed (fail-open)");
            // Roll back every counter that did succeed, concurrently.
            let rollbacks = incremented.iter().map(|inc| {
                let state = self.state.clone();
                let key = inc.counter_key.clone();
                let ttl = inc.window_ttl;
                async move { state.increment(&key, -1, ttl).await }
            });
            let _ = futures::future::join_all(rollbacks).await;
            return Err(());
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
                // Reject obviously-bad records (colon injection,
                // zero window, zero max_actions) at the cold-path
                // boundary so the enforcement path can assume
                // well-formed policies. The defense-in-depth means
                // a manually-inserted or pre-validation record
                // can't crash the gateway — worst case that policy
                // silently skips enforcement until it's repaired.
                if let Err(e) = policy.validate_scope() {
                    warn!(
                        policy_id = %id,
                        error = %e,
                        "skipping invalid quota policy loaded from state store"
                    );
                    continue;
                }
                policies.push(policy);
                if policies.len() >= acteon_core::MAX_POLICIES_PER_BUCKET {
                    warn!(
                        namespace = %namespace,
                        tenant = %tenant,
                        cap = acteon_core::MAX_POLICIES_PER_BUCKET,
                        "quota bucket hit per-tenant policy cap; ignoring remaining policy IDs from the index"
                    );
                    break;
                }
            }
        }
        Ok(policies)
    }
}
