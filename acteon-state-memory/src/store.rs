use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;
use tokio::time::Instant;

use acteon_state::error::StateError;
use acteon_state::key::StateKey;
use acteon_state::store::{CasResult, StateStore};

/// A single entry in the in-memory store.
#[derive(Debug, Clone)]
struct Entry {
    value: String,
    version: u64,
    expires_at: Option<Instant>,
}

impl Entry {
    /// Returns `true` if this entry has passed its TTL deadline.
    fn is_expired(&self) -> bool {
        self.expires_at
            .is_some_and(|deadline| Instant::now() >= deadline)
    }
}

/// Compute the expiry instant from an optional TTL duration.
fn expiry_from_ttl(ttl: Option<Duration>) -> Option<Instant> {
    ttl.map(|d| Instant::now() + d)
}

/// In-memory [`StateStore`] backed by a [`DashMap`].
///
/// Entries are lazily evicted on read when their TTL has elapsed. This
/// implementation is fully synchronous internally; the async trait methods
/// return immediately.
#[derive(Debug, Default)]
pub struct MemoryStateStore {
    data: DashMap<String, Entry>,
}

impl MemoryStateStore {
    /// Create a new, empty in-memory state store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Render a [`StateKey`] into the string used as the map key.
    fn render_key(key: &StateKey) -> String {
        key.canonical()
    }
}

#[async_trait]
impl StateStore for MemoryStateStore {
    async fn check_and_set(
        &self,
        key: &StateKey,
        value: &str,
        ttl: Option<Duration>,
    ) -> Result<bool, StateError> {
        let rendered = Self::render_key(key);

        // Check if a live entry already exists.
        if let Some(existing) = self.data.get(&rendered) {
            if !existing.is_expired() {
                return Ok(false);
            }
        }
        // Drop the read guard before writing.
        // Remove any expired entry, then try to insert.
        self.data
            .remove_if(&rendered, |_, entry| entry.is_expired());

        // Use `entry` API for atomicity: only insert if vacant.
        let was_inserted = match self.data.entry(rendered) {
            dashmap::mapref::entry::Entry::Occupied(_) => false,
            dashmap::mapref::entry::Entry::Vacant(vacant) => {
                vacant.insert(Entry {
                    value: value.to_owned(),
                    version: 1,
                    expires_at: expiry_from_ttl(ttl),
                });
                true
            }
        };

        Ok(was_inserted)
    }

    async fn get(&self, key: &StateKey) -> Result<Option<String>, StateError> {
        let rendered = Self::render_key(key);

        // Lazy TTL eviction: check and remove if expired.
        if let Some(entry) = self.data.get(&rendered) {
            if entry.is_expired() {
                drop(entry);
                self.data.remove(&rendered);
                return Ok(None);
            }
            return Ok(Some(entry.value.clone()));
        }

        Ok(None)
    }

    async fn set(
        &self,
        key: &StateKey,
        value: &str,
        ttl: Option<Duration>,
    ) -> Result<(), StateError> {
        let rendered = Self::render_key(key);
        let expires_at = expiry_from_ttl(ttl);

        self.data
            .entry(rendered)
            .and_modify(|entry| {
                value.clone_into(&mut entry.value);
                entry.version += 1;
                entry.expires_at = expires_at;
            })
            .or_insert_with(|| Entry {
                value: value.to_owned(),
                version: 1,
                expires_at,
            });

        Ok(())
    }

    async fn delete(&self, key: &StateKey) -> Result<bool, StateError> {
        let rendered = Self::render_key(key);

        // Remove, but treat expired entries as "not found".
        match self.data.remove(&rendered) {
            Some((_, entry)) => Ok(!entry.is_expired()),
            None => Ok(false),
        }
    }

    async fn increment(
        &self,
        key: &StateKey,
        delta: i64,
        ttl: Option<Duration>,
    ) -> Result<i64, StateError> {
        let rendered = Self::render_key(key);
        let expires_at = expiry_from_ttl(ttl);

        // Remove any expired entry first so the counter starts fresh.
        self.data
            .remove_if(&rendered, |_, entry| entry.is_expired());

        let mut ref_mut = self.data.entry(rendered).or_insert_with(|| Entry {
            value: "0".to_owned(),
            version: 1,
            expires_at,
        });

        let current: i64 = ref_mut
            .value
            .parse()
            .map_err(|e: std::num::ParseIntError| {
                StateError::Serialization(format!("counter value is not an integer: {e}"))
            })?;

        let new_value = current + delta;
        ref_mut.value = new_value.to_string();
        ref_mut.version += 1;
        if let Some(ea) = expires_at {
            ref_mut.expires_at = Some(ea);
        }

        Ok(new_value)
    }

