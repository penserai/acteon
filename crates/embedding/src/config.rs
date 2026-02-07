use serde::Deserialize;

/// Configuration for an HTTP-based embedding provider.
#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddingConfig {
    /// The API endpoint (e.g., `https://api.openai.com/v1/embeddings`).
    pub endpoint: String,
    /// The model name (e.g., `text-embedding-3-small`).
    pub model: String,
    /// API key for authentication.
    pub api_key: String,
    /// Request timeout in seconds.
    pub timeout_seconds: u64,
}

/// Configuration for the [`EmbeddingBridge`](crate::EmbeddingBridge) caching
/// and resilience behaviour.
///
/// # Memory footprint
///
/// Each cached embedding is a `Vec<f32>` whose size depends on the model
/// dimension. For a 1536-dimensional model (`text-embedding-3-small`), one
/// vector is ~6 KB. With the defaults below the worst-case memory usage is:
///
/// - Topic cache: 10 000 x 6 KB = ~60 MB
/// - Text cache:   1 000 x 6 KB = ~6 MB
///
/// Adjust `topic_cache_capacity` and `text_cache_capacity` for your
/// environment. Monitor hit rates via the `/metrics` endpoint.
#[derive(Debug, Clone, Copy)]
pub struct EmbeddingBridgeConfig {
    /// Maximum number of topic embeddings to cache.
    pub topic_cache_capacity: u64,
    /// TTL in seconds for cached topic embeddings.
    pub topic_cache_ttl_seconds: u64,
    /// Maximum number of text embeddings to cache.
    pub text_cache_capacity: u64,
    /// TTL in seconds for cached text embeddings.
    pub text_cache_ttl_seconds: u64,
    /// Whether to fail open (return similarity `0.0`) on embedding errors.
    ///
    /// When `true` (the default), embedding API failures cause
    /// `semantic_match` rules to evaluate to `false` (no match) rather
    /// than killing the dispatch pipeline. This mirrors the `llm_fail_open`
    /// behaviour.
    pub fail_open: bool,
}

impl Default for EmbeddingBridgeConfig {
    fn default() -> Self {
        Self {
            topic_cache_capacity: 10_000,
            topic_cache_ttl_seconds: 3600,
            text_cache_capacity: 1_000,
            text_cache_ttl_seconds: 60,
            fail_open: true,
        }
    }
}
