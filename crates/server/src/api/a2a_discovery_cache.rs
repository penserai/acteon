//! Short-TTL cache for the A2A discovery endpoint.
//!
//! `GET /a2a/{ns}/{tenant}/.well-known/agent.json` is hit by every
//! A2A peer that wants to learn the tenant's agent surface; its
//! cold-path cost is `scan_keys(KeyKind::BusAgentCard)` over every
//! agent registered in the tenant. For a tenant with thousands of
//! agents that's a meaningful memory + latency spike on each call.
//!
//! This cache holds the **resolved** tenant card (verbatim or
//! aggregated) but **before** the intrinsic-scheme enrichment step,
//! so it is safe to share between the public discovery endpoint and
//! the authenticated `agent/getAuthenticatedExtendedCard` JSON-RPC
//! method.
//!
//! TTL is intentionally short (60s by default) so a `PUT` or
//! `DELETE` of an agent card takes effect quickly — and the mutating
//! endpoints invalidate the cache entry on write, so a successful
//! update is immediately visible.
//!
//! Concurrency: `tokio::sync::RwLock` around a `HashMap`. The read
//! path takes the lock in shared mode; writes (insert + invalidate)
//! take it exclusive. The lock is held for HashMap-lookup time only
//! — never across an `.await`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use acteon_core::AgentCard;
use tokio::sync::RwLock;

/// Default cache TTL. Short enough that a `PUT` or `DELETE` takes
/// effect quickly even without explicit invalidation; long enough to
/// fold a burst of discovery calls from one A2A peer into a single
/// scan.
pub const DEFAULT_DISCOVERY_TTL: Duration = Duration::from_secs(60);

/// Counters exposed for observability. Updated on the hot path with
/// `Ordering::Relaxed`. Like `PushDeliveryMetrics`, the snapshot is
/// internally non-consistent — fine for the "hit rate?" use case.
#[derive(Debug, Default)]
pub struct DiscoveryCacheMetrics {
    /// Cache lookups that returned a fresh entry. The hit rate is
    /// `hits / (hits + misses + expirations)`.
    pub hits: AtomicU64,
    /// Cache lookups that found no entry at all (cold cache or
    /// previously invalidated).
    pub misses: AtomicU64,
    /// Cache lookups that found an entry, but it had aged past the
    /// TTL. Counted separately from `misses` so an operator can
    /// distinguish "cache is too small" from "TTL is too short".
    pub expirations: AtomicU64,
    /// Explicit invalidations (a `PUT` or `DELETE` on an agent
    /// card).
    pub invalidations: AtomicU64,
}

impl DiscoveryCacheMetrics {
    /// Take an internally-non-consistent snapshot.
    #[must_use]
    pub fn snapshot(&self) -> DiscoveryCacheMetricsSnapshot {
        DiscoveryCacheMetricsSnapshot {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            expirations: self.expirations.load(Ordering::Relaxed),
            invalidations: self.invalidations.load(Ordering::Relaxed),
        }
    }
}

/// Snapshot of [`DiscoveryCacheMetrics`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryCacheMetricsSnapshot {
    pub hits: u64,
    pub misses: u64,
    pub expirations: u64,
    pub invalidations: u64,
}

type CacheKey = (String, String);

#[derive(Clone)]
struct CacheEntry {
    inserted_at: Instant,
    card: Arc<AgentCard>,
}

/// Short-TTL discovery cache. Construct once at server startup and
/// share via `Arc` through `AppState`.
pub struct DiscoveryCache {
    inner: RwLock<HashMap<CacheKey, CacheEntry>>,
    ttl: Duration,
    metrics: Arc<DiscoveryCacheMetrics>,
}

impl Default for DiscoveryCache {
    fn default() -> Self {
        Self::with_ttl(DEFAULT_DISCOVERY_TTL)
    }
}

