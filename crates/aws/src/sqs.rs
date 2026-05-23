use std::collections::HashMap;

use acteon_core::{Action, ProviderResponse};
use acteon_provider::ProviderError;
use acteon_provider::provider::Provider;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, instrument};

use crate::auth::build_sdk_config;
use crate::config::AwsBaseConfig;
use crate::error::classify_sdk_error;

/// Configuration for the AWS SQS provider.
#[derive(Clone, Serialize, Deserialize)]
pub struct SqsConfig {
    /// Shared AWS configuration (region, role ARN, endpoint URL).
    #[serde(flatten)]
    pub aws: AwsBaseConfig,

    /// Default SQS queue URL. Can be overridden per-action in the payload.
    pub queue_url: Option<String>,
}

impl std::fmt::Debug for SqsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqsConfig")
            .field("aws", &self.aws)
            .field("queue_url", &self.queue_url)
            .finish()
    }
}

impl SqsConfig {
    /// Create a new `SqsConfig` with the given AWS region.
    pub fn new(region: impl Into<String>) -> Self {
        Self {
            aws: AwsBaseConfig::new(region),
            queue_url: None,
        }
    }

    /// Set the default queue URL.
    #[must_use]
    pub fn with_queue_url(mut self, queue_url: impl Into<String>) -> Self {
        self.queue_url = Some(queue_url.into());
        self
    }

    /// Set the endpoint URL override (for `LocalStack`).
    #[must_use]
    pub fn with_endpoint_url(mut self, endpoint_url: impl Into<String>) -> Self {
        self.aws.endpoint_url = Some(endpoint_url.into());
        self
    }

    /// Set the IAM role ARN to assume.
    #[must_use]
    pub fn with_role_arn(mut self, role_arn: impl Into<String>) -> Self {
        self.aws.role_arn = Some(role_arn.into());
        self
    }

    /// Set the STS session name for assume-role.
    #[must_use]
    pub fn with_session_name(mut self, session_name: impl Into<String>) -> Self {
        self.aws.session_name = Some(session_name.into());
        self
    }

    /// Set the external ID for cross-account trust policies.
    #[must_use]
    pub fn with_external_id(mut self, external_id: impl Into<String>) -> Self {
        self.aws.external_id = Some(external_id.into());
        self
    }
}

/// Payload for the `send_message` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqsSendPayload {
    /// SQS queue URL. Overrides config default.
    pub queue_url: Option<String>,

    /// Message body (string or JSON that gets serialized to string).
    pub message_body: String,

    /// Optional delay in seconds before the message becomes visible (0-900).
    pub delay_seconds: Option<i32>,

    /// Optional message group ID (required for FIFO queues).
    pub message_group_id: Option<String>,

    /// Optional message deduplication ID (for FIFO queues).
    pub message_dedup_id: Option<String>,

    /// Optional string message attributes.
    #[serde(default)]
    pub message_attributes: HashMap<String, String>,
}

/// AWS SQS provider for sending messages to SQS queues.
pub struct SqsProvider {
    config: SqsConfig,
    client: aws_sdk_sqs::Client,
}

impl std::fmt::Debug for SqsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqsProvider")
            .field("config", &self.config)
            .field("client", &"<SqsClient>")
            .finish()
    }
}

impl SqsProvider {
    /// Create a new `SqsProvider` by building an AWS SDK client.
    pub async fn new(config: SqsConfig) -> Self {
        let sdk_config = build_sdk_config(&config.aws).await;
        let client = aws_sdk_sqs::Client::new(&sdk_config);
        Self { config, client }
    }

    /// Create an `SqsProvider` with a pre-built client (for testing).
    pub fn with_client(config: SqsConfig, client: aws_sdk_sqs::Client) -> Self {
        Self { config, client }
    }
}

