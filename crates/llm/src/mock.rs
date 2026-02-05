use std::sync::Mutex;

use acteon_core::Action;
use async_trait::async_trait;

use crate::error::LlmEvaluatorError;
use crate::evaluator::{LlmEvaluator, LlmGuardrailResponse};

/// A mock LLM evaluator that returns a configurable response.
#[derive(Debug, Clone)]
pub struct MockLlmEvaluator {
    response: LlmGuardrailResponse,
}

impl MockLlmEvaluator {
    /// Create a mock that always allows actions.
    pub fn allowing() -> Self {
        Self {
            response: LlmGuardrailResponse {
                allowed: true,
                reason: "allowed by mock".into(),
            },
        }
    }

    /// Create a mock that always denies actions.
    pub fn denying(reason: impl Into<String>) -> Self {
        Self {
            response: LlmGuardrailResponse {
                allowed: false,
                reason: reason.into(),
            },
        }
    }

    /// Create a mock with a custom response.
    pub fn with_response(response: LlmGuardrailResponse) -> Self {
        Self { response }
    }
}

#[async_trait]
impl LlmEvaluator for MockLlmEvaluator {
    async fn evaluate(
        &self,
        _action: &Action,
        _policy: &str,
    ) -> Result<LlmGuardrailResponse, LlmEvaluatorError> {
        Ok(self.response.clone())
    }
}

/// A mock LLM evaluator that always returns an error.
#[derive(Debug, Clone)]
pub struct FailingLlmEvaluator {
    error_message: String,
}

impl FailingLlmEvaluator {
    /// Create a failing evaluator with the given error message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            error_message: message.into(),
        }
    }
}

#[async_trait]
impl LlmEvaluator for FailingLlmEvaluator {
    async fn evaluate(
        &self,
        _action: &Action,
        _policy: &str,
    ) -> Result<LlmGuardrailResponse, LlmEvaluatorError> {
        Err(LlmEvaluatorError::ApiError(self.error_message.clone()))
    }
}

/// A mock LLM evaluator that captures the policy string passed to it.
///
/// Always allows actions. Use [`captured_policies`](Self::captured_policies) to
/// inspect which policy strings were received.
#[derive(Debug)]
pub struct CapturingLlmEvaluator {
    policies: Mutex<Vec<String>>,
}

impl CapturingLlmEvaluator {
    /// Create a new capturing evaluator.
    pub fn new() -> Self {
        Self {
            policies: Mutex::new(Vec::new()),
        }
    }

    /// Return all policy strings that were passed to [`evaluate`](LlmEvaluator::evaluate).
    pub fn captured_policies(&self) -> Vec<String> {
        self.policies.lock().unwrap().clone()
    }
}

impl Default for CapturingLlmEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LlmEvaluator for CapturingLlmEvaluator {
    async fn evaluate(
        &self,
        _action: &Action,
        policy: &str,
    ) -> Result<LlmGuardrailResponse, LlmEvaluatorError> {
        self.policies.lock().unwrap().push(policy.to_owned());
        Ok(LlmGuardrailResponse {
            allowed: true,
            reason: "allowed by capturing mock".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_action() -> Action {
        Action::new(
            "ns",
            "tenant",
            "provider",
            "test_action",
            serde_json::json!({}),
        )
    }

    #[tokio::test]
    async fn mock_allowing() {
        let evaluator = MockLlmEvaluator::allowing();
        let result = evaluator.evaluate(&test_action(), "policy").await.unwrap();
        assert!(result.allowed);
    }

    #[tokio::test]
    async fn mock_denying() {
        let evaluator = MockLlmEvaluator::denying("unsafe query");
        let result = evaluator.evaluate(&test_action(), "policy").await.unwrap();
        assert!(!result.allowed);
        assert_eq!(result.reason, "unsafe query");
    }

    #[tokio::test]
    async fn failing_evaluator() {
        let evaluator = FailingLlmEvaluator::new("service unavailable");
        let result = evaluator.evaluate(&test_action(), "policy").await;
        assert!(result.is_err());
    }
}
