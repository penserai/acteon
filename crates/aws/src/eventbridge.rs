use acteon_core::{Action, ProviderResponse};
use acteon_provider::ProviderError;
use acteon_provider::provider::Provider;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, instrument};

use crate::auth::build_sdk_config;
use crate::config::AwsBaseConfig;
use crate::error::classify_sdk_error;

/// Configuration for the AWS `EventBridge` provider.
#[derive(Clone, Serialize, Deserialize)]
pub struct EventBridgeConfig {
    /// Shared AWS configuration (region, role ARN, endpoint URL).
    #[serde(flatten)]
    pub aws: AwsBaseConfig,

    /// Default event bus name. Defaults to `"default"` if not set.
    pub event_bus_name: Option<String>,
}

impl std::fmt::Debug for EventBridgeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBridgeConfig")
            .field("aws", &self.aws)
            .field("event_bus_name", &self.event_bus_name)
            .finish()
    }
}

impl EventBridgeConfig {
    /// Create a new `EventBridgeConfig` with the given AWS region.
    pub fn new(region: impl Into<String>) -> Self {
        Self {
            aws: AwsBaseConfig::new(region),
            event_bus_name: None,
        }
    }

    /// Set the default event bus name.
    #[must_use]
    pub fn with_event_bus_name(mut self, name: impl Into<String>) -> Self {
        self.event_bus_name = Some(name.into());
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

/// Payload for the `put_events` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventBridgePutPayload {
    /// Event bus name. Overrides config default.
    pub event_bus_name: Option<String>,

    /// Event source (e.g. `"com.myapp.orders"`).
    pub source: String,

    /// Event detail type (e.g. `"OrderCreated"`).
    pub detail_type: String,

    /// Event detail as a JSON object.
    pub detail: serde_json::Value,

    /// Optional list of resource ARNs associated with the event.
    #[serde(default)]
    pub resources: Vec<String>,
}

/// AWS `EventBridge` provider for publishing events.
pub struct EventBridgeProvider {
    config: EventBridgeConfig,
    client: aws_sdk_eventbridge::Client,
}

impl std::fmt::Debug for EventBridgeProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBridgeProvider")
            .field("config", &self.config)
            .field("client", &"<EventBridgeClient>")
            .finish()
    }
}

impl EventBridgeProvider {
    /// Create a new `EventBridgeProvider` by building an AWS SDK client.
    pub async fn new(config: EventBridgeConfig) -> Self {
        let sdk_config = build_sdk_config(&config.aws).await;
        let client = aws_sdk_eventbridge::Client::new(&sdk_config);
        Self { config, client }
    }

    /// Create an `EventBridgeProvider` with a pre-built client (for testing).
    pub fn with_client(config: EventBridgeConfig, client: aws_sdk_eventbridge::Client) -> Self {
        Self { config, client }
    }
}

impl Provider for EventBridgeProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "aws-eventbridge"
    }

    #[instrument(skip(self, action), fields(action_id = %action.id, provider = "aws-eventbridge"))]
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing EventBridge payload");
        let payload: EventBridgePutPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let event_bus = payload
            .event_bus_name
            .as_deref()
            .or(self.config.event_bus_name.as_deref())
            .unwrap_or("default");

        let detail_json = serde_json::to_string(&payload.detail)
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        debug!(
            event_bus = %event_bus,
            source = %payload.source,
            detail_type = %payload.detail_type,
            "putting event to EventBridge"
        );

        let mut entry = aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
            .event_bus_name(event_bus)
            .source(&payload.source)
            .detail_type(&payload.detail_type)
            .detail(&detail_json);

        for resource in &payload.resources {
            entry = entry.resources(resource);
        }

        let result = self
            .client
            .put_events()
            .entries(entry.build())
            .send()
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                error!(error = %err_str, "EventBridge put_events failed");
                let aws_err: ProviderError = classify_sdk_error(&err_str).into();
                aws_err
            })?;

        let failed_count = result.failed_entry_count();
        if failed_count > 0 {
            let error_msg = result
                .entries()
                .iter()
                .filter_map(|e| e.error_message())
                .collect::<Vec<_>>()
                .join("; ");
            error!(
                failed_count = failed_count,
                "some EventBridge entries failed"
            );
            return Err(ProviderError::ExecutionFailed(format!(
                "{failed_count} entries failed: {error_msg}"
            )));
        }

        let event_id = result
            .entries()
            .first()
            .and_then(|e| e.event_id())
            .unwrap_or("unknown")
            .to_owned();

        info!(event_id = %event_id, event_bus = %event_bus, "event published to EventBridge");

        Ok(ProviderResponse::success(serde_json::json!({
            "event_id": event_id,
            "event_bus": event_bus,
            "status": "published"
        })))
    }

    #[instrument(skip(self), fields(provider = "aws-eventbridge"))]
    async fn health_check(&self) -> Result<(), ProviderError> {
        debug!("performing EventBridge health check");
        self.client
            .list_event_buses()
            .limit(1)
            .send()
            .await
            .map_err(|e| {
                error!(error = %e, "EventBridge health check failed");
                ProviderError::Connection(format!("EventBridge health check failed: {e}"))
            })?;
        info!("EventBridge health check passed");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_new_sets_region() {
        let config = EventBridgeConfig::new("us-east-1");
        assert_eq!(config.aws.region, "us-east-1");
        assert!(config.event_bus_name.is_none());
    }

    #[test]
    fn config_with_event_bus_name() {
        let config = EventBridgeConfig::new("us-east-1").with_event_bus_name("custom-bus");
        assert_eq!(config.event_bus_name.as_deref(), Some("custom-bus"));
    }

    #[test]
    fn config_debug_format() {
        let config =
            EventBridgeConfig::new("us-east-1").with_role_arn("arn:aws:iam::123:role/test");
        let debug = format!("{config:?}");
        assert!(debug.contains("EventBridgeConfig"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn config_serde_roundtrip() {
        let config = EventBridgeConfig::new("eu-west-1").with_event_bus_name("my-bus");

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: EventBridgeConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.aws.region, "eu-west-1");
        assert_eq!(deserialized.event_bus_name.as_deref(), Some("my-bus"));
    }

    #[test]
    fn deserialize_put_payload() {
        let json = serde_json::json!({
            "source": "com.myapp.orders",
            "detail_type": "OrderCreated",
            "detail": {"order_id": "123", "amount": 99.99},
            "resources": ["arn:aws:s3:::my-bucket"]
        });
        let payload: EventBridgePutPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.source, "com.myapp.orders");
        assert_eq!(payload.detail_type, "OrderCreated");
        assert_eq!(payload.resources.len(), 1);
    }

    #[test]
    fn deserialize_minimal_put_payload() {
        let json = serde_json::json!({
            "source": "test",
            "detail_type": "TestEvent",
            "detail": {}
        });
        let payload: EventBridgePutPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.source, "test");
        assert!(payload.resources.is_empty());
        assert!(payload.event_bus_name.is_none());
    }
}
