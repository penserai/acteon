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

use acteon_core::silence::{MAX_REGEX_SIZE, MatchOp, Silence, SilenceMatcher};
use acteon_core::{Action, ActionMetadata};
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
    /// # Errors
    ///
    /// Returns an error string if any regex matcher fails to compile.
    pub fn new(silence: Silence) -> Result<Self, String> {
        let mut compiled = Vec::with_capacity(silence.matchers.len());
        for matcher in &silence.matchers {
            match matcher.op {
                MatchOp::Regex | MatchOp::NotRegex => {
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
    #[must_use]
    pub fn check_silence(&self, action: &Action) -> Option<String> {
        let now = Utc::now();
        let key = (action.namespace.to_string(), action.tenant.to_string());
        let silences = self.silences.read();
        let list = silences.get(&key)?;
        let labels = action_labels(&action.metadata);
        for cached in list {
            if cached.applies_to(labels, now) {
                debug!(
                    silence_id = %cached.silence.id,
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
        let key = (
            cached.silence.namespace.clone(),
            cached.silence.tenant.clone(),
        );
        let mut silences = self.silences.write();
        let list = silences.entry(key).or_default();
        // Remove any existing entry with the same ID, then push the new one.
        list.retain(|s| s.silence.id != cached.silence.id);
        list.push(cached);
        Ok(())
    }

    /// Remove a silence from the in-memory cache by ID.
    pub fn remove_silence_cache(&self, namespace: &str, tenant: &str, silence_id: &str) {
        let key = (namespace.to_string(), tenant.to_string());
        let mut silences = self.silences.write();
        if let Some(list) = silences.get_mut(&key) {
            list.retain(|s| s.silence.id != silence_id);
            if list.is_empty() {
                silences.remove(&key);
            }
        }
    }

    /// Load all silences from the state store into the in-memory cache.
    ///
    /// Called from server startup after the gateway is constructed. Idempotent —
    /// can be called repeatedly to refresh the cache.
    ///
    /// # Errors
    ///
    /// Returns an error if the state store scan fails. Individual silences
    /// that fail to deserialize or compile are logged and skipped so one
    /// bad record doesn't prevent the rest from loading.
    pub async fn load_silences_from_state_store(&self) -> Result<usize, GatewayError> {
        let entries = self
            .state
            .scan_keys_by_kind(KeyKind::Silence)
            .await
            .map_err(GatewayError::from)?;

        let mut new_cache: HashMap<(String, String), Vec<CachedSilence>> = HashMap::new();
        let mut loaded = 0usize;

        for (_key, value) in entries {
            let Ok(silence) = serde_json::from_str::<Silence>(&value) else {
                warn!("silence cache load: failed to parse silence record");
                continue;
            };
            match CachedSilence::new(silence) {
                Ok(cached) => {
                    let key = (
                        cached.silence.namespace.clone(),
                        cached.silence.tenant.clone(),
                    );
                    new_cache.entry(key).or_default().push(cached);
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
    #[must_use]
    pub fn list_silences(&self, namespace: Option<&str>, tenant: Option<&str>) -> Vec<Silence> {
        let silences = self.silences.read();
        silences
            .iter()
            .filter(|((ns, t), _)| {
                namespace.is_none_or(|n| n == ns) && tenant.is_none_or(|x| x == t)
            })
            .flat_map(|(_, list)| list.iter().map(|c| c.silence.clone()))
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
        for ((ns, tenant), list) in silences.iter() {
            for cached in list {
                if cached.silence.ends_at <= now {
                    out.push((ns.clone(), tenant.clone(), cached.silence.id.clone()));
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
}
