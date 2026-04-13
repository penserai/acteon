//! Time interval registry and dispatch-path enforcement.
//!
//! Time intervals are named, tenant-scoped recurring schedules. Rules
//! reference them by name through their [`mute_time_intervals`] and
//! [`active_time_intervals`] fields (mirroring Alertmanager). At dispatch
//! time the gateway looks up the matched rule, evaluates its referenced
//! intervals against `Utc::now()`, and short-circuits to
//! [`ActionOutcome::Muted`](acteon_core::ActionOutcome::Muted) if the
//! schedule says the rule should not fire right now.
//!
//! [`mute_time_intervals`]: acteon_rules::Rule::mute_time_intervals
//! [`active_time_intervals`]: acteon_rules::Rule::active_time_intervals

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use tracing::{debug, warn};

use acteon_core::TimeInterval;
use acteon_state::{KeyKind, StateKey};

use crate::error::GatewayError;
use crate::gateway::Gateway;

/// Outcome of evaluating a rule's time-interval references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeIntervalDecision {
    /// No interval gating applies — proceed with the verdict.
    Proceed,
    /// Mute the action: a `mute_time_intervals` interval matched right now.
    MutedByMute(String),
    /// Mute the action: at least one `active_time_intervals` interval is
    /// configured but none of them match right now.
    MutedByInactive(String),
}

impl TimeIntervalDecision {
    /// Convenience: short reason string for the [`ActionOutcome::Muted`] payload.
    #[must_use]
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Proceed => "proceed",
            Self::MutedByMute(_) => "mute_time_interval",
            Self::MutedByInactive(_) => "active_time_interval",
        }
    }

    /// The interval name responsible for the decision, if any.
    #[must_use]
    pub fn interval_name(&self) -> Option<&str> {
        match self {
            Self::Proceed => None,
            Self::MutedByMute(name) | Self::MutedByInactive(name) => Some(name),
        }
    }
}

/// Hierarchical tenant match identical to silence's `tenant_covers`. A
/// time interval defined for `acme` covers `acme.us-east`,
/// `acme.us-east.prod`, etc.
fn tenant_covers(interval_tenant: &str, action_tenant: &str) -> bool {
    if interval_tenant == action_tenant {
        return true;
    }
    action_tenant.len() > interval_tenant.len() + 1
        && action_tenant.starts_with(interval_tenant)
        && action_tenant.as_bytes()[interval_tenant.len()] == b'.'
}

/// Composite cache key for the in-memory time interval registry. We key on
/// `namespace` only and match the tenant per-interval at lookup time so
/// that hierarchical tenant matching works (mirroring silences).
type IntervalCache = HashMap<String, Vec<TimeInterval>>;

fn time_interval_state_key(id: &str) -> StateKey {
    StateKey::new("_system", "_time_intervals", KeyKind::TimeInterval, id)
}

/// Stable cache ID for a time interval. Names are unique per
/// `(namespace, tenant)` so the cache key combines them.
#[must_use]
pub fn time_interval_cache_id(namespace: &str, tenant: &str, name: &str) -> String {
    format!("{namespace}:{tenant}:{name}")
}