impl DiscoveryCache {
    /// Build a fresh cache with the default TTL.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a fresh cache with a custom TTL. Tests use this to
    /// drive expiration without sleeping a full minute.
    #[must_use]
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
            ttl,
            metrics: Arc::new(DiscoveryCacheMetrics::default()),
        }
    }

    /// Borrow the metrics handle.
    #[must_use]
    pub fn metrics(&self) -> Arc<DiscoveryCacheMetrics> {
        Arc::clone(&self.metrics)
    }

    /// Look up a `(namespace, tenant)` entry. Returns `Some(card)` on
    /// a fresh hit; `None` on either a cold miss or an expired
    /// entry. Bumps the appropriate counter.
    pub async fn get(&self, namespace: &str, tenant: &str) -> Option<Arc<AgentCard>> {
        let key = (namespace.to_string(), tenant.to_string());
        let cache = self.inner.read().await;
        match cache.get(&key) {
            Some(entry) if entry.inserted_at.elapsed() < self.ttl => {
                self.metrics.hits.fetch_add(1, Ordering::Relaxed);
                Some(Arc::clone(&entry.card))
            }
            Some(_) => {
                self.metrics.expirations.fetch_add(1, Ordering::Relaxed);
                None
            }
            None => {
                self.metrics.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// Insert or replace an entry. Caller already paid for the
    /// underlying scan; this just memoizes the result.
    ///
    /// Also opportunistically prunes expired entries while we hold
    /// the write lock — keeps the cache size bounded even if no
    /// reader ever observes an expired entry.
    pub async fn insert(&self, namespace: &str, tenant: &str, card: AgentCard) {
        let key = (namespace.to_string(), tenant.to_string());
        let mut cache = self.inner.write().await;
        let now = Instant::now();
        cache.retain(|_, e| now.duration_since(e.inserted_at) < self.ttl);
        cache.insert(
            key,
            CacheEntry {
                inserted_at: now,
                card: Arc::new(card),
            },
        );
    }

    /// Drop the entry for `(namespace, tenant)`. Called from the
    /// agent-card mutation endpoints so a freshly-written card is
    /// visible to the next discovery call.
    pub async fn invalidate(&self, namespace: &str, tenant: &str) {
        let key = (namespace.to_string(), tenant.to_string());
        let mut cache = self.inner.write().await;
        if cache.remove(&key).is_some() {
            self.metrics.invalidations.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Test-only: return the current number of entries. Useful for
    /// asserting that `insert` actually pruned expired entries.
    #[cfg(test)]
    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_card(agent_id: &str) -> AgentCard {
        AgentCard::new(agent_id, "agents", "demo", "test", "1.0")
    }

    #[tokio::test]
    async fn cold_get_returns_none_and_counts_a_miss() {
        let c = DiscoveryCache::new();
        assert!(c.get("agents", "demo").await.is_none());
        let snap = c.metrics().snapshot();
        assert_eq!(snap.misses, 1);
        assert_eq!(snap.hits, 0);
    }

    #[tokio::test]
    async fn insert_then_get_returns_cached_card_and_counts_a_hit() {
        let c = DiscoveryCache::new();
        c.insert("agents", "demo", sample_card("a1")).await;
        let got = c.get("agents", "demo").await.expect("cache hit");
        assert_eq!(got.agent_id, "a1");
        let snap = c.metrics().snapshot();
        assert_eq!(snap.hits, 1);
        assert_eq!(snap.misses, 0);
    }

    #[tokio::test]
    async fn expired_entry_returns_none_and_counts_an_expiration() {
        // Use a 1ms TTL so we can deterministically observe
        // expiration without sleeping a meaningful duration.
        let c = DiscoveryCache::with_ttl(Duration::from_millis(1));
        c.insert("agents", "demo", sample_card("a1")).await;
        tokio::time::sleep(Duration::from_millis(15)).await;
        assert!(c.get("agents", "demo").await.is_none());
        let snap = c.metrics().snapshot();
        // The expired entry is `expirations`, not `misses` — the
        // distinction matters for tuning TTL vs. cache sizing.
        assert_eq!(snap.expirations, 1);
        assert_eq!(snap.misses, 0);
        assert_eq!(snap.hits, 0);
    }

    #[tokio::test]
    async fn invalidate_drops_the_entry_and_counts_an_invalidation() {
        let c = DiscoveryCache::new();
        c.insert("agents", "demo", sample_card("a1")).await;
        c.invalidate("agents", "demo").await;
        assert!(c.get("agents", "demo").await.is_none());
        let snap = c.metrics().snapshot();
        assert_eq!(snap.invalidations, 1);
        // The post-invalidate get is a cold miss, not an expiration.
        assert_eq!(snap.misses, 1);
        assert_eq!(snap.expirations, 0);
    }

    #[tokio::test]
    async fn invalidate_of_missing_entry_does_not_count() {
        let c = DiscoveryCache::new();
        c.invalidate("agents", "demo").await;
        // Nothing was removed, so the counter stays at 0 — keeps
        // the rate-of-actual-cache-invalidations metric honest.
        assert_eq!(c.metrics().snapshot().invalidations, 0);
    }

    #[tokio::test]
    async fn insert_prunes_expired_entries() {
        let c = DiscoveryCache::with_ttl(Duration::from_millis(1));
        // Insert two entries, let them age past the TTL, then
        // insert a third — the prune step inside `insert` must
        // collapse the cache to a single entry.
        c.insert("ns1", "tnt", sample_card("a")).await;
        c.insert("ns2", "tnt", sample_card("b")).await;
        tokio::time::sleep(Duration::from_millis(15)).await;
        c.insert("ns3", "tnt", sample_card("c")).await;
        assert_eq!(c.len().await, 1);
    }

    #[tokio::test]
    async fn isolated_tenants_do_not_share_entries() {
        let c = DiscoveryCache::new();
        c.insert("ns", "tnt-a", sample_card("a")).await;
        // `tnt-b` shares the namespace but is a distinct cache key.
        assert!(c.get("ns", "tnt-b").await.is_none());
        // `tnt-a` is still there.
        assert!(c.get("ns", "tnt-a").await.is_some());
    }
}
