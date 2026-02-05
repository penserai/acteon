use acteon_core::Action;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::LlmEvaluatorError;

/// Response from an LLM guardrail evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmGuardrailResponse {
    /// Whether the action is allowed.
    pub allowed: bool,
    /// Explanation of the decision.
    pub reason: String,
}

/// Trait for evaluating actions against an LLM-based policy.
#[async_trait]
pub trait LlmEvaluator: Send + Sync + std::fmt::Debug {
    /// Evaluate whether the given action is allowed under the policy.
    async fn evaluate(
        &self,
        action: &Action,
        policy: &str,
    ) -> Result<LlmGuardrailResponse, LlmEvaluatorError>;
}
