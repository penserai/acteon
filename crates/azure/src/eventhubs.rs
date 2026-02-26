use std::collections::HashMap;

use acteon_core::{Action, ProviderResponse};
use acteon_provider::ProviderError;
use acteon_provider::provider::Provider;
use azure_messaging_eventhubs::{EventDataBatchOptions, ProducerClient, SendEventOptions};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, instrument};

use crate::auth::build_azure_credential;
use crate::config::AzureBaseConfig;
use crate::error::classify_azure_error;

/// Configuration for the Azure Event Hubs provider.
#[derive(Clone, Serialize, Deserialize)]
pub struct EventHubsConfig {
    /// Shared Azure configuration.
    #[serde(flatten)]
    pub azure: AzureBaseConfig,

    /// Event Hubs fully-qualified namespace (e.g. `"mynamespace.servicebus.windows.net"`).
    pub namespace: Option<String>,

    /// Default Event Hub name. Can be overridden per-action in the payload.
    pub event_hub_name: Option<String>,
}

impl std::fmt::Debug for EventHubsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventHubsConfig")
            .field("azure", &self.azure)
            .field("namespace", &self.namespace)
            .field("event_hub_name", &self.event_hub_name)
            .finish()
    }
}

impl EventHubsConfig {
    /// Create a new `EventHubsConfig` with the given Azure location.
    pub fn new(location: impl Into<String>) -> Self {
        Self {
            azure: AzureBaseConfig::new(location),
            namespace: None,
            event_hub_name: None,
        }
    }

    /// Set the Event Hubs fully-qualified namespace.
    #[must_use]
    pub fn with_namespace(mut self, namespace: impl Into<String>) -> Self {
        self.namespace = Some(namespace.into());
        self
    }

    /// Set the default Event Hub name.
    #[must_use]
    pub fn with_event_hub_name(mut self, event_hub_name: impl Into<String>) -> Self {
        self.event_hub_name = Some(event_hub_name.into());
        self
    }

    /// Set the endpoint URL override.
    #[must_use]
    pub fn with_endpoint_url(mut self, endpoint_url: impl Into<String>) -> Self {
        self.azure.endpoint_url = Some(endpoint_url.into());
        self
    }

    /// Set the Azure AD tenant ID.
    #[must_use]
    pub fn with_tenant_id(mut self, tenant_id: impl Into<String>) -> Self {
        self.azure.tenant_id = Some(tenant_id.into());
        self
    }

    /// Set the Azure AD client ID.
    #[must_use]
    pub fn with_client_id(mut self, client_id: impl Into<String>) -> Self {
        self.azure.client_id = Some(client_id.into());
        self
    }

    /// Set the Azure AD client credential.
    #[must_use]
    pub fn with_client_credential(mut self, client_credential: impl Into<String>) -> Self {
        self.azure.client_credential = Some(client_credential.into());
        self
    }
}

/// A single event to send to Event Hubs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventDataPayload {
    /// Event body (JSON or plain text).
    pub body: serde_json::Value,

    /// Optional partition ID for routing.
    pub partition_id: Option<String>,

    /// Optional application properties.
    #[serde(default)]
    pub properties: HashMap<String, String>,
}

/// Payload for the `send_event` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventHubsSendPayload {
    /// Event Hub name. Overrides config default.
    pub event_hub_name: Option<String>,

    /// Event body (JSON or plain text).
    pub body: serde_json::Value,

    /// Optional partition ID for direct routing to a specific partition.
    pub partition_id: Option<String>,

    /// Optional partition key for hash-based partition routing.
    /// Mutually exclusive with `partition_id`.
    pub partition_key: Option<String>,

    /// Optional application properties.
    #[serde(default)]
    pub properties: HashMap<String, String>,
}

/// Payload for the `send_batch` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventHubsSendBatchPayload {
    /// Event Hub name. Overrides config default.
    pub event_hub_name: Option<String>,

    /// Optional partition key for hash-based partition routing (applies to all events in batch).
    /// Mutually exclusive with per-event `partition_id`.
    pub partition_key: Option<String>,

    /// List of events to send.
    pub events: Vec<EventDataPayload>,
}

/// Azure Event Hubs provider for sending events.
pub struct EventHubsProvider {
    config: EventHubsConfig,
    producer: ProducerClient,
}

impl std::fmt::Debug for EventHubsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventHubsProvider")
            .field("config", &self.config)
            .field("producer", &"<ProducerClient>")
            .finish()
    }
}

