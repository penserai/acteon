use std::collections::HashMap;
use std::sync::Arc;

use acteon_core::{Action, ProviderResponse};
use acteon_provider::ProviderError;
use acteon_provider::provider::Provider;
use dashmap::DashMap;
use google_cloud_auth::credentials::Credentials;
use google_cloud_pubsub::client::Publisher;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, instrument};

use crate::auth::build_gcp_credentials;
use crate::config::GcpBaseConfig;
use crate::error::classify_gcp_error;

/// Configuration for the GCP `Pub/Sub` provider.
#[derive(Clone, Serialize, Deserialize)]
pub struct PubSubConfig {
    /// Shared GCP configuration.
    #[serde(flatten)]
    pub gcp: GcpBaseConfig,

    /// Default `Pub/Sub` topic name. Can be overridden per-action in the payload.
    pub topic: Option<String>,
}

impl std::fmt::Debug for PubSubConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PubSubConfig")
            .field("gcp", &self.gcp)
            .field("topic", &self.topic)
            .finish()
    }
}

impl PubSubConfig {
    /// Create a new `PubSubConfig` with the given GCP project ID.
    pub fn new(project_id: impl Into<String>) -> Self {
        Self {
            gcp: GcpBaseConfig::new(project_id),
            topic: None,
        }
    }

    /// Set the default `Pub/Sub` topic name.
    #[must_use]
    pub fn with_topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = Some(topic.into());
        self
    }

    /// Set the endpoint URL override (for the `Pub/Sub` emulator).
    #[must_use]
    pub fn with_endpoint_url(mut self, endpoint_url: impl Into<String>) -> Self {
        self.gcp.endpoint_url = Some(endpoint_url.into());
        self
    }

    /// Set the path to a service account JSON key file.
    #[must_use]
    pub fn with_credentials_path(mut self, path: impl Into<String>) -> Self {
        self.gcp.credentials_path = Some(path.into());
        self
    }
}

/// Payload for the `publish` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PubSubPublishPayload {
    /// Message data as a UTF-8 string.
    /// Mutually exclusive with `data_base64`.
    pub data: Option<String>,

    /// Message data as base64-encoded bytes.
    /// Mutually exclusive with `data`.
    pub data_base64: Option<String>,

    /// Optional message attributes (key-value pairs).
    #[serde(default)]
    pub attributes: HashMap<String, String>,

    /// Optional ordering key for message ordering.
    pub ordering_key: Option<String>,

    /// Topic name override. Overrides config default.
    pub topic: Option<String>,
}

/// A single message within a `publish_batch` payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PubSubBatchMessage {
    /// Message data as a UTF-8 string.
    pub data: Option<String>,

    /// Message data as base64-encoded bytes.
    pub data_base64: Option<String>,

    /// Optional message attributes.
    #[serde(default)]
    pub attributes: HashMap<String, String>,

    /// Optional ordering key.
    pub ordering_key: Option<String>,
}

/// Payload for the `publish_batch` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PubSubPublishBatchPayload {
    /// Topic name override. Overrides config default.
    pub topic: Option<String>,

    /// List of messages to publish.
    pub messages: Vec<PubSubBatchMessage>,
}

/// GCP `Pub/Sub` provider for publishing messages.
pub struct PubSubProvider {
    config: PubSubConfig,
    credentials: Option<Credentials>,
    publishers: DashMap<String, Publisher>,
}

