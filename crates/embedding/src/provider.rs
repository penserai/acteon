use async_trait::async_trait;

use crate::error::EmbeddingError;

/// Trait for computing text embeddings.
///
/// Implementations call an external service (e.g., `OpenAI`) to convert text
/// into a dense vector representation.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed a single text string into a vector.
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError>;

    /// Embed multiple texts in a single batch.
    ///
    /// The default implementation calls [`embed`](Self::embed) sequentially.
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }
}
