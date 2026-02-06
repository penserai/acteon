use std::time::Duration;

use async_trait::async_trait;

use crate::error::StateError;
use crate::key::StateKey;

/// Result of a compare-and-swap operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CasResult {
    /// The swap succeeded and the new version is stored.
    Ok,
    /// The swap failed because the current version didn't match.
    Conflict {
        current_value: Option<String>,
        current_version: u64,
    },
}

/// Trait for persisting action state.
///
/// Implementations must be `Send + Sync` and safe for concurrent access.
#[async_trait]
pub trait StateStore: Send + Sync {
    /// Check if a key exists; if not, set it atomically with an optional TTL.
    /// Returns `true` if the key was newly set, `false` if it already existed.
    async fn check_and_set(
        &self,
        key: &StateKey,
        value: &str,
        ttl: Option<Duration>,
    ) -> Result<bool, StateError>;

    /// Get the value for a key. Returns `None` if not found or expired.
    async fn get(&self, key: &StateKey) -> Result<Option<String>, StateError>;

    /// Set a value with an optional TTL, overwriting any previous value.
    async fn set(
        &self,
        key: &StateKey,
        value: &str,
        ttl: Option<Duration>,
    ) -> Result<(), StateError>;

    /// Delete a key. Returns `true` if the key existed.
    async fn delete(&self, key: &StateKey) -> Result<bool, StateError>;

    /// Atomically increment a counter by `delta`. Returns the new value.
    /// Creates the counter at 0 if it doesn't exist before incrementing.
    async fn increment(
        &self,
        key: &StateKey,
        delta: i64,
        ttl: Option<Duration>,
    ) -> Result<i64, StateError>;

    /// Compare-and-swap: update value only if the current version matches.
    async fn compare_and_swap(
        &self,
        key: &StateKey,
        expected_version: u64,
        new_value: &str,
        ttl: Option<Duration>,
    ) -> Result<CasResult, StateError>;

    /// Scan keys matching a prefix pattern.
    ///
    /// Returns a list of (key, value) pairs where the key matches the given
    /// namespace, tenant, and kind. The `prefix` parameter filters keys that
    /// start with the given string after the kind prefix.
    ///
    /// This operation may be expensive on some backends. Use sparingly.
    async fn scan_keys(
        &self,
        namespace: &str,
        tenant: &str,
        kind: crate::key::KeyKind,
        prefix: Option<&str>,
    ) -> Result<Vec<(String, String)>, StateError>;

    /// Scan all keys of a given kind across all namespaces and tenants.
    ///
    /// Returns a list of (key, value) pairs. The key format is
    /// `{namespace}:{tenant}:{kind}:{identifier}`.
    ///
    /// This operation scans the entire keyspace for the given kind, which
    /// can be expensive. Use sparingly and consider pagination for large datasets.
    async fn scan_keys_by_kind(
        &self,
        kind: crate::key::KeyKind,
    ) -> Result<Vec<(String, String)>, StateError>;

    /// Add a key to the timeout index with its expiration timestamp.
    ///
    /// This enables efficient O(log N) queries for expired timeouts instead of
    /// scanning all timeout keys. The `expires_at` is a Unix timestamp in milliseconds.
    async fn index_timeout(&self, key: &StateKey, expires_at_ms: i64) -> Result<(), StateError>;

    /// Remove a key from the timeout index.
    async fn remove_timeout_index(&self, key: &StateKey) -> Result<(), StateError>;

    /// Get all timeout keys that have expired (`expires_at` <= now).
    ///
    /// Returns a list of canonical key strings. This is O(log N + M) where M is
    /// the number of expired keys, compared to O(N) for scanning all timeouts.
    async fn get_expired_timeouts(&self, now_ms: i64) -> Result<Vec<String>, StateError>;

    /// Add a chain to the ready index with a `ready_at` timestamp (ms).
    ///
    /// Chains with `ready_at <= now` will be returned by [`get_ready_chains`].
    async fn index_chain_ready(&self, key: &StateKey, ready_at_ms: i64) -> Result<(), StateError> {
        let _ = (key, ready_at_ms);
        Ok(())
    }

    /// Remove a chain from the ready index.
    async fn remove_chain_ready_index(&self, key: &StateKey) -> Result<(), StateError> {
        let _ = key;
        Ok(())
    }

    /// Get all chains that are ready for advancement (`ready_at <= now_ms`).
    ///
    /// Returns canonical key strings. The default implementation falls back to
    /// [`scan_keys_by_kind`] with `PendingChains` (O(N)).
    async fn get_ready_chains(&self, now_ms: i64) -> Result<Vec<String>, StateError> {
        let _ = now_ms;
        let entries = self
            .scan_keys_by_kind(crate::key::KeyKind::PendingChains)
            .await?;
        Ok(entries.into_iter().map(|(k, _)| k).collect())
    }
}
