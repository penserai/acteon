use std::sync::atomic::{AtomicU64, Ordering};

/// Atomic counters tracking embedding cache and provider behaviour.
///
/// All counters use relaxed ordering for maximum throughput. For a
/// consistent point-in-time view, call [`snapshot`](Self::snapshot).
#[derive(Debug, Default)]
pub struct EmbeddingMetrics {
    /// Topic cache hits (embedding already cached).
    pub topic_cache_hits: AtomicU64,
    /// Topic cache misses (required provider call).
    pub topic_cache_misses: AtomicU64,
    /// Text cache hits.
    pub text_cache_hits: AtomicU64,
    /// Text cache misses.
    pub text_cache_misses: AtomicU64,
    /// Total embedding provider errors.
    pub errors: AtomicU64,
    /// Times fail-open returned `0.0` instead of propagating an error.
    pub fail_open_count: AtomicU64,
}

impl EmbeddingMetrics {
    /// Increment the topic cache hit counter.
    pub fn increment_topic_hit(&self) {
        self.topic_cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the topic cache miss counter.
    pub fn increment_topic_miss(&self) {
        self.topic_cache_misses.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the text cache hit counter.
    pub fn increment_text_hit(&self) {
        self.text_cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the text cache miss counter.
    pub fn increment_text_miss(&self) {
        self.text_cache_misses.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the error counter.
    pub fn increment_errors(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the fail-open activation counter.
    pub fn increment_fail_open(&self) {
        self.fail_open_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Take a consistent point-in-time snapshot of all counters.
    pub fn snapshot(&self) -> EmbeddingMetricsSnapshot {
        EmbeddingMetricsSnapshot {
            topic_cache_hits: self.topic_cache_hits.load(Ordering::Relaxed),
            topic_cache_misses: self.topic_cache_misses.load(Ordering::Relaxed),
            text_cache_hits: self.text_cache_hits.load(Ordering::Relaxed),
            text_cache_misses: self.text_cache_misses.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            fail_open_count: self.fail_open_count.load(Ordering::Relaxed),
        }
    }
}

/// A plain data snapshot of [`EmbeddingMetrics`] at a point in time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingMetricsSnapshot {
    /// Topic cache hits.
    pub topic_cache_hits: u64,
    /// Topic cache misses.
    pub topic_cache_misses: u64,
    /// Text cache hits.
    pub text_cache_hits: u64,
    /// Text cache misses.
    pub text_cache_misses: u64,
    /// Total embedding provider errors.
    pub errors: u64,
    /// Times fail-open returned `0.0` instead of propagating an error.
    pub fail_open_count: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_metrics_are_zero() {
        let m = EmbeddingMetrics::default();
        let snap = m.snapshot();
        assert_eq!(snap.topic_cache_hits, 0);
        assert_eq!(snap.topic_cache_misses, 0);
        assert_eq!(snap.text_cache_hits, 0);
        assert_eq!(snap.text_cache_misses, 0);
        assert_eq!(snap.errors, 0);
        assert_eq!(snap.fail_open_count, 0);
    }

    #[test]
    fn increment_and_snapshot() {
        let m = EmbeddingMetrics::default();
        m.increment_topic_hit();
        m.increment_topic_hit();
        m.increment_topic_miss();
        m.increment_text_hit();
        m.increment_text_miss();
        m.increment_text_miss();
        m.increment_errors();
        m.increment_fail_open();

        let snap = m.snapshot();
        assert_eq!(snap.topic_cache_hits, 2);
        assert_eq!(snap.topic_cache_misses, 1);
        assert_eq!(snap.text_cache_hits, 1);
        assert_eq!(snap.text_cache_misses, 2);
        assert_eq!(snap.errors, 1);
        assert_eq!(snap.fail_open_count, 1);
    }
}
