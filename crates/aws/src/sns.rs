use acteon_core::{Action, ProviderResponse};
use acteon_provider::ProviderError;
use acteon_provider::provider::Provider;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, instrument};

use crate::auth::build_sdk_config;
use crate::config::AwsBaseConfig;
use crate::error::classify_sdk_error;

/// Configuration for the AWS SNS provider.
#[derive(Clone, Serialize, Deserialize)]
pub struct SnsConfig {
    /// Shared AWS configuration (region, role ARN, endpoint URL).
    #[serde(flatten)]
    pub aws: AwsBaseConfig,

    /// Default SNS topic ARN. Can be overridden per-action in the payload.
    pub topic_arn: Option<String>,
}

impl std::fmt::Debug for SnsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnsConfig")
            .field("aws", &self.aws)
            .field("topic_arn", &self.topic_arn)
            .finish()
    }
}

impl SnsConfig {
    /// Create a new `SnsConfig` with the given AWS region.
    pub fn new(region: impl Into<String>) -> Self {
        Self {
            aws: AwsBaseConfig::new(region),
            topic_arn: None,
        }
    }

    /// Set the default topic ARN.
    #[must_use]
    pub fn with_topic_arn(mut self, topic_arn: impl Into<String>) -> Self {
        self.topic_arn = Some(topic_arn.into());
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
}

/// Payload for the `publish_message` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnsPublishPayload {
    /// SNS topic ARN. Overrides the default topic ARN from config.
    pub topic_arn: Option<String>,

    /// Message body.
    pub message: String,

    /// Optional message subject (used for email endpoints).
    pub subject: Option<String>,

    /// Optional message group ID (for FIFO topics).
    pub message_group_id: Option<String>,

    /// Optional message deduplication ID (for FIFO topics).
    pub message_dedup_id: Option<String>,
}

/// AWS SNS provider for publishing messages to SNS topics.
pub struct SnsProvider {
    config: SnsConfig,
    client: aws_sdk_sns::Client,
}

impl std::fmt::Debug for SnsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnsProvider")
            .field("config", &self.config)
            .field("client", &"<SnsClient>")
            .finish()
    }
}

impl SnsProvider {
    /// Create a new `SnsProvider` by building an AWS SDK client.
    pub async fn new(config: SnsConfig) -> Self {
        let sdk_config = build_sdk_config(&config.aws).await;
        let client = aws_sdk_sns::Client::new(&sdk_config);
        Self { config, client }
    }

    /// Create an `SnsProvider` with a pre-built client (for testing).
    pub fn with_client(config: SnsConfig, client: aws_sdk_sns::Client) -> Self {
        Self { config, client }
    }
}

impl Provider for SnsProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "aws-sns"
    }

    #[instrument(skip(self, action), fields(action_id = %action.id, provider = "aws-sns"))]
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing SNS payload");
        let payload: SnsPublishPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let topic_arn = payload
            .topic_arn
            .as_deref()
            .or(self.config.topic_arn.as_deref())
            .ok_or_else(|| {
                ProviderError::Configuration(
                    "no topic_arn in payload or provider config".to_owned(),
                )
            })?;

        debug!(topic_arn = %topic_arn, "publishing to SNS topic");

        let mut request = self
            .client
            .publish()
            .topic_arn(topic_arn)
            .message(&payload.message);

        if let Some(ref subject) = payload.subject {
            request = request.subject(subject);
        }
        if let Some(ref group_id) = payload.message_group_id {
            request = request.message_group_id(group_id);
        }
        if let Some(ref dedup_id) = payload.message_dedup_id {
            request = request.message_deduplication_id(dedup_id);
        }

        let result = request.send().await.map_err(|e| {
            let err_str = e.to_string();
            error!(error = %err_str, "SNS publish failed");
            let aws_err: ProviderError = classify_sdk_error(&err_str).into();
            aws_err
        })?;

        let message_id = result.message_id().unwrap_or("unknown").to_owned();
        info!(message_id = %message_id, topic_arn = %topic_arn, "SNS message published");

        Ok(ProviderResponse::success(serde_json::json!({
            "message_id": message_id,
            "topic_arn": topic_arn,
            "status": "published"
        })))
    }

    #[instrument(skip(self), fields(provider = "aws-sns"))]
    async fn health_check(&self) -> Result<(), ProviderError> {
        debug!("performing SNS health check");
        // List topics with a limit of 1 to verify connectivity.
        self.client.list_topics().send().await.map_err(|e| {
            error!(error = %e, "SNS health check failed");
            ProviderError::Connection(format!("SNS health check failed: {e}"))
        })?;
        info!("SNS health check passed");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_new_sets_region() {
        let config = SnsConfig::new("eu-west-1");
        assert_eq!(config.aws.region, "eu-west-1");
        assert!(config.topic_arn.is_none());
    }

    #[test]
    fn config_with_topic_arn() {
        let config = SnsConfig::new("us-east-1")
            .with_topic_arn("arn:aws:sns:us-east-1:123456789012:my-topic");
        assert_eq!(
            config.topic_arn.as_deref(),
            Some("arn:aws:sns:us-east-1:123456789012:my-topic")
        );
    }

    #[test]
    fn config_with_endpoint_url() {
        let config = SnsConfig::new("us-east-1").with_endpoint_url("http://localhost:4566");
        assert_eq!(
            config.aws.endpoint_url.as_deref(),
            Some("http://localhost:4566")
        );
    }

    #[test]
    fn config_debug_format() {
        let config = SnsConfig::new("us-east-1")
            .with_topic_arn("arn:aws:sns:us-east-1:123:topic")
            .with_role_arn("arn:aws:iam::123:role/test");
        let debug = format!("{config:?}");
        assert!(debug.contains("SnsConfig"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn config_serde_roundtrip() {
        let config =
            SnsConfig::new("ap-southeast-1").with_topic_arn("arn:aws:sns:ap-southeast-1:123:topic");

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: SnsConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.aws.region, "ap-southeast-1");
        assert_eq!(
            deserialized.topic_arn.as_deref(),
            Some("arn:aws:sns:ap-southeast-1:123:topic")
        );
    }

    #[test]
    fn deserialize_publish_payload() {
        let json = serde_json::json!({
            "message": "Hello from Acteon",
            "subject": "Alert",
            "topic_arn": "arn:aws:sns:us-east-1:123:topic"
        });
        let payload: SnsPublishPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.message, "Hello from Acteon");
        assert_eq!(payload.subject.as_deref(), Some("Alert"));
        assert_eq!(
            payload.topic_arn.as_deref(),
            Some("arn:aws:sns:us-east-1:123:topic")
        );
    }

    #[test]
    fn deserialize_minimal_publish_payload() {
        let json = serde_json::json!({
            "message": "test"
        });
        let payload: SnsPublishPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.message, "test");
        assert!(payload.topic_arn.is_none());
        assert!(payload.subject.is_none());
        assert!(payload.message_group_id.is_none());
    }

    #[test]
    fn deserialize_fifo_publish_payload() {
        let json = serde_json::json!({
            "message": "ordered message",
            "topic_arn": "arn:aws:sns:us-east-1:123:topic.fifo",
            "message_group_id": "group-1",
            "message_dedup_id": "dedup-abc"
        });
        let payload: SnsPublishPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.message_group_id.as_deref(), Some("group-1"));
        assert_eq!(payload.message_dedup_id.as_deref(), Some("dedup-abc"));
    }
}
