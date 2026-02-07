use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use acteon_core::Action;
use acteon_state::StateStore;

use crate::error::RuleError;

/// Trait for providing embedding-based similarity evaluation.
///
/// Implementations compute the cosine similarity between a text string and a
/// topic description using vector embeddings. The trait is defined in
/// `acteon-rules` so the rule engine can call it without depending on any
/// specific embedding provider.
#[async_trait]
pub trait EmbeddingEvalSupport: Send + Sync + std::fmt::Debug {
    /// Compute the similarity between `text` and `topic`.
    ///
    /// Returns a value in `[0.0, 1.0]` where higher means more similar.
    async fn similarity(&self, text: &str, topic: &str) -> Result<f64, RuleError>;
}

/// The evaluation context supplied to the rule engine when evaluating expressions.
///
/// It provides access to the action being evaluated, the state store for
/// stateful lookups (counters, dedup, etc.), environment variables, and the
/// current timestamp.
pub struct EvalContext<'a> {
    /// The action being evaluated.
    pub action: &'a Action,
    /// The state store for stateful rule conditions.
    pub state: &'a dyn StateStore,
    /// Environment variables and external configuration.
    pub environment: &'a HashMap<String, String>,
    /// The current timestamp for time-based evaluations.
    pub now: DateTime<Utc>,
    /// Optional embedding support for semantic matching.
    pub embedding: Option<Arc<dyn EmbeddingEvalSupport>>,
}

impl<'a> EvalContext<'a> {
    /// Create a new evaluation context.
    pub fn new(
        action: &'a Action,
        state: &'a dyn StateStore,
        environment: &'a HashMap<String, String>,
    ) -> Self {
        Self {
            action,
            state,
            environment,
            now: Utc::now(),
            embedding: None,
        }
    }

    /// Create a new evaluation context with a specific timestamp.
    #[must_use]
    pub fn with_now(mut self, now: DateTime<Utc>) -> Self {
        self.now = now;
        self
    }

    /// Set the embedding support for semantic matching.
    #[must_use]
    pub fn with_embedding(mut self, embedding: Arc<dyn EmbeddingEvalSupport>) -> Self {
        self.embedding = Some(embedding);
        self
    }
}