impl std::fmt::Debug for PubSubProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PubSubProvider")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl PubSubProvider {
    /// Create a new `PubSubProvider` by resolving GCP credentials.
    pub async fn new(config: PubSubConfig) -> Result<Self, ProviderError> {
        let credentials = build_gcp_credentials(config.gcp.credentials_path.as_deref())
            .await
            .map_err(|e| ProviderError::Configuration(e.to_string()))?;

        Ok(Self {
            config,
            credentials,
            publishers: DashMap::new(),
        })
    }

    /// Build a [`Publisher`] for the given fully-qualified topic path, or return a cached one.
    async fn get_or_build_publisher(&self, topic_path: &str) -> Result<Publisher, ProviderError> {
        if let Some(publisher) = self.publishers.get(topic_path) {
            return Ok(publisher.clone());
        }

        let mut builder = Publisher::builder(topic_path);
        if let Some(ref endpoint) = self.config.gcp.endpoint_url {
            builder = builder.with_endpoint(endpoint);
        }
        if let Some(ref creds) = self.credentials {
            builder = builder.with_credentials(creds.clone());
        }

        let publisher = builder
            .build()
            .await
            .map_err(|e| ProviderError::Configuration(format!("Pub/Sub publisher error: {e}")))?;

        self.publishers
            .insert(topic_path.to_owned(), publisher.clone());
        Ok(publisher)
    }

    /// Resolve the topic name from the payload or config default.
    fn resolve_topic<'a>(&'a self, payload_topic: Option<&'a str>) -> Option<&'a str> {
        payload_topic.or(self.config.topic.as_deref())
    }

    /// Build the fully-qualified topic path from a topic name.
    fn topic_path(&self, topic_name: &str) -> String {
        format!(
            "projects/{}/topics/{topic_name}",
            self.config.gcp.project_id
        )
    }

    /// Resolve message data from either `data` (UTF-8) or `data_base64` fields.
    fn resolve_data(
        data: Option<&str>,
        data_base64: Option<&str>,
    ) -> Result<Vec<u8>, ProviderError> {
        if let Some(b64) = data_base64 {
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64)
                .map_err(|e| ProviderError::Serialization(format!("invalid base64 data: {e}")))
        } else if let Some(text) = data {
            Ok(text.as_bytes().to_vec())
        } else {
            Ok(Vec::new())
        }
    }

    /// Build a [`google_cloud_pubsub::model::Message`] from data fields.
    fn build_message(
        data_bytes: Vec<u8>,
        attributes: &HashMap<String, String>,
        ordering_key: Option<&str>,
    ) -> google_cloud_pubsub::model::Message {
        let mut msg =
            google_cloud_pubsub::model::Message::new().set_data(bytes::Bytes::from(data_bytes));
        if !attributes.is_empty() {
            msg = msg.set_attributes(attributes.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        if let Some(key) = ordering_key {
            msg = msg.set_ordering_key(key);
        }
        msg
    }
}

impl Provider for PubSubProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "gcp-pubsub"
    }

    #[instrument(skip(self, action), fields(action_id = %action.id, provider = "gcp-pubsub"))]
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        match action.action_type.as_str() {
            "publish" => self.publish(action).await,
            "publish_batch" => self.publish_batch(action).await,
            other => Err(ProviderError::Configuration(format!(
                "unknown Pub/Sub action type '{other}' (expected 'publish' or 'publish_batch')"
            ))),
        }
    }

    #[instrument(skip(self), fields(provider = "gcp-pubsub"))]
    async fn health_check(&self) -> Result<(), ProviderError> {
        debug!("performing Pub/Sub health check");
        // Build a publisher to verify connectivity. If credentials or endpoint
        // are invalid, this will fail.
        if let Some(topic) = self.config.topic.as_deref() {
            let topic_path = self.topic_path(topic);
            let _publisher = self.get_or_build_publisher(&topic_path).await?;
        }
        info!("Pub/Sub health check passed");
        Ok(())
    }
}

impl PubSubProvider {
    async fn publish(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing Pub/Sub publish payload");
        let payload: PubSubPublishPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let topic_name = self
            .resolve_topic(payload.topic.as_deref())
            .ok_or_else(|| {
                ProviderError::Configuration("no topic in payload or provider config".to_owned())
            })?;

        let topic_path = self.topic_path(topic_name);
        debug!(topic = %topic_name, "publishing message to Pub/Sub");

        let data_bytes =
            Self::resolve_data(payload.data.as_deref(), payload.data_base64.as_deref())?;
        let msg = Self::build_message(
            data_bytes,
            &payload.attributes,
            payload.ordering_key.as_deref(),
        );

        let publisher = self.get_or_build_publisher(&topic_path).await?;
        let message_id =
            publisher
                .publish(msg)
                .await
                .map_err(|e: Arc<google_cloud_pubsub::Error>| {
                    let err_str = e.to_string();
                    error!(error = %err_str, "Pub/Sub publish failed");
                    let gcp_err: ProviderError = classify_gcp_error(&err_str).into();
                    gcp_err
                })?;

        info!(topic = %topic_name, message_id = %message_id, "message published to Pub/Sub");

        Ok(ProviderResponse::success(serde_json::json!({
            "topic": topic_name,
            "message_id": message_id,
            "status": "published"
        })))
    }

