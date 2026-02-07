use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;

use crate::error::EmbeddingError;
use crate::provider::EmbeddingProvider;

/// A mock embedding provider that always returns the same fixed vector.
///
/// Tracks the number of calls via an atomic counter so tests can verify
/// caching behaviour.
pub struct MockEmbeddingProvider {
    vector: Vec<f32>,
    calls: AtomicUsize,
}

impl MockEmbeddingProvider {
    /// Create a mock provider returning the given fixed vector.
    pub fn new(vector: Vec<f32>) -> Self {
        Self {
            vector,
            calls: AtomicUsize::new(0),
        }
    }

    /// Number of times [`embed`](EmbeddingProvider::embed) was called.
    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl EmbeddingProvider for MockEmbeddingProvider {
    async fn embed(&self, _text: &str) -> Result<Vec<f32>, EmbeddingError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(self.vector.clone())
    }
}

/// A mock embedding provider that maps specific text to specific vectors.
///
/// Unknown texts receive a zero vector of the configured dimension.
pub struct MappingEmbeddingProvider {
    mappings: HashMap<String, Vec<f32>>,
    dimension: usize,
}

impl MappingEmbeddingProvider {
    /// Create a mapping provider with the given text-to-vector mappings.
    pub fn new(mappings: HashMap<String, Vec<f32>>, dimension: usize) -> Self {
        Self {
            mappings,
            dimension,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for MappingEmbeddingProvider {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        Ok(self
            .mappings
            .get(text)
            .cloned()
            .unwrap_or_else(|| vec![0.0; self.dimension]))
    }
}

/// A mock embedding provider that always returns an error.
pub struct FailingEmbeddingProvider;

#[async_trait]
impl EmbeddingProvider for FailingEmbeddingProvider {
    async fn embed(&self, _text: &str) -> Result<Vec<f32>, EmbeddingError> {
        Err(EmbeddingError::ApiError("mock failure".to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_returns_fixed_vector() {
        let provider = MockEmbeddingProvider::new(vec![1.0, 2.0, 3.0]);
        let result = provider.embed("anything").await.unwrap();
        assert_eq!(result, vec![1.0, 2.0, 3.0]);
    }

    #[tokio::test]
    async fn mock_tracks_call_count() {
        let provider = MockEmbeddingProvider::new(vec![1.0]);
        assert_eq!(provider.call_count(), 0);
        provider.embed("a").await.unwrap();
        provider.embed("b").await.unwrap();
        assert_eq!(provider.call_count(), 2);
    }

    #[tokio::test]
    async fn mapping_returns_known_vector() {
        let mut mappings = HashMap::new();
        mappings.insert("hello".to_owned(), vec![0.5, 0.5]);
        let provider = MappingEmbeddingProvider::new(mappings, 2);

        let result = provider.embed("hello").await.unwrap();
        assert_eq!(result, vec![0.5, 0.5]);

        let result = provider.embed("unknown").await.unwrap();
        assert_eq!(result, vec![0.0, 0.0]);
    }

    #[tokio::test]
    async fn failing_always_errors() {
        let provider = FailingEmbeddingProvider;
        assert!(provider.embed("anything").await.is_err());
    }
}