impl EventHubsProvider {
    /// Create a new `EventHubsProvider` by building an Event Hubs producer client.
    pub async fn new(config: EventHubsConfig) -> Result<Self, ProviderError> {
        let namespace = config.namespace.as_deref().ok_or_else(|| {
            ProviderError::Configuration("azure eventhubs: namespace is required".to_owned())
        })?;

        let event_hub_name = config.event_hub_name.as_deref().ok_or_else(|| {
            ProviderError::Configuration("azure eventhubs: event_hub_name is required".to_owned())
        })?;

        let credential = build_azure_credential(&config.azure)
            .await
            .map_err(|e| ProviderError::Configuration(e.to_string()))?;

        let mut builder = ProducerClient::builder();
        if let Some(ref endpoint) = config.azure.endpoint_url {
            builder = builder.with_custom_endpoint(endpoint.clone());
        }

        let producer = builder
            .open(namespace, event_hub_name, credential)
            .await
            .map_err(|e| {
                ProviderError::Configuration(format!("failed to open Event Hubs producer: {e}"))
            })?;

        Ok(Self { config, producer })
    }
}

impl Provider for EventHubsProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "azure-eventhubs"
    }

    #[instrument(skip(self, action), fields(action_id = %action.id, provider = "azure-eventhubs"))]
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        match action.action_type.as_str() {
            "send_event" => self.send_event(action).await,
            "send_batch" => self.send_batch(action).await,
            other => Err(ProviderError::Configuration(format!(
                "unknown Event Hubs action type '{other}' (expected 'send_event' or 'send_batch')"
            ))),
        }
    }

    #[instrument(skip(self), fields(provider = "azure-eventhubs"))]
    async fn health_check(&self) -> Result<(), ProviderError> {
        debug!("performing Event Hubs health check");
        self.producer.get_eventhub_properties().await.map_err(|e| {
            error!(error = %e, "Event Hubs health check failed");
            ProviderError::Connection(format!("Event Hubs health check failed: {e}"))
        })?;
        info!("Event Hubs health check passed");
        Ok(())
    }
}

