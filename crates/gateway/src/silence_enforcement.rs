//! Silence enforcement methods on [`Gateway`].
//!
//! Silences are time-bounded label-pattern mutes that suppress dispatched
//! actions in the dispatch pipeline. They are evaluated after rule
//! evaluation but before provider dispatch, so the audit record still
//! captures the rule verdict that would have applied.
//!
//! See `docs/design-alertmanager-parity.md` for the design rationale.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use regex::{Regex, RegexBuilder};
use tracing::{debug, warn};

use acteon_core::silence::{
    MAX_REGEX_PATTERN_LEN, MAX_REGEX_SIZE, MatchOp, Silence, SilenceMatcher,
};
use acteon_core::{Action, ActionMetadata};

/// Hierarchical tenant match: the silence's tenant is either equal to
/// the action's tenant, or is a strict prefix of it delimited by `.`.
///
/// This mirrors the grant system's `tenant_matches` helper but for a
/// single pattern → single tenant comparison. A silence on `acme`
/// covers `acme`, `acme.us-east`, and `acme.us-east.prod`; it does NOT
/// cover `acme-corp` (no dot) or `acme-staging` (different suffix).
fn silence_tenant_covers(silence_tenant: &str, action_tenant: &str) -> bool {
    if silence_tenant == action_tenant {
        return true;
    }
    action_tenant.len() > silence_tenant.len() + 1
        && action_tenant.starts_with(silence_tenant)
        && action_tenant.as_bytes()[silence_tenant.len()] == b'.'
}
use acteon_state::{KeyKind, StateKey};

use crate::error::GatewayError;
use crate::gateway::Gateway;

/// A silence with its regex matchers pre-compiled.
///
/// Compilation happens at cache insertion time so the hot path
/// (dispatch-time lookup) never has to pay regex build cost.
#[derive(Debug, Clone)]
pub struct CachedSilence {
    pub silence: Silence,
    /// Pre-compiled regexes, one per matcher. `None` for non-regex ops.
    compiled: Vec<Option<Regex>>,
}

impl CachedSilence {
    /// Build a cached silence from a raw silence, compiling any regex matchers.
    ///
    /// This is the single safety gate for state-store records: bad
    /// silences (oversized regex, malformed pattern, over-length pattern)
    /// are rejected here, so [`Gateway::load_silences_from_state_store`]
    /// will skip them with a warning rather than load them into the
    /// dispatch-path cache. A malicious or corrupted state-store write
    /// cannot take down the gateway.
    ///
    /// # Errors
    ///
    /// Returns an error string if any regex matcher exceeds the pattern
    /// length cap or fails to compile within the DFA size cap.
    pub fn new(silence: Silence) -> Result<Self, String> {
        let mut compiled = Vec::with_capacity(silence.matchers.len());
        for matcher in &silence.matchers {
            match matcher.op {
                MatchOp::Regex | MatchOp::NotRegex => {
                    if matcher.value.len() > MAX_REGEX_PATTERN_LEN {
                        return Err(format!(
                            "regex pattern exceeds {MAX_REGEX_PATTERN_LEN}-character limit"
                        ));
                    }
                    compiled.push(Some(compile_anchored_regex(&matcher.value)?));
                }
                MatchOp::Equal | MatchOp::NotEqual => {
                    compiled.push(None);
                }
            }
        }
        Ok(Self { silence, compiled })
    }

    /// Check whether this silence applies to the given labels at `now`.
    #[must_use]
    pub fn applies_to(&self, labels: &HashMap<String, String>, now: DateTime<Utc>) -> bool {
        if !self.silence.is_active_at(now) {
            return false;
        }
        if self.silence.matchers.is_empty() {
            return false;
        }
        self.silence
            .matchers
            .iter()
            .zip(self.compiled.iter())
            .all(|(matcher, regex)| matcher_matches_with_cache(matcher, regex.as_ref(), labels))
    }
}

