use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tracing::warn;

use acteon_rules::EmbeddingEvalSupport;
use acteon_rules::error::RuleError;

use crate::cache::{CacheTier, EmbeddingCache};
use crate::config::EmbeddingBridgeConfig;
use crate::cosine::cosine_similarity;
use crate::metrics::EmbeddingMetrics;
use crate::provider::EmbeddingProvider;

/// Bridge between the `acteon-embedding` provider layer and the
/// `EmbeddingEvalSupport` trait expected by the rule engine.
///
/// It caches both topic and text embeddings and computes their cosine
/// similarity. When `fail_open` is enabled (the default), embedding errors
/// return similarity `0.0` instead of propagating to the caller — this
/// means `semantic_match` rules evaluate to `false` on API failure rather
/// than killing the entire dispatch pipeline.
pub struct EmbeddingBridge {
    topic_cache: EmbeddingCache,
    text_cache: EmbeddingCache,
    fail_open: bool,
    metrics: Arc<EmbeddingMetrics>,
}

impl std::fmt::Debug for EmbeddingBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbeddingBridge")
            .field("fail_open", &self.fail_open)
            .finish_non_exhaustive()
    }
}

impl EmbeddingBridge {
    /// Create a new bridge wrapping the given embedding provider.
    pub fn new(provider: Arc<dyn EmbeddingProvider>, config: EmbeddingBridgeConfig) -> Self {
        let metrics = Arc::new(EmbeddingMetrics::default());
        let topic_cache = EmbeddingCache::new(
            Arc::clone(&provider),
            config.topic_cache_capacity,
            Duration::from_secs(config.topic_cache_ttl_seconds),
            Arc::clone(&metrics),
            CacheTier::Topic,
        );
        let text_cache = EmbeddingCache::new(
            provider,
            config.text_cache_capacity,
            Duration::from_secs(config.text_cache_ttl_seconds),
            Arc::clone(&metrics),
            CacheTier::Text,
        );
        Self {
            topic_cache,
            text_cache,
            fail_open: config.fail_open,
            metrics,
        }
    }

    /// Return a shared handle to the embedding metrics counters.
    ///
    /// Use this to expose cache hit/miss rates and error counts to
    /// monitoring endpoints. The returned `Arc` can be stored separately
    /// from the bridge (e.g. in `AppState`) so metrics remain accessible
    /// even when the bridge is behind a trait object.
    pub fn metrics(&self) -> Arc<EmbeddingMetrics> {
        Arc::clone(&self.metrics)
    }

    /// Pre-warm the topic cache with the given topic strings.
    ///
    /// Call this after loading rules to avoid cold-start latency on the
    /// first requests. Errors for individual topics are logged and skipped.
    pub async fn warm_topics(&self, topics: &[&str]) {
        for topic in topics {
            if let Err(e) = self.topic_cache.get(topic).await {
                tracing::warn!(
                    topic = topic,
                    error = %e,
                    "failed to pre-warm topic embedding"
                );
            }
        }
    }

    /// Compute cosine similarity between a text and a topic embedding.
    async fn compute_similarity(&self, text: &str, topic: &str) -> Result<f64, RuleError> {
        let topic_vec = self
            .topic_cache
            .get(topic)
            .await
            .map_err(|e| RuleError::Evaluation(format!("embedding error (topic): {e}")))?;

        let text_vec = self
            .text_cache
            .get(text)
            .await
            .map_err(|e| RuleError::Evaluation(format!("embedding error (text): {e}")))?;

        Ok(f64::from(cosine_similarity(&text_vec, &topic_vec)))
    }
}

#[async_trait]
impl EmbeddingEvalSupport for EmbeddingBridge {
    async fn similarity(&self, text: &str, topic: &str) -> Result<f64, RuleError> {
        match self.compute_similarity(text, topic).await {
            Ok(sim) => Ok(sim),
            Err(e) if self.fail_open => {
                self.metrics.increment_errors();
                self.metrics.increment_fail_open();
                warn!(
                    text = text,
                    topic = topic,
                    error = %e,
                    "embedding similarity failed, returning 0.0 (fail-open)"
                );
                Ok(0.0)
            }
            Err(e) => {
                self.metrics.increment_errors();
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EmbeddingBridgeConfig;
    use crate::mock::{FailingEmbeddingProvider, MockEmbeddingProvider};

    #[tokio::test]
    async fn bridge_computes_similarity() {
        // Mock returns the same vector for all inputs -> similarity = 1.0
        let provider = Arc::new(MockEmbeddingProvider::new(vec![1.0, 0.0, 0.0]));
        let bridge = EmbeddingBridge::new(provider, EmbeddingBridgeConfig::default());

        let sim = bridge.similarity("hello", "world").await.unwrap();
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn bridge_caches_text_embeddings() {
        let provider = Arc::new(MockEmbeddingProvider::new(vec![1.0, 0.0]));
        let bridge =
            EmbeddingBridge::new(Arc::clone(&provider) as _, EmbeddingBridgeConfig::default());

        // Call twice with the same text + topic.
        bridge.similarity("hello", "world").await.unwrap();
        bridge.similarity("hello", "world").await.unwrap();

        // topic "world" = 1 call, text "hello" = 1 call -> 2 total (not 4).
        assert_eq!(provider.call_count(), 2);

        let snap = bridge.metrics().snapshot();
        assert_eq!(snap.topic_cache_misses, 1);
        assert_eq!(snap.topic_cache_hits, 1);
        assert_eq!(snap.text_cache_misses, 1);
        assert_eq!(snap.text_cache_hits, 1);
    }

    #[tokio::test]
    async fn fail_open_returns_zero_and_records_metrics() {
        let provider: Arc<dyn EmbeddingProvider> = Arc::new(FailingEmbeddingProvider);
        let config = EmbeddingBridgeConfig {
            fail_open: true,
            ..EmbeddingBridgeConfig::default()
        };
        let bridge = EmbeddingBridge::new(provider, config);

        let sim = bridge.similarity("hello", "world").await.unwrap();
        assert!((sim - 0.0).abs() < 1e-6);

        let snap = bridge.metrics().snapshot();
        assert_eq!(snap.errors, 1);
        assert_eq!(snap.fail_open_count, 1);
    }

    #[tokio::test]
    async fn fail_closed_propagates_error_and_records_metrics() {
        let provider: Arc<dyn EmbeddingProvider> = Arc::new(FailingEmbeddingProvider);
        let config = EmbeddingBridgeConfig {
            fail_open: false,
            ..EmbeddingBridgeConfig::default()
        };
        let bridge = EmbeddingBridge::new(provider, config);

        let result = bridge.similarity("hello", "world").await;
        assert!(result.is_err());

        let snap = bridge.metrics().snapshot();
        assert_eq!(snap.errors, 1);
        assert_eq!(snap.fail_open_count, 0);
    }
}