impl Gateway {
    /// Evaluate the matched rule's time-interval references against `now`.
    ///
    /// `rule_name` is the name returned by the rule engine for the matched
    /// rule (or `None` if no rule matched — in that case the dispatch is
    /// always allowed to proceed because the implicit default-allow has no
    /// interval references).
    ///
    /// **Hierarchical tenant matching** is enforced: an interval defined
    /// at `acme` covers actions for `acme.us-east`. Intervals are looked
    /// up by `(namespace, name)` first, then filtered by tenant cover.
    #[must_use]
    pub fn check_time_intervals(
        &self,
        action_namespace: &str,
        action_tenant: &str,
        rule_name: Option<&str>,
        now: DateTime<Utc>,
    ) -> TimeIntervalDecision {
        let Some(rule_name) = rule_name else {
            return TimeIntervalDecision::Proceed;
        };

        // Look up the rule by name in the engine. If the rule can't be
        // found (it was disabled or removed between evaluate and check),
        // we conservatively proceed — the rule engine already determined
        // the verdict and we don't want a stale lookup to silently mute.
        let Some(rule) = self.engine.rules().iter().find(|r| r.name == rule_name) else {
            return TimeIntervalDecision::Proceed;
        };

        if rule.mute_time_intervals.is_empty() && rule.active_time_intervals.is_empty() {
            return TimeIntervalDecision::Proceed;
        }

        let cache = self.time_intervals.read();

        // Mute intervals — first match wins. A missing name is
        // fail-open (proceed) but warned so typos show up in logs.
        for name in &rule.mute_time_intervals {
            match lookup_interval(&cache, action_namespace, action_tenant, name) {
                Some(interval) if interval.matches_at(now) => {
                    debug!(
                        interval = %name,
                        rule = %rule_name,
                        "action muted by mute_time_interval"
                    );
                    return TimeIntervalDecision::MutedByMute(name.clone());
                }
                Some(_) => {}
                None => {
                    warn!(
                        interval = %name,
                        rule = %rule_name,
                        namespace = %action_namespace,
                        tenant = %action_tenant,
                        "rule references unknown mute_time_interval (typo?) — treating as non-matching"
                    );
                }
            }
        }

        // Active intervals — if any are configured, at least one must
        // match for the dispatch to proceed. Missing names here are
        // fail-closed (contributing to the mute), which would silently
        // break all matching dispatches on a typo; emit a warn so the
        // operator can spot it in logs. See the feature page
        // `docs/book/features/time-intervals.md` for the full model.
        if !rule.active_time_intervals.is_empty() {
            let mut any_matched = false;
            let mut last_seen: Option<String> = None;
            for name in &rule.active_time_intervals {
                match lookup_interval(&cache, action_namespace, action_tenant, name) {
                    Some(interval) => {
                        last_seen = Some(name.clone());
                        if interval.matches_at(now) {
                            any_matched = true;
                            break;
                        }
                    }
                    None => {
                        warn!(
                            interval = %name,
                            rule = %rule_name,
                            namespace = %action_namespace,
                            tenant = %action_tenant,
                            "rule references unknown active_time_interval (typo?) — treating as not-currently-active (fail-closed)"
                        );
                    }
                }
            }
            if !any_matched {
                let name = last_seen.unwrap_or_else(|| {
                    rule.active_time_intervals
                        .first()
                        .cloned()
                        .unwrap_or_default()
                });
                debug!(
                    interval = %name,
                    rule = %rule_name,
                    "action muted: outside active_time_interval"
                );
                return TimeIntervalDecision::MutedByInactive(name);
            }
        }

        TimeIntervalDecision::Proceed
    }

    /// Insert or replace a time interval in the in-memory cache.
    ///
    /// # Errors
    ///
    /// Returns an error if the interval fails [`TimeInterval::validate`].
    pub fn upsert_time_interval_cache(&self, interval: TimeInterval) -> Result<(), String> {
        interval.validate()?;
        let namespace = interval.namespace.clone();
        let mut cache = self.time_intervals.write();
        let list = cache.entry(namespace).or_default();
        list.retain(|t| !(t.tenant == interval.tenant && t.name == interval.name));
        list.push(interval);
        Ok(())
    }

    /// Remove a time interval from the in-memory cache by `(ns, tenant, name)`.
    pub fn remove_time_interval_cache(&self, namespace: &str, tenant: &str, name: &str) {
        let mut cache = self.time_intervals.write();
        if let Some(list) = cache.get_mut(namespace) {
            list.retain(|t| !(t.tenant == tenant && t.name == name));
            if list.is_empty() {
                cache.remove(namespace);
            }
        }
    }

    /// Persist a time interval to the state store.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization or the state store write fails.
    pub async fn persist_time_interval(&self, interval: &TimeInterval) -> Result<(), GatewayError> {
        let id = time_interval_cache_id(&interval.namespace, &interval.tenant, &interval.name);
        let key = time_interval_state_key(&id);
        let value = serde_json::to_string(interval).map_err(|e| {
            GatewayError::Configuration(format!("time interval serialize failed: {e}"))
        })?;
        self.state
            .set(&key, &value, None)
            .await
            .map_err(GatewayError::from)
    }

