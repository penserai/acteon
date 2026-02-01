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
}