/// Anchored regex compilation matching [`SilenceMatcher::validate`].
fn compile_anchored_regex(pattern: &str) -> Result<Regex, String> {
    RegexBuilder::new(&format!("^(?:{pattern})$"))
        .size_limit(MAX_REGEX_SIZE)
        .dfa_size_limit(MAX_REGEX_SIZE)
        .build()
        .map_err(|e| format!("invalid regex pattern: {e}"))
}

/// Match a single matcher using the pre-compiled regex when available.
fn matcher_matches_with_cache(
    matcher: &SilenceMatcher,
    compiled: Option<&Regex>,
    labels: &HashMap<String, String>,
) -> bool {
    let label_value = labels.get(&matcher.name).map(String::as_str);

    match matcher.op {
        MatchOp::Equal => label_value == Some(matcher.value.as_str()),
        MatchOp::NotEqual => label_value != Some(matcher.value.as_str()),
        MatchOp::Regex => {
            let Some(value) = label_value else {
                return false;
            };
            compiled.is_some_and(|r| r.is_match(value))
        }
        MatchOp::NotRegex => {
            let Some(value) = label_value else {
                return true;
            };
            compiled.is_none_or(|r| !r.is_match(value))
        }
    }
}

/// Key prefix used for silence state-store entries.
///
/// Layout: `silence:{silence_id}` under a shared system namespace so that
/// the reaper can `scan_keys_by_kind(Silence)` to enumerate all silences.
fn silence_state_key(id: &str) -> StateKey {
    StateKey::new("_system", "_silences", KeyKind::Silence, id)
}

/// Best-effort label extractor from an action's metadata.
fn action_labels(metadata: &ActionMetadata) -> &HashMap<String, String> {
    &metadata.labels
}

impl Gateway {
    /// Check whether an action should be silenced.
    ///
    /// Returns the ID of the first matching silence, or `None`. Called
    /// from the dispatch pipeline after rule evaluation but before the
    /// verdict is executed.
    ///
    /// **Hierarchical tenant matching** is enforced here: a silence on
    /// tenant `acme` covers actions dispatched to `acme.us-east` and
    /// `acme.us-east.prod`. The cache is keyed by namespace alone, and
    /// each silence's `tenant` is checked against the action's tenant
    /// via a dot-strict prefix match.
    #[must_use]
    pub fn check_silence(&self, action: &Action) -> Option<String> {
        let now = Utc::now();
        let silences = self.silences.read();
        let list = silences.get(action.namespace.as_str())?;
        let labels = action_labels(&action.metadata);
        for cached in list {
            if !silence_tenant_covers(&cached.silence.tenant, action.tenant.as_str()) {
                continue;
            }
            if cached.applies_to(labels, now) {
                debug!(
                    silence_id = %cached.silence.id,
                    silence_tenant = %cached.silence.tenant,
                    action_tenant = %action.tenant,
                    action_id = %action.id,
                    "action silenced"
                );
                return Some(cached.silence.id.clone());
            }
        }
        None
    }

    /// Insert or replace a silence in the in-memory cache.
    ///
    /// Callers are responsible for persisting the underlying silence to
    /// the state store; this method only updates the cache.
    ///
    /// # Errors
    ///
    /// Returns an error if the silence has invalid regex matchers.
    pub fn upsert_silence_cache(&self, silence: Silence) -> Result<(), String> {
        let cached = CachedSilence::new(silence)?;
        let namespace = cached.silence.namespace.clone();
        let mut silences = self.silences.write();
        let list = silences.entry(namespace).or_default();
        // Remove any existing entry with the same ID, then push the new one.
        list.retain(|s| s.silence.id != cached.silence.id);
        list.push(cached);
        Ok(())
    }

    /// Remove a silence from the in-memory cache by ID.
    pub fn remove_silence_cache(&self, namespace: &str, _tenant: &str, silence_id: &str) {
        let mut silences = self.silences.write();
        if let Some(list) = silences.get_mut(namespace) {
            list.retain(|s| s.silence.id != silence_id);
            if list.is_empty() {
                silences.remove(namespace);
            }
        }
    }