impl EventHubsProvider {
    async fn send_event(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing Event Hubs send_event payload");
        let payload: EventHubsSendPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let event_hub_name = payload
            .event_hub_name
            .as_deref()
            .or(self.config.event_hub_name.as_deref())
            .unwrap_or("default");

        debug!(event_hub_name = %event_hub_name, "sending event to Event Hubs");

        let body_str = serde_json::to_string(&payload.body)
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let mut builder = azure_messaging_eventhubs::models::EventData::builder()
            .with_body(body_str.into_bytes());

        for (k, v) in &payload.properties {
            builder = builder.add_property(k.clone(), v.as_str());
        }

        let event = builder.build();

        let options = if payload.partition_id.is_some() || payload.partition_key.is_some() {
            Some(SendEventOptions {
                partition_id: payload.partition_id,
            })
        } else {
            None
        };

        self.producer
            .send_event(event, options)
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                error!(error = %err_str, "Event Hubs send failed");
                let azure_err: ProviderError = classify_azure_error(&err_str).into();
                azure_err
            })?;

        info!(event_hub_name = %event_hub_name, "event sent to Event Hubs");

        Ok(ProviderResponse::success(serde_json::json!({
            "event_hub_name": event_hub_name,
            "event_count": 1,
            "status": "sent"
        })))
    }

    async fn send_batch(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing Event Hubs send_batch payload");
        let payload: EventHubsSendBatchPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let event_hub_name = payload
            .event_hub_name
            .as_deref()
            .or(self.config.event_hub_name.as_deref())
            .unwrap_or("default");

        let event_count = payload.events.len();
        debug!(event_hub_name = %event_hub_name, count = event_count, "sending batch to Event Hubs");

        let batch_options = if payload.partition_key.is_some() {
            Some(EventDataBatchOptions {
                partition_key: payload.partition_key,
                ..Default::default()
            })
        } else {
            None
        };

        let batch = self
            .producer
            .create_batch(batch_options)
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                error!(error = %err_str, "Event Hubs create_batch failed");
                let azure_err: ProviderError = classify_azure_error(&err_str).into();
                azure_err
            })?;

        for ed in &payload.events {
            let body_str = serde_json::to_string(&ed.body)
                .map_err(|e| ProviderError::Serialization(e.to_string()))?;

            let mut builder = azure_messaging_eventhubs::models::EventData::builder()
                .with_body(body_str.into_bytes());

            for (k, v) in &ed.properties {
                builder = builder.add_property(k.clone(), v.as_str());
            }

            let event = builder.build();

            batch.try_add_event_data(event, None).map_err(|e| {
                ProviderError::Serialization(format!("event too large for batch: {e}"))
            })?;
        }

        self.producer.send_batch(batch, None).await.map_err(|e| {
            let err_str = e.to_string();
            error!(error = %err_str, "Event Hubs batch send failed");
            let azure_err: ProviderError = classify_azure_error(&err_str).into();
            azure_err
        })?;

        info!(event_hub_name = %event_hub_name, count = event_count, "batch sent to Event Hubs");

        Ok(ProviderResponse::success(serde_json::json!({
            "event_hub_name": event_hub_name,
            "event_count": event_count,
            "status": "sent"
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_new_sets_location() {
        let config = EventHubsConfig::new("westeurope");
        assert_eq!(config.azure.location, "westeurope");
        assert!(config.namespace.is_none());
        assert!(config.event_hub_name.is_none());
    }

    #[test]
    fn config_with_namespace() {
        let config =
            EventHubsConfig::new("eastus").with_namespace("mynamespace.servicebus.windows.net");
        assert_eq!(
            config.namespace.as_deref(),
            Some("mynamespace.servicebus.windows.net")
        );
    }

    #[test]
    fn config_with_event_hub_name() {
        let config = EventHubsConfig::new("eastus").with_event_hub_name("my-hub");
        assert_eq!(config.event_hub_name.as_deref(), Some("my-hub"));
    }

    #[test]
    fn config_builder_chain() {
        let config = EventHubsConfig::new("eastus2")
            .with_namespace("ns.servicebus.windows.net")
            .with_event_hub_name("events")
            .with_endpoint_url("http://localhost:5672")
            .with_tenant_id("tid-123")
            .with_client_id("cid-456")
            .with_client_credential("cred-789");
        assert_eq!(
            config.namespace.as_deref(),
            Some("ns.servicebus.windows.net")
        );
        assert_eq!(config.event_hub_name.as_deref(), Some("events"));
        assert!(config.azure.endpoint_url.is_some());
        assert!(config.azure.tenant_id.is_some());
    }

    #[test]
    fn config_debug_format() {
        let config = EventHubsConfig::new("eastus")
            .with_namespace("ns.servicebus.windows.net")
            .with_client_credential("private-val");
        let debug = format!("{config:?}");
        assert!(debug.contains("EventHubsConfig"));
        assert!(debug.contains("[REDACTED]"));
        assert!(debug.contains("ns.servicebus.windows.net"));
    }

    #[test]
    fn config_serde_roundtrip() {
        let config = EventHubsConfig::new("northeurope")
            .with_namespace("ns.servicebus.windows.net")
            .with_event_hub_name("telemetry");

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: EventHubsConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.azure.location, "northeurope");
        assert_eq!(
            deserialized.namespace.as_deref(),
            Some("ns.servicebus.windows.net")
        );
        assert_eq!(deserialized.event_hub_name.as_deref(), Some("telemetry"));
    }

    #[test]
    fn deserialize_send_payload() {
        let json = serde_json::json!({
            "body": {"temperature": 72.5, "unit": "F"},
            "partition_id": "0",
            "event_hub_name": "telemetry",
            "properties": {
                "source": "sensor"
            }
        });
        let payload: EventHubsSendPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.event_hub_name.as_deref(), Some("telemetry"));
        assert_eq!(payload.partition_id.as_deref(), Some("0"));
        assert!(payload.body.is_object());
        assert_eq!(payload.properties.get("source").unwrap(), "sensor");
    }

    #[test]
    fn deserialize_minimal_send_payload() {
        let json = serde_json::json!({
            "body": "hello"
        });
        let payload: EventHubsSendPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.body, "hello");
        assert!(payload.event_hub_name.is_none());
        assert!(payload.partition_id.is_none());
        assert!(payload.properties.is_empty());
    }

    #[test]
    fn deserialize_batch_payload() {
        let json = serde_json::json!({
            "event_hub_name": "logs",
            "events": [
                {"body": "event 1"},
                {"body": {"key": "value"}, "partition_id": "1"}
            ]
        });
        let payload: EventHubsSendBatchPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.event_hub_name.as_deref(), Some("logs"));
        assert_eq!(payload.events.len(), 2);
        assert_eq!(payload.events[0].body, "event 1");
        assert_eq!(payload.events[1].partition_id.as_deref(), Some("1"));
    }
}