    async fn compare_and_swap(
        &self,
        key: &StateKey,
        expected_version: u64,
        new_value: &str,
        ttl: Option<Duration>,
    ) -> Result<CasResult, StateError> {
        let rendered = Self::render_key(key);

        // Remove expired entries so they appear as missing.
        self.data
            .remove_if(&rendered, |_, entry| entry.is_expired());

        let Some(mut entry) = self.data.get_mut(&rendered) else {
            return Ok(CasResult::Conflict {
                current_value: None,
                current_version: 0,
            });
        };

        if entry.version != expected_version {
            return Ok(CasResult::Conflict {
                current_value: Some(entry.value.clone()),
                current_version: entry.version,
            });
        }

        new_value.clone_into(&mut entry.value);
        entry.version += 1;
        entry.expires_at = expiry_from_ttl(ttl).or(entry.expires_at);

        Ok(CasResult::Ok)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use acteon_state::key::{KeyKind, StateKey};
    use acteon_state::testing::run_store_conformance_tests;

    use super::*;

    fn test_key(kind: KeyKind, id: &str) -> StateKey {
        StateKey::new("test-ns", "test-tenant", kind, id)
    }

    #[tokio::test]
    async fn conformance() {
        let store = MemoryStateStore::new();
        run_store_conformance_tests(&store)
            .await
            .expect("conformance tests should pass");
    }

    #[tokio::test(start_paused = true)]
    async fn ttl_expiry_via_get() {
        let store = MemoryStateStore::new();
        let key = test_key(KeyKind::State, "ttl-expire");

        store
            .set(&key, "short-lived", Some(Duration::from_secs(5)))
            .await
            .unwrap();

        // Value should be present before TTL elapses.
        let val = store.get(&key).await.unwrap();
        assert_eq!(val.as_deref(), Some("short-lived"));

        // Advance time past TTL.
        tokio::time::advance(Duration::from_secs(6)).await;

        // Lazy eviction: get should return None.
        let val = store.get(&key).await.unwrap();
        assert!(val.is_none(), "value should be expired");
    }

    #[tokio::test(start_paused = true)]
    async fn ttl_check_and_set_after_expiry() {
        let store = MemoryStateStore::new();
        let key = test_key(KeyKind::Dedup, "ttl-cas");

        let created = store
            .check_and_set(&key, "v1", Some(Duration::from_secs(3)))
            .await
            .unwrap();
        assert!(created);

        // Should fail while still alive.
        let created = store.check_and_set(&key, "v2", None).await.unwrap();
        assert!(!created);

        // Advance past TTL.
        tokio::time::advance(Duration::from_secs(4)).await;

        // Should succeed now that the entry has expired.
        let created = store.check_and_set(&key, "v2", None).await.unwrap();
        assert!(created, "should re-create after expiry");

        let val = store.get(&key).await.unwrap();
        assert_eq!(val.as_deref(), Some("v2"));
    }

    #[tokio::test(start_paused = true)]
    async fn ttl_increment_resets_after_expiry() {
        let store = MemoryStateStore::new();
        let key = test_key(KeyKind::Counter, "ttl-counter");

        store
            .increment(&key, 10, Some(Duration::from_secs(2)))
            .await
            .unwrap();

        tokio::time::advance(Duration::from_secs(3)).await;

        // After expiry the counter should restart from zero.
        let val = store.increment(&key, 1, None).await.unwrap();
        assert_eq!(val, 1, "counter should reset after TTL expiry");
    }

    #[tokio::test]
    async fn delete_returns_false_for_missing() {
        let store = MemoryStateStore::new();
        let key = test_key(KeyKind::State, "never-set");
        let existed = store.delete(&key).await.unwrap();
        assert!(!existed);
    }
}