    /// Load all silences from the state store into the in-memory cache.
    ///
    /// Called from server startup after the gateway is constructed, and
    /// periodically from the background processor to pick up changes
    /// made by peer instances. Idempotent — replaces the entire cache
    /// atomically.
    ///
    /// Malformed silences (bad regex, failed JSON parse) are logged and
    /// skipped so that one bad record cannot take down the gateway.
    ///
    /// # Errors
    ///
    /// Returns an error only if the state store scan itself fails.
    pub async fn load_silences_from_state_store(&self) -> Result<usize, GatewayError> {
        let entries = self
            .state
            .scan_keys_by_kind(KeyKind::Silence)
            .await
            .map_err(GatewayError::from)?;

        let mut new_cache: HashMap<String, Vec<CachedSilence>> = HashMap::new();
        let mut loaded = 0usize;

        for (_key, value) in entries {
            let Ok(silence) = serde_json::from_str::<Silence>(&value) else {
                warn!("silence cache load: failed to parse silence record");
                continue;
            };
            match CachedSilence::new(silence) {
                Ok(cached) => {
                    let namespace = cached.silence.namespace.clone();
                    new_cache.entry(namespace).or_default().push(cached);
                    loaded += 1;
                }
                Err(e) => {
                    warn!(error = %e, "silence cache load: skipping malformed silence");
                }
            }
        }

        *self.silences.write() = new_cache;
        Ok(loaded)
    }

    /// Periodically-invoked sync from the state store.
    ///
    /// Delegates to [`load_silences_from_state_store`](Self::load_silences_from_state_store).
    /// Called from the background processor so that silences created on
    /// peer gateway instances become visible here within the sync
    /// interval (default: 10 seconds), preventing split-brain silence
    /// behavior in HA deployments.
    ///
    /// # Errors
    ///
    /// Returns an error only if the underlying state store scan fails.
    pub async fn sync_silences_from_store(&self) -> Result<usize, GatewayError> {
        self.load_silences_from_state_store().await
    }

    /// Persist a silence to the state store.
    ///
    /// Writes under `silence:{silence_id}`. The state store uses this
    /// suffix plus the `Silence` key kind so the reaper can enumerate
    /// all silences with `scan_keys_by_kind`.
    ///
    /// # Errors
    ///
    /// Returns an error if JSON serialization or the state store write fails.
    pub async fn persist_silence(&self, silence: &Silence) -> Result<(), GatewayError> {
        let key = silence_state_key(&silence.id);
        let value = serde_json::to_string(silence)
            .map_err(|e| GatewayError::Configuration(format!("silence serialize failed: {e}")))?;
        self.state
            .set(&key, &value, None)
            .await
            .map_err(GatewayError::from)
    }

    /// Delete a silence from the state store by ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the state store delete fails.
    pub async fn delete_silence(&self, silence_id: &str) -> Result<(), GatewayError> {
        let key = silence_state_key(silence_id);
        self.state.delete(&key).await.map_err(GatewayError::from)?;
        Ok(())
    }

    /// Fetch a single silence from the state store by ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the state store read or JSON parsing fails.
    pub async fn get_silence(&self, silence_id: &str) -> Result<Option<Silence>, GatewayError> {
        let key = silence_state_key(silence_id);
        match self.state.get(&key).await.map_err(GatewayError::from)? {
            Some(value) => serde_json::from_str::<Silence>(&value)
                .map(Some)
                .map_err(|e| GatewayError::Configuration(format!("silence parse failed: {e}"))),
            None => Ok(None),
        }
    }