    /// Delete a time interval from the state store.
    ///
    /// # Errors
    ///
    /// Returns an error if the state store delete fails.
    pub async fn delete_time_interval(
        &self,
        namespace: &str,
        tenant: &str,
        name: &str,
    ) -> Result<(), GatewayError> {
        let id = time_interval_cache_id(namespace, tenant, name);
        let key = time_interval_state_key(&id);
        self.state.delete(&key).await.map_err(GatewayError::from)?;
        Ok(())
    }

    /// Fetch a single time interval from the state store.
    ///
    /// # Errors
    ///
    /// Returns an error if the state store read or JSON parsing fails.
    pub async fn get_time_interval(
        &self,
        namespace: &str,
        tenant: &str,
        name: &str,
    ) -> Result<Option<TimeInterval>, GatewayError> {
        let id = time_interval_cache_id(namespace, tenant, name);
        let key = time_interval_state_key(&id);
        match self.state.get(&key).await.map_err(GatewayError::from)? {
            Some(value) => serde_json::from_str::<TimeInterval>(&value)
                .map(Some)
                .map_err(|e| {
                    GatewayError::Configuration(format!("time interval parse failed: {e}"))
                }),
            None => Ok(None),
        }
    }

    /// List all time intervals in the cache, optionally filtered.
    #[must_use]
    pub fn list_time_intervals(
        &self,
        namespace: Option<&str>,
        tenant: Option<&str>,
    ) -> Vec<TimeInterval> {
        let cache = self.time_intervals.read();
        cache
            .iter()
            .filter(|(ns, _)| namespace.is_none_or(|n| n == ns.as_str()))
            .flat_map(|(_, list)| {
                list.iter()
                    .filter(|t| tenant.is_none_or(|tn| tn == t.tenant.as_str()))
                    .cloned()
            })
            .collect()
    }

    /// Number of intervals currently in the cache.
    #[must_use]
    pub fn time_interval_cache_size(&self) -> usize {
        self.time_intervals.read().values().map(Vec::len).sum()
    }

    /// Load all time intervals from the state store into the in-memory
    /// cache. Idempotent — replaces the entire cache atomically.
    ///
    /// # Errors
    ///
    /// Returns an error only if the underlying scan fails.
    pub async fn load_time_intervals_from_state_store(&self) -> Result<usize, GatewayError> {
        let entries = self
            .state
            .scan_keys_by_kind(KeyKind::TimeInterval)
            .await
            .map_err(GatewayError::from)?;

        let mut new_cache: IntervalCache = HashMap::new();
        let mut loaded = 0usize;
        for (_key, value) in entries {
            let Ok(interval) = serde_json::from_str::<TimeInterval>(&value) else {
                warn!("time interval load: failed to parse record");
                continue;
            };
            if let Err(e) = interval.validate() {
                warn!(error = %e, "time interval load: skipping invalid record");
                continue;
            }
            new_cache
                .entry(interval.namespace.clone())
                .or_default()
                .push(interval);
            loaded += 1;
        }
        *self.time_intervals.write() = new_cache;
        Ok(loaded)
    }
}