    async fn publish_batch(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing Pub/Sub publish_batch payload");
        let payload: PubSubPublishBatchPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let topic_name = self
            .resolve_topic(payload.topic.as_deref())
            .ok_or_else(|| {
                ProviderError::Configuration("no topic in payload or provider config".to_owned())
            })?;

        let msg_count = payload.messages.len();
        let topic_path = self.topic_path(topic_name);
        debug!(topic = %topic_name, count = msg_count, "publishing batch to Pub/Sub");

        let publisher = self.get_or_build_publisher(&topic_path).await?;

        // Publish all messages and collect their futures.
        let mut publish_futures = Vec::with_capacity(msg_count);
        for m in &payload.messages {
            let data_bytes = Self::resolve_data(m.data.as_deref(), m.data_base64.as_deref())?;
            let msg = Self::build_message(data_bytes, &m.attributes, m.ordering_key.as_deref());
            publish_futures.push(publisher.publish(msg));
        }

        // Flush to ensure all messages are sent.
        publisher.flush().await;

        // Collect message IDs.
        let mut message_ids = Vec::with_capacity(msg_count);
        for future in publish_futures {
            let message_id = future.await.map_err(|e: Arc<google_cloud_pubsub::Error>| {
                let err_str = e.to_string();
                error!(error = %err_str, "Pub/Sub batch publish failed");
                let gcp_err: ProviderError = classify_gcp_error(&err_str).into();
                gcp_err
            })?;
            message_ids.push(message_id);
        }

        info!(topic = %topic_name, count = msg_count, "batch published to Pub/Sub");

        Ok(ProviderResponse::success(serde_json::json!({
            "topic": topic_name,
            "message_count": msg_count,
            "message_ids": message_ids,
            "status": "published"
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_new_sets_project_id() {
        let config = PubSubConfig::new("my-project");
        assert_eq!(config.gcp.project_id, "my-project");
        assert!(config.topic.is_none());
    }

    #[test]
    fn config_with_topic() {
        let config = PubSubConfig::new("my-project").with_topic("my-topic");
        assert_eq!(config.topic.as_deref(), Some("my-topic"));
    }

    #[test]
    fn config_builder_chain() {
        let config = PubSubConfig::new("test-project")
            .with_topic("events")
            .with_endpoint_url("http://localhost:8085")
            .with_credentials_path("/path/to/sa.json");
        assert_eq!(config.topic.as_deref(), Some("events"));
        assert_eq!(
            config.gcp.endpoint_url.as_deref(),
            Some("http://localhost:8085")
        );
        assert!(config.gcp.credentials_path.is_some());
    }

    #[test]
    fn config_debug_format() {
        let config = PubSubConfig::new("my-project")
            .with_topic("events")
            .with_credentials_path("/private/key.json");
        let debug = format!("{config:?}");
        assert!(debug.contains("PubSubConfig"));
        assert!(debug.contains("[REDACTED]"));
        assert!(debug.contains("events"));
    }

    #[test]
    fn config_serde_roundtrip() {
        let config = PubSubConfig::new("serde-project").with_topic("telemetry");

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: PubSubConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.gcp.project_id, "serde-project");
        assert_eq!(deserialized.topic.as_deref(), Some("telemetry"));
    }

    #[test]
    fn deserialize_publish_payload() {
        let json = serde_json::json!({
            "data": "{\"temperature\": 72.5}",
            "attributes": {
                "source": "sensor"
            },
            "ordering_key": "device-001",
            "topic": "telemetry"
        });
        let payload: PubSubPublishPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.data.as_deref(), Some("{\"temperature\": 72.5}"));
        assert_eq!(payload.attributes.get("source").unwrap(), "sensor");
        assert_eq!(payload.ordering_key.as_deref(), Some("device-001"));
        assert_eq!(payload.topic.as_deref(), Some("telemetry"));
    }

    #[test]
    fn deserialize_publish_payload_base64() {
        let json = serde_json::json!({
            "data_base64": "SGVsbG8gV29ybGQ="
        });
        let payload: PubSubPublishPayload = serde_json::from_value(json).unwrap();
        assert!(payload.data.is_none());
        assert_eq!(payload.data_base64.as_deref(), Some("SGVsbG8gV29ybGQ="));
    }

    #[test]
    fn deserialize_minimal_publish_payload() {
        let json = serde_json::json!({
            "data": "hello"
        });
        let payload: PubSubPublishPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.data.as_deref(), Some("hello"));
        assert!(payload.data_base64.is_none());
        assert!(payload.topic.is_none());
        assert!(payload.ordering_key.is_none());
        assert!(payload.attributes.is_empty());
    }

    #[test]
    fn deserialize_batch_payload() {
        let json = serde_json::json!({
            "topic": "events",
            "messages": [
                {"data": "event 1"},
                {"data": "{\"key\": \"value\"}", "ordering_key": "k1"},
                {"data_base64": "SGVsbG8="}
            ]
        });
        let payload: PubSubPublishBatchPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.topic.as_deref(), Some("events"));
        assert_eq!(payload.messages.len(), 3);
        assert_eq!(payload.messages[0].data.as_deref(), Some("event 1"));
        assert_eq!(payload.messages[1].ordering_key.as_deref(), Some("k1"));
        assert_eq!(payload.messages[2].data_base64.as_deref(), Some("SGVsbG8="));
    }

    #[test]
    fn resolve_data_text() {
        let data = PubSubProvider::resolve_data(Some("hello"), None).unwrap();
        assert_eq!(data, b"hello");
    }

    #[test]
    fn resolve_data_base64() {
        let data = PubSubProvider::resolve_data(None, Some("SGVsbG8=")).unwrap();
        assert_eq!(data, b"Hello");
    }

    #[test]
    fn resolve_data_empty() {
        let data = PubSubProvider::resolve_data(None, None).unwrap();
        assert!(data.is_empty());
    }

    #[test]
    fn resolve_data_invalid_base64() {
        let result = PubSubProvider::resolve_data(None, Some("!!!invalid!!!"));
        assert!(result.is_err());
    }

    #[test]
    fn build_message_with_data() {
        let msg = PubSubProvider::build_message(b"hello".to_vec(), &HashMap::new(), None);
        assert_eq!(msg.data, bytes::Bytes::from_static(b"hello"));
    }

    #[test]
    fn build_message_with_attributes() {
        let mut attrs = HashMap::new();
        attrs.insert("key".to_owned(), "value".to_owned());
        let msg = PubSubProvider::build_message(b"data".to_vec(), &attrs, Some("ordering-key"));
        assert_eq!(msg.attributes.get("key").unwrap(), "value");
        assert_eq!(msg.ordering_key, "ordering-key");
    }
}
