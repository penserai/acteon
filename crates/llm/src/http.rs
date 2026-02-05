use std::fmt::Write;
use std::time::Duration;

use acteon_core::Action;
use async_trait::async_trait;
use serde_json::json;
use tracing::{debug, warn};

use crate::config::LlmGuardrailConfig;
use crate::error::LlmEvaluatorError;
use crate::evaluator::{LlmEvaluator, LlmGuardrailResponse};

/// HTTP-based LLM evaluator using an OpenAI-compatible chat completions API.
#[derive(Debug)]
pub struct HttpLlmEvaluator {
    client: reqwest::Client,
    config: LlmGuardrailConfig,
}

impl HttpLlmEvaluator {
    /// Create a new HTTP evaluator with the given configuration.
    pub fn new(config: LlmGuardrailConfig) -> Result<Self, LlmEvaluatorError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .build()
            .map_err(|e| LlmEvaluatorError::Configuration(e.to_string()))?;

        Ok(Self { client, config })
    }

    /// Build the user message summarising the action for the LLM.
    fn build_action_summary(action: &Action) -> String {
        let mut summary = format!(
            "Action type: {}\nProvider: {}\nNamespace: {}\nTenant: {}",
            action.action_type, action.provider, action.namespace, action.tenant,
        );

        if !action.payload.is_null() {
            let _ = write!(
                summary,
                "\nPayload: {}",
                serde_json::to_string_pretty(&action.payload).unwrap_or_default()
            );
        }

        if !action.metadata.labels.is_empty() {
            let _ = write!(summary, "\nLabels: {:?}", action.metadata.labels);
        }

        summary
    }

    /// Parse the LLM response, stripping markdown code fences if present.
    fn parse_response(content: &str) -> Result<LlmGuardrailResponse, LlmEvaluatorError> {
        let trimmed = content.trim();

        // Strip markdown code fences (```json ... ``` or ``` ... ```)
        let json_str = if trimmed.starts_with("```") {
            let without_opening = if let Some(rest) = trimmed.strip_prefix("```json") {
                rest
            } else {
                trimmed.strip_prefix("```").unwrap_or(trimmed)
            };
            without_opening
                .strip_suffix("```")
                .unwrap_or(without_opening)
                .trim()
        } else {
            trimmed
        };

        serde_json::from_str::<LlmGuardrailResponse>(json_str).map_err(|e| {
            LlmEvaluatorError::ParseError(format!(
                "failed to parse LLM response as JSON: {e}. Raw content: {content}"
            ))
        })
    }
}

#[async_trait]
impl LlmEvaluator for HttpLlmEvaluator {
    async fn evaluate(
        &self,
        action: &Action,
        policy: &str,
    ) -> Result<LlmGuardrailResponse, LlmEvaluatorError> {
        let action_summary = Self::build_action_summary(action);

        let request_body = json!({
            "model": self.config.model,
            "temperature": self.config.temperature,
            "max_tokens": self.config.max_tokens,
            "messages": [
                {
                    "role": "system",
                    "content": policy,
                },
                {
                    "role": "user",
                    "content": action_summary,
                }
            ]
        });

        debug!(endpoint = %self.config.endpoint, model = %self.config.model, "sending LLM guardrail request");

        let response = self
            .client
            .post(&self.config.endpoint)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    LlmEvaluatorError::Timeout(self.config.timeout_seconds)
                } else {
                    LlmEvaluatorError::HttpError(e.to_string())
                }
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            warn!(status = %status, "LLM API returned error");
            return Err(LlmEvaluatorError::ApiError(format!(
                "HTTP {status}: {body}"
            )));
        }

        let response_json: serde_json::Value = response.json().await.map_err(|e| {
            LlmEvaluatorError::ParseError(format!("failed to parse API response: {e}"))
        })?;

        // Extract the content from the OpenAI chat completions response format
        let content = response_json
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| {
                LlmEvaluatorError::ParseError(format!(
                    "unexpected response format: {response_json}"
                ))
            })?;

        Self::parse_response(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_json_response() {
        let content = r#"{"allowed": true, "reason": "Query is safe"}"#;
        let resp = HttpLlmEvaluator::parse_response(content).unwrap();
        assert!(resp.allowed);
        assert_eq!(resp.reason, "Query is safe");
    }

    #[test]
    fn parse_json_with_markdown_fences() {
        let content = "```json\n{\"allowed\": false, \"reason\": \"DROP TABLE detected\"}\n```";
        let resp = HttpLlmEvaluator::parse_response(content).unwrap();
        assert!(!resp.allowed);
        assert_eq!(resp.reason, "DROP TABLE detected");
    }

    #[test]
    fn parse_json_with_plain_fences() {
        let content = "```\n{\"allowed\": true, \"reason\": \"ok\"}\n```";
        let resp = HttpLlmEvaluator::parse_response(content).unwrap();
        assert!(resp.allowed);
    }

    #[test]
    fn parse_malformed_json_returns_error() {
        let content = "this is not json";
        let result = HttpLlmEvaluator::parse_response(content);
        assert!(result.is_err());
    }

    #[test]
    fn build_action_summary_includes_fields() {
        let action = Action::new(
            "ns",
            "tenant",
            "provider",
            "sql_query",
            serde_json::json!({"query": "SELECT * FROM users"}),
        );
        let summary = HttpLlmEvaluator::build_action_summary(&action);
        assert!(summary.contains("sql_query"));
        assert!(summary.contains("SELECT * FROM users"));
    }

    #[test]
    fn config_defaults() {
        let config = LlmGuardrailConfig::new(
            "http://localhost:8080/v1/chat/completions",
            "gpt-4o-mini",
            "sk-test",
        );
        assert_eq!(config.timeout_seconds, 10);
        assert_eq!(config.temperature, 0.0);
        assert_eq!(config.max_tokens, 256);
    }

    #[test]
    fn config_builder() {
        let config = LlmGuardrailConfig::new(
            "http://localhost:8080/v1/chat/completions",
            "gpt-4o-mini",
            "sk-test",
        )
        .with_timeout(30)
        .with_temperature(0.5)
        .with_max_tokens(512);
        assert_eq!(config.timeout_seconds, 30);
        assert_eq!(config.temperature, 0.5);
        assert_eq!(config.max_tokens, 512);
    }
}
