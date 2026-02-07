use std::sync::Arc;
use std::time::Duration;

use moka::future::Cache;

use crate::error::EmbeddingError;
use crate::metrics::EmbeddingMetrics;
use crate::provider::EmbeddingProvider;

/// Which cache tier this instance represents (used to increment the correct
/// metric counters).
#[derive(Debug, Clone, Copy)]
pub enum CacheTier {
    /// Topic embeddings (long TTL, large capacity).
    Topic,
    /// Text embeddings (short TTL, small capacity).
    Text,
}

/// A bounded, TTL-based embedding cache backed by [`moka`].
///
/// Uses `try_get_with` to coalesce concurrent requests for the same key
/// (thundering herd protection). Works for both topic embeddings (long TTL,
/// large capacity) and text embeddings (short TTL, small capacity).
pub struct EmbeddingCache {
    provider: Arc<dyn EmbeddingProvider>,
    cache: Cache<String, Vec<f32>>,
    metrics: Arc<EmbeddingMetrics>,
    tier: CacheTier,
}

impl EmbeddingCache {
    /// Create a new cache backed by the given embedding provider.
    pub fn new(
        provider: Arc<dyn EmbeddingProvider>,
        max_capacity: u64,
        ttl: Duration,
        metrics: Arc<EmbeddingMetrics>,
        tier: CacheTier,
    ) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_capacity)
            .time_to_live(ttl)
            .build();
        Self {
            provider,
            cache,
            metrics,
            tier,
        }
    }

    /// Get the embedding for a key, computing it via the provider on cache miss.
    ///
    /// Concurrent callers for the same key will coalesce into a single provider
    /// call. Hit/miss counters are approximate under high concurrency (a small
    /// number of concurrent requests for the same uncached key may all count as
    /// misses even though only one provider call is made).
    pub async fn get(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        // Check cache first for metrics tracking. This is racy with
        // try_get_with but acceptable for operational counters.
        if let Some(val) = self.cache.get(text).await {
            self.record_hit();
            return Ok(val);
        }

        self.record_miss();
        let provider = Arc::clone(&self.provider);
        let key = text.to_owned();
        self.cache
            .try_get_with(key, async move { provider.embed(text).await })
            .await
            .map_err(|e| EmbeddingError::ApiError(e.to_string()))
    }

    fn record_hit(&self) {
        match self.tier {
            CacheTier::Topic => self.metrics.increment_topic_hit(),
            CacheTier::Text => self.metrics.increment_text_hit(),
        }
    }

    fn record_miss(&self) {
        match self.tier {
            CacheTier::Topic => self.metrics.increment_topic_miss(),
            CacheTier::Text => self.metrics.increment_text_miss(),
        }
    }
}

impl std::fmt::Debug for EmbeddingCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbeddingCache")
            .field("tier", &self.tier)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockEmbeddingProvider;

    fn test_metrics() -> Arc<EmbeddingMetrics> {
        Arc::new(EmbeddingMetrics::default())
    }

    #[tokio::test]
    async fn caches_embeddings() {
        let metrics = test_metrics();
        let provider = Arc::new(MockEmbeddingProvider::new(vec![0.1, 0.2, 0.3]));
        let cache = EmbeddingCache::new(
            Arc::clone(&provider) as _,
            100,
            Duration::from_secs(60),
            Arc::clone(&metrics),
            CacheTier::Topic,
        );

        let first = cache.get("test topic").await.unwrap();
        let second = cache.get("test topic").await.unwrap();
        assert_eq!(first, second);
        assert_eq!(provider.call_count(), 1);

        let snap = metrics.snapshot();
        assert_eq!(snap.topic_cache_misses, 1);
        assert_eq!(snap.topic_cache_hits, 1);
    }

    #[tokio::test]
    async fn different_keys_call_provider() {
        let metrics = test_metrics();
        let provider = Arc::new(MockEmbeddingProvider::new(vec![1.0]));
        let cache = EmbeddingCache::new(
            Arc::clone(&provider) as _,
            100,
            Duration::from_secs(60),
            Arc::clone(&metrics),
            CacheTier::Text,
        );

        cache.get("a").await.unwrap();
        cache.get("b").await.unwrap();
        assert_eq!(provider.call_count(), 2);

        let snap = metrics.snapshot();
        assert_eq!(snap.text_cache_misses, 2);
        assert_eq!(snap.text_cache_hits, 0);
    }
}