fn lookup_interval<'a>(
    cache: &'a IntervalCache,
    action_namespace: &str,
    action_tenant: &str,
    name: &str,
) -> Option<&'a TimeInterval> {
    let list = cache.get(action_namespace)?;
    list.iter()
        .find(|t| t.name == name && tenant_covers(&t.tenant, action_tenant))
}

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_core::time_interval::{TimeOfDayRange, TimeRange, WeekdayRange};
    use chrono::TimeZone;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    fn business_hours_interval(name: &str, namespace: &str, tenant: &str) -> TimeInterval {
        TimeInterval {
            name: name.into(),
            namespace: namespace.into(),
            tenant: tenant.into(),
            time_ranges: vec![TimeRange {
                times: vec![TimeOfDayRange::from_hm(9, 0, 17, 0).unwrap()],
                weekdays: vec![WeekdayRange { start: 1, end: 5 }],
                ..Default::default()
            }],
            location: Some("UTC".into()),
            description: None,
            created_by: "test".into(),
            created_at: utc(2026, 1, 1, 0, 0),
            updated_at: utc(2026, 1, 1, 0, 0),
        }
    }

    #[test]
    fn tenant_covers_hierarchy() {
        assert!(tenant_covers("acme", "acme"));
        assert!(tenant_covers("acme", "acme.us-east"));
        assert!(!tenant_covers("acme", "acme-corp"));
        assert!(!tenant_covers("acme.us-east", "acme"));
    }

    #[test]
    fn decision_reason_strings_are_stable() {
        assert_eq!(TimeIntervalDecision::Proceed.reason(), "proceed");
        assert_eq!(
            TimeIntervalDecision::MutedByMute("x".into()).reason(),
            "mute_time_interval"
        );
        assert_eq!(
            TimeIntervalDecision::MutedByInactive("x".into()).reason(),
            "active_time_interval"
        );
    }

    #[test]
    fn lookup_interval_respects_hierarchy() {
        let mut cache = IntervalCache::new();
        cache
            .entry("prod".into())
            .or_default()
            .push(business_hours_interval("biz", "prod", "acme"));
        // Action on `acme.us-east` finds the parent-tenant interval.
        assert!(lookup_interval(&cache, "prod", "acme.us-east", "biz").is_some());
        // Action on a sibling tenant does not.
        assert!(lookup_interval(&cache, "prod", "other", "biz").is_none());
    }

    // =========================================================================
    // Missing-name fail-open (mute) vs fail-closed (active) contract
    // =========================================================================

    use crate::GatewayBuilder;
    use acteon_rules::Rule;
    use acteon_rules::ir::expr::Expr;
    use acteon_rules::ir::rule::RuleAction;
    use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
    use std::sync::Arc;

    fn rule_with_intervals(mute: Vec<&str>, active: Vec<&str>) -> Rule {
        Rule::new("test-rule", Expr::Bool(true), RuleAction::Allow)
            .with_mute_time_intervals(mute.into_iter().map(String::from).collect())
            .with_active_time_intervals(active.into_iter().map(String::from).collect())
    }

    async fn build_gateway_with_rule(rule: Rule) -> crate::Gateway {
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());
        GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .rules(vec![rule])
            .build()
            .expect("build gateway")
    }

    #[tokio::test]
    async fn unknown_mute_interval_name_fails_open() {
        // A rule references a nonexistent mute interval (typo). The
        // contract is fail-open: the action proceeds, but a warn! log
        // records the missing name so operators can spot the typo.
        let gw = build_gateway_with_rule(rule_with_intervals(vec!["does-not-exist"], vec![])).await;
        let decision = gw.check_time_intervals("prod", "acme", Some("test-rule"), Utc::now());
        assert_eq!(decision, TimeIntervalDecision::Proceed);
    }

    #[tokio::test]
    async fn unknown_active_interval_name_fails_closed() {
        // A rule references a nonexistent active interval (typo). The
        // contract is fail-closed: the dispatch is muted, because an
        // "active window" requirement that can't be satisfied should
        // never silently fire. A warn! log records the missing name.
        let gw = build_gateway_with_rule(rule_with_intervals(vec![], vec!["does-not-exist"])).await;
        let decision = gw.check_time_intervals("prod", "acme", Some("test-rule"), Utc::now());
        assert!(
            matches!(decision, TimeIntervalDecision::MutedByInactive(_)),
            "expected MutedByInactive, got {decision:?}"
        );
    }

    #[tokio::test]
    async fn rule_without_intervals_is_unaffected() {
        let gw = build_gateway_with_rule(rule_with_intervals(vec![], vec![])).await;
        let decision = gw.check_time_intervals("prod", "acme", Some("test-rule"), Utc::now());
        assert_eq!(decision, TimeIntervalDecision::Proceed);
    }

    #[tokio::test]
    async fn unknown_rule_name_proceeds() {
        // A verdict referencing a rule name that's no longer in the
        // engine (hot-reload race) must not silently mute.
        let gw = build_gateway_with_rule(rule_with_intervals(vec!["x"], vec!["y"])).await;
        let decision = gw.check_time_intervals("prod", "acme", Some("vanished-rule"), Utc::now());
        assert_eq!(decision, TimeIntervalDecision::Proceed);
    }
}