impl Provider for SqsProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "aws-sqs"
    }

    #[instrument(skip(self, action), fields(action_id = %action.id, provider = "aws-sqs"))]
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing SQS payload");
        let payload: SqsSendPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let queue_url = payload
            .queue_url
            .as_deref()
            .or(self.config.queue_url.as_deref())
            .ok_or_else(|| {
                ProviderError::Configuration(
                    "no queue_url in payload or provider config".to_owned(),
                )
            })?;

        debug!(queue_url = %queue_url, "sending message to SQS queue");

        let mut request = self
            .client
            .send_message()
            .queue_url(queue_url)
            .message_body(&payload.message_body);

        if let Some(delay) = payload.delay_seconds {
            request = request.delay_seconds(delay);
        }
        if let Some(ref group_id) = payload.message_group_id {
            request = request.message_group_id(group_id);
        }
        if let Some(ref dedup_id) = payload.message_dedup_id {
            request = request.message_deduplication_id(dedup_id);
        }

        for (key, value) in &payload.message_attributes {
            let attr = aws_sdk_sqs::types::MessageAttributeValue::builder()
                .data_type("String")
                .string_value(value)
                .build()
                .map_err(|e| ProviderError::Serialization(e.to_string()))?;
            request = request.message_attributes(key, attr);
        }

        let result = request.send().await.map_err(|e| {
            let err_str = e.to_string();
            error!(error = %err_str, "SQS send_message failed");
            let aws_err: ProviderError = classify_sdk_error(&err_str).into();
            aws_err
        })?;

        let message_id = result.message_id().unwrap_or("unknown").to_owned();
        info!(message_id = %message_id, queue_url = %queue_url, "SQS message sent");

        Ok(ProviderResponse::success(serde_json::json!({
            "message_id": message_id,
            "queue_url": queue_url,
            "status": "sent"
        })))
    }

    #[instrument(skip(self), fields(provider = "aws-sqs"))]
    async fn health_check(&self) -> Result<(), ProviderError> {
        debug!("performing SQS health check");
        self.client
            .list_queues()
            .max_results(1)
            .send()
            .await
            .map_err(|e| {
                error!(error = %e, "SQS health check failed");
                ProviderError::Connection(format!("SQS health check failed: {e}"))
            })?;
        info!("SQS health check passed");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_new_sets_region() {
        let config = SqsConfig::new("us-west-2");
        assert_eq!(config.aws.region, "us-west-2");
        assert!(config.queue_url.is_none());
    }

    #[test]
    fn config_with_queue_url() {
        let config = SqsConfig::new("us-east-1")
            .with_queue_url("https://sqs.us-east-1.amazonaws.com/123456789012/my-queue");
        assert!(config.queue_url.is_some());
    }

    #[test]
    fn config_debug_format() {
        let config = SqsConfig::new("us-east-1").with_role_arn("arn:aws:iam::123:role/test");
        let debug = format!("{config:?}");
        assert!(debug.contains("SqsConfig"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn config_serde_roundtrip() {
        let config =
            SqsConfig::new("eu-west-1").with_queue_url("https://sqs.eu-west-1.amazonaws.com/123/q");

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: SqsConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.aws.region, "eu-west-1");
        assert!(deserialized.queue_url.is_some());
    }

    #[test]
    fn deserialize_send_payload() {
        let json = serde_json::json!({
            "message_body": "Hello, SQS!",
            "delay_seconds": 10,
            "queue_url": "https://sqs.us-east-1.amazonaws.com/123/q"
        });
        let payload: SqsSendPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.message_body, "Hello, SQS!");
        assert_eq!(payload.delay_seconds, Some(10));
        assert!(payload.queue_url.is_some());
    }

    #[test]
    fn deserialize_minimal_send_payload() {
        let json = serde_json::json!({
            "message_body": "test"
        });
        let payload: SqsSendPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.message_body, "test");
        assert!(payload.queue_url.is_none());
        assert!(payload.delay_seconds.is_none());
        assert!(payload.message_group_id.is_none());
        assert!(payload.message_attributes.is_empty());
    }

    #[test]
    fn deserialize_fifo_send_payload() {
        let json = serde_json::json!({
            "message_body": "ordered",
            "message_group_id": "group-1",
            "message_dedup_id": "dedup-abc",
            "message_attributes": {
                "env": "production"
            }
        });
        let payload: SqsSendPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.message_group_id.as_deref(), Some("group-1"));
        assert_eq!(payload.message_dedup_id.as_deref(), Some("dedup-abc"));
        assert_eq!(payload.message_attributes.get("env").unwrap(), "production");
    }
}