    /// List silences in the in-memory cache, optionally filtered by
    /// namespace/tenant.
    ///
    /// Filtering is **exact** at list time — a `tenant=acme` filter does
    /// NOT return silences created for `acme.us-east`. Hierarchical
    /// inheritance only applies to dispatch-path enforcement, not to
    /// the list view (which is about "show me silences explicitly
    /// created for this tenant").
    #[must_use]
    pub fn list_silences(&self, namespace: Option<&str>, tenant: Option<&str>) -> Vec<Silence> {
        let silences = self.silences.read();
        silences
            .iter()
            .filter(|(ns, _)| namespace.is_none_or(|n| n == ns.as_str()))
            .flat_map(|(_, list)| {
                list.iter()
                    .filter(|c| tenant.is_none_or(|t| t == c.silence.tenant.as_str()))
                    .map(|c| c.silence.clone())
            })
            .collect()
    }

    /// Return the silence cache size, for metrics and tests.
    #[must_use]
    pub fn silence_cache_size(&self) -> usize {
        self.silences.read().values().map(Vec::len).sum()
    }

    /// Scan the cache for silences whose `ends_at` is in the past, returning
    /// their IDs grouped by `(namespace, tenant)`.
    ///
    /// Used by the background reaper.
    #[must_use]
    pub fn expired_silence_ids(&self, now: DateTime<Utc>) -> Vec<(String, String, String)> {
        let silences = self.silences.read();
        let mut out = Vec::new();
        for (ns, list) in silences.iter() {
            for cached in list {
                if cached.silence.ends_at <= now {
                    out.push((
                        ns.clone(),
                        cached.silence.tenant.clone(),
                        cached.silence.id.clone(),
                    ));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn labels(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn silence_for(
        matchers: Vec<SilenceMatcher>,
        window_hours_ago: i64,
        window_hours_ahead: i64,
    ) -> Silence {
        let now = Utc::now();
        Silence {
            id: "test".to_owned(),
            namespace: "prod".to_owned(),
            tenant: "acme".to_owned(),
            matchers,
            starts_at: now - Duration::hours(window_hours_ago),
            ends_at: now + Duration::hours(window_hours_ahead),
            created_by: "test".to_owned(),
            comment: "test".to_owned(),
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn cached_silence_applies_to_matching_labels() {
        let s = silence_for(
            vec![SilenceMatcher::new("severity", "warning", MatchOp::Equal).unwrap()],
            1,
            1,
        );
        let cached = CachedSilence::new(s).unwrap();

        assert!(cached.applies_to(&labels(&[("severity", "warning")]), Utc::now()));
        assert!(!cached.applies_to(&labels(&[("severity", "critical")]), Utc::now()));
    }

    #[test]
    fn cached_silence_uses_precompiled_regex() {
        let s = silence_for(
            vec![SilenceMatcher::new("severity", "warn.*", MatchOp::Regex).unwrap()],
            1,
            1,
        );
        let cached = CachedSilence::new(s).unwrap();
        assert!(cached.applies_to(&labels(&[("severity", "warning")]), Utc::now()));
        assert!(!cached.applies_to(&labels(&[("severity", "critical")]), Utc::now()));
    }

    #[test]
    fn cached_silence_inactive_outside_window() {
        let s = silence_for(
            vec![SilenceMatcher::new("severity", "warning", MatchOp::Equal).unwrap()],
            3,  // starts 3h ago
            -2, // ends 2h ago (expired)
        );
        let cached = CachedSilence::new(s).unwrap();
        assert!(!cached.applies_to(&labels(&[("severity", "warning")]), Utc::now()));
    }

    // =========================================================================
    // Hierarchical tenant covers — pure-function tests
    // =========================================================================

    #[test]
    fn hierarchical_tenant_exact_match() {
        assert!(silence_tenant_covers("acme", "acme"));
    }

    #[test]
    fn hierarchical_tenant_child() {
        assert!(silence_tenant_covers("acme", "acme.us-east"));
        assert!(silence_tenant_covers("acme", "acme.us-east.prod"));
    }

    #[test]
    fn hierarchical_tenant_sibling_does_not_match() {
        assert!(!silence_tenant_covers("acme.us-east", "acme.eu-west"));
    }

    #[test]
    fn hierarchical_tenant_child_does_not_cover_parent() {
        assert!(!silence_tenant_covers("acme.us-east", "acme"));
    }

    #[test]
    fn hierarchical_tenant_dot_strict() {
        // Prefix match without the `.` separator must NOT match.
        assert!(!silence_tenant_covers("acme", "acme-corp"));
        assert!(!silence_tenant_covers("acme", "acmecorp"));
        assert!(!silence_tenant_covers("acme", "acmecar"));
    }

    #[test]
    fn hierarchical_tenant_unrelated_does_not_match() {
        assert!(!silence_tenant_covers("acme", "other"));
        assert!(!silence_tenant_covers("acme", ""));
    }

    // =========================================================================
    // Load-path safety: malformed state store entries must not panic
    // =========================================================================

    use crate::GatewayBuilder;
    use acteon_state::{StateKey, StateStore};
    use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

    /// Build a minimal gateway for load-path tests.
    async fn build_test_gateway() -> (crate::Gateway, std::sync::Arc<MemoryStateStore>) {
        let store = std::sync::Arc::new(MemoryStateStore::new());
        let lock = std::sync::Arc::new(MemoryDistributedLock::new());
        let gw = GatewayBuilder::new()
            .state(store.clone())
            .lock(lock)
            .build()
            .expect("build gateway");
        (gw, store)
    }

    fn silence_json(id: &str, pattern: &str) -> String {
        // Build the JSON directly so we can inject a regex that bypasses
        // the `SilenceMatcher::new` validator. This simulates a malicious
        // direct state-store write or an old server version that didn't
        // enforce the current caps.
        let now = chrono::Utc::now();
        let later = now + chrono::Duration::hours(1);
        format!(
            r#"{{
                "id": "{id}",
                "namespace": "prod",
                "tenant": "acme",
                "matchers": [
                    {{
                        "name": "severity",
                        "value": {pattern:?},
                        "op": "regex"
                    }}
                ],
                "starts_at": "{now_s}",
                "ends_at": "{later_s}",
                "created_by": "test",
                "comment": "load-path test",
                "created_at": "{now_s}",
                "updated_at": "{now_s}"
            }}"#,
            now_s = now.to_rfc3339(),
            later_s = later.to_rfc3339(),
        )
    }

    #[tokio::test]
    async fn load_skips_malformed_regex_without_panic() {
        let (gw, store) = build_test_gateway().await;

        // 1) A valid silence with a simple regex.
        let ok_id = "ok-silence";
        let ok_json = silence_json(ok_id, "warning");
        store
            .set(
                &StateKey::new("_system", "_silences", KeyKind::Silence, ok_id),
                &ok_json,
                None,
            )
            .await
            .unwrap();

        // 2) A silence with an oversized regex pattern that would fail the
        //    complexity cap at compile time.
        let bad_id = "bad-silence";
        let oversized = "a".repeat(1024);
        let bad_json = silence_json(bad_id, &oversized);
        store
            .set(
                &StateKey::new("_system", "_silences", KeyKind::Silence, bad_id),
                &bad_json,
                None,
            )
            .await
            .unwrap();

        // 3) A completely malformed JSON entry.
        let garbage_id = "garbage";
        store
            .set(
                &StateKey::new("_system", "_silences", KeyKind::Silence, garbage_id),
                "not valid json at all",
                None,
            )
            .await
            .unwrap();

        // Load must succeed and return count = 1 (only the valid silence).
        let loaded = gw.load_silences_from_state_store().await.unwrap();
        assert_eq!(
            loaded, 1,
            "only the valid silence should load; oversized and garbage are skipped"
        );

        // The valid silence is usable.
        let all = gw.list_silences(None, None);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, ok_id);
    }

    #[tokio::test]
    async fn load_handles_empty_state_store() {
        let (gw, _store) = build_test_gateway().await;
        let loaded = gw.load_silences_from_state_store().await.unwrap();
        assert_eq!(loaded, 0);
        assert_eq!(gw.silence_cache_size(), 0);
    }
}
