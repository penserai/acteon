//! Convenience helpers for creating Azure-targeted actions.
//!
//! Provides builder types for each supported Azure provider:
//! [`blob`] and [`eventhubs`].
//!
//! # Example
//!
//! ```no_run
//! use acteon_client::azure;
//!
//! // Upload a blob
//! let action = azure::blob::upload("storage", "tenant-1")
//!     .blob_name("data.json")
//!     .body(r#"{"hello":"world"}"#)
//!     .content_type("application/json")
//!     .build();
//!
//! // Send an event to Event Hubs
//! let action = azure::eventhubs::send_event("events", "tenant-1")
//!     .body(serde_json::json!({"key": "value"}))
//!     .build();
//! ```

// =============================================================================
// Blob Storage
// =============================================================================

/// Helpers for creating Azure Blob Storage actions.
pub mod blob {
    use acteon_core::Action;
    use std::collections::HashMap;

    /// Create a builder for an Azure Blob Storage `upload_blob` action.
    pub fn upload(namespace: impl Into<String>, tenant: impl Into<String>) -> BlobUploadBuilder {
        BlobUploadBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            blob_name: String::new(),
            container: None,
            body: None,
            body_base64: None,
            content_type: None,
            metadata: None,
            dedup_key: None,
        }
    }

    /// Create a builder for an Azure Blob Storage `download_blob` action.
    pub fn download(
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> BlobDownloadBuilder {
        BlobDownloadBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            blob_name: String::new(),
            container: None,
            dedup_key: None,
        }
    }

    /// Create a builder for an Azure Blob Storage `delete_blob` action.
    pub fn delete(namespace: impl Into<String>, tenant: impl Into<String>) -> BlobDeleteBuilder {
        BlobDeleteBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            blob_name: String::new(),
            container: None,
            dedup_key: None,
        }
    }

    /// Builder for an Azure Blob Storage `upload_blob` action.
    #[derive(Debug)]
    pub struct BlobUploadBuilder {
        namespace: String,
        tenant: String,
        blob_name: String,
        container: Option<String>,
        body: Option<String>,
        body_base64: Option<String>,
        content_type: Option<String>,
        metadata: Option<HashMap<String, String>>,
        dedup_key: Option<String>,
    }

    impl BlobUploadBuilder {
        /// Set the blob name.
        #[must_use]
        pub fn blob_name(mut self, name: impl Into<String>) -> Self {
            self.blob_name = name.into();
            self
        }

        /// Override the container name configured on the provider.
        #[must_use]
        pub fn container(mut self, container: impl Into<String>) -> Self {
            self.container = Some(container.into());
            self
        }

        /// Set the blob body as a UTF-8 string.
        #[must_use]
        pub fn body(mut self, body: impl Into<String>) -> Self {
            self.body = Some(body.into());
            self
        }

        /// Set the blob body as base64-encoded bytes.
        #[must_use]
        pub fn body_base64(mut self, data: impl Into<String>) -> Self {
            self.body_base64 = Some(data.into());
            self
        }

        /// Set the content type (e.g., `"application/json"`).
        #[must_use]
        pub fn content_type(mut self, ct: impl Into<String>) -> Self {
            self.content_type = Some(ct.into());
            self
        }

        /// Set blob metadata as key-value string pairs.
        #[must_use]
        pub fn metadata(mut self, metadata: HashMap<String, String>) -> Self {
            self.metadata = Some(metadata);
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the Azure Blob Storage `upload_blob` Action.
        ///
        /// # Panics
        ///
        /// Panics if `blob_name` has not been set.
        pub fn build(self) -> Action {
            assert!(
                !self.blob_name.is_empty(),
                "Azure Blob blob_name must be set"
            );

            let mut payload = serde_json::Map::new();
            payload.insert(
                "blob_name".to_string(),
                serde_json::Value::String(self.blob_name),
            );

            if let Some(v) = self.container {
                payload.insert("container".to_string(), serde_json::Value::String(v));
            }
            if let Some(v) = self.body {
                payload.insert("body".to_string(), serde_json::Value::String(v));
            }
            if let Some(v) = self.body_base64 {
                payload.insert("body_base64".to_string(), serde_json::Value::String(v));
            }
            if let Some(v) = self.content_type {
                payload.insert("content_type".to_string(), serde_json::Value::String(v));
            }
            if let Some(v) = self.metadata {
                payload.insert(
                    "metadata".to_string(),
                    serde_json::to_value(v)
                        .expect("HashMap<String, String> serialization cannot fail"),
                );
            }

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "azure-blob",
                "upload_blob",
                serde_json::Value::Object(payload),
            );

            if let Some(key) = self.dedup_key {
                action.dedup_key = Some(key);
            }

            action
        }
    }

    /// Builder for an Azure Blob Storage `download_blob` action.
    #[derive(Debug)]
    pub struct BlobDownloadBuilder {
        namespace: String,
        tenant: String,
        blob_name: String,
        container: Option<String>,
        dedup_key: Option<String>,
    }

    impl BlobDownloadBuilder {
        /// Set the blob name.
        #[must_use]
        pub fn blob_name(mut self, name: impl Into<String>) -> Self {
            self.blob_name = name.into();
            self
        }

        /// Override the container name configured on the provider.
        #[must_use]
        pub fn container(mut self, container: impl Into<String>) -> Self {
            self.container = Some(container.into());
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the Azure Blob Storage `download_blob` Action.
        ///
        /// # Panics
        ///
        /// Panics if `blob_name` has not been set.
        pub fn build(self) -> Action {
            assert!(
                !self.blob_name.is_empty(),
                "Azure Blob blob_name must be set"
            );

            let mut payload = serde_json::Map::new();
            payload.insert(
                "blob_name".to_string(),
                serde_json::Value::String(self.blob_name),
            );

            if let Some(v) = self.container {
                payload.insert("container".to_string(), serde_json::Value::String(v));
            }

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "azure-blob",
                "download_blob",
                serde_json::Value::Object(payload),
            );

            if let Some(key) = self.dedup_key {
                action.dedup_key = Some(key);
            }

            action
        }
    }

    /// Builder for an Azure Blob Storage `delete_blob` action.
    #[derive(Debug)]
    pub struct BlobDeleteBuilder {
        namespace: String,
        tenant: String,
        blob_name: String,
        container: Option<String>,
        dedup_key: Option<String>,
    }

    impl BlobDeleteBuilder {
        /// Set the blob name.
        #[must_use]
        pub fn blob_name(mut self, name: impl Into<String>) -> Self {
            self.blob_name = name.into();
            self
        }

        /// Override the container name configured on the provider.
        #[must_use]
        pub fn container(mut self, container: impl Into<String>) -> Self {
            self.container = Some(container.into());
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the Azure Blob Storage `delete_blob` Action.
        ///
        /// # Panics
        ///
        /// Panics if `blob_name` has not been set.
        pub fn build(self) -> Action {
            assert!(
                !self.blob_name.is_empty(),
                "Azure Blob blob_name must be set"
            );

            let mut payload = serde_json::Map::new();
            payload.insert(
                "blob_name".to_string(),
                serde_json::Value::String(self.blob_name),
            );

            if let Some(v) = self.container {
                payload.insert("container".to_string(), serde_json::Value::String(v));
            }

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "azure-blob",
                "delete_blob",
                serde_json::Value::Object(payload),
            );

            if let Some(key) = self.dedup_key {
                action.dedup_key = Some(key);
            }

            action
        }
    }
}

// =============================================================================
// Event Hubs
// =============================================================================

/// Helpers for creating Azure Event Hubs actions.
pub mod eventhubs {
    use acteon_core::Action;
    use std::collections::HashMap;

    /// Create a builder for an Azure Event Hubs `send_event` action.
    pub fn send_event(
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> EventHubsSendBuilder {
        EventHubsSendBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            body: serde_json::Value::Object(serde_json::Map::new()),
            event_hub_name: None,
            partition_id: None,
            properties: None,
            dedup_key: None,
        }
    }

    /// Create a builder for an Azure Event Hubs `send_batch` action.
    pub fn send_batch(
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> EventHubsSendBatchBuilder {
        EventHubsSendBatchBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            events: Vec::new(),
            event_hub_name: None,
            dedup_key: None,
        }
    }

    /// Builder for an Azure Event Hubs `send_event` action.
    #[derive(Debug)]
    pub struct EventHubsSendBuilder {
        namespace: String,
        tenant: String,
        body: serde_json::Value,
        event_hub_name: Option<String>,
        partition_id: Option<String>,
        properties: Option<HashMap<String, String>>,
        dedup_key: Option<String>,
    }

    impl EventHubsSendBuilder {
        /// Set the event body as a JSON value.
        #[must_use]
        pub fn body(mut self, body: serde_json::Value) -> Self {
            self.body = body;
            self
        }

        /// Override the Event Hub name configured on the provider.
        #[must_use]
        pub fn event_hub_name(mut self, name: impl Into<String>) -> Self {
            self.event_hub_name = Some(name.into());
            self
        }

        /// Target a specific partition.
        #[must_use]
        pub fn partition_id(mut self, id: impl Into<String>) -> Self {
            self.partition_id = Some(id.into());
            self
        }

        /// Set application properties as key-value string pairs.
        #[must_use]
        pub fn properties(mut self, props: HashMap<String, String>) -> Self {
            self.properties = Some(props);
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the Azure Event Hubs `send_event` Action.
        pub fn build(self) -> Action {
            let mut payload = serde_json::Map::new();
            payload.insert("body".to_string(), self.body);

            if let Some(v) = self.event_hub_name {
                payload.insert("event_hub_name".to_string(), serde_json::Value::String(v));
            }
            if let Some(v) = self.partition_id {
                payload.insert("partition_id".to_string(), serde_json::Value::String(v));
            }
            if let Some(v) = self.properties {
                payload.insert(
                    "properties".to_string(),
                    serde_json::to_value(v)
                        .expect("HashMap<String, String> serialization cannot fail"),
                );
            }

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "azure-eventhubs",
                "send_event",
                serde_json::Value::Object(payload),
            );

            if let Some(key) = self.dedup_key {
                action.dedup_key = Some(key);
            }

            action
        }
    }

    /// Builder for an Azure Event Hubs `send_batch` action.
    #[derive(Debug)]
    pub struct EventHubsSendBatchBuilder {
        namespace: String,
        tenant: String,
        events: Vec<serde_json::Value>,
        event_hub_name: Option<String>,
        dedup_key: Option<String>,
    }

    impl EventHubsSendBatchBuilder {
        /// Set the list of events to send as a batch.
        #[must_use]
        pub fn events(mut self, events: Vec<serde_json::Value>) -> Self {
            self.events = events;
            self
        }

        /// Override the Event Hub name configured on the provider.
        #[must_use]
        pub fn event_hub_name(mut self, name: impl Into<String>) -> Self {
            self.event_hub_name = Some(name.into());
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the Azure Event Hubs `send_batch` Action.
        ///
        /// # Panics
        ///
        /// Panics if `events` is empty.
        pub fn build(self) -> Action {
            assert!(
                !self.events.is_empty(),
                "Event Hubs events must not be empty"
            );

            let mut payload = serde_json::Map::new();
            payload.insert(
                "events".to_string(),
                serde_json::to_value(&self.events).expect("Vec<Value> serialization cannot fail"),
            );

            if let Some(v) = self.event_hub_name {
                payload.insert("event_hub_name".to_string(), serde_json::Value::String(v));
            }

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "azure-eventhubs",
                "send_batch",
                serde_json::Value::Object(payload),
            );

            if let Some(key) = self.dedup_key {
                action.dedup_key = Some(key);
            }

            action
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // =========================================================================
    // Blob Storage tests
    // =========================================================================

    #[test]
    fn blob_upload_basic() {
        let a = blob::upload("ns", "t1")
            .blob_name("data.json")
            .body(r#"{"hello":"world"}"#)
            .build();

        assert_eq!(a.provider.as_str(), "azure-blob");
        assert_eq!(a.action_type, "upload_blob");
        assert_eq!(a.payload["blob_name"], "data.json");
        assert_eq!(a.payload["body"], r#"{"hello":"world"}"#);
    }

    #[test]
    fn blob_upload_with_all_options() {
        let mut meta = HashMap::new();
        meta.insert("env".to_string(), "staging".to_string());

        let a = blob::upload("ns", "t1")
            .blob_name("data.bin")
            .container("my-container")
            .body_base64("SGVsbG8gV29ybGQ=")
            .content_type("application/octet-stream")
            .metadata(meta)
            .dedup_key("upload-dedup")
            .build();

        assert_eq!(a.payload["blob_name"], "data.bin");
        assert_eq!(a.payload["container"], "my-container");
        assert_eq!(a.payload["body_base64"], "SGVsbG8gV29ybGQ=");
        assert_eq!(a.payload["content_type"], "application/octet-stream");
        assert_eq!(a.payload["metadata"]["env"], "staging");
        assert_eq!(a.dedup_key.as_deref(), Some("upload-dedup"));
    }

    #[test]
    fn blob_download_basic() {
        let a = blob::download("ns", "t1").blob_name("data.json").build();

        assert_eq!(a.provider.as_str(), "azure-blob");
        assert_eq!(a.action_type, "download_blob");
        assert_eq!(a.payload["blob_name"], "data.json");
    }

    #[test]
    fn blob_download_with_container() {
        let a = blob::download("ns", "t1")
            .blob_name("data.json")
            .container("my-container")
            .build();

        assert_eq!(a.payload["container"], "my-container");
    }

    #[test]
    fn blob_delete_basic() {
        let a = blob::delete("ns", "t1").blob_name("data.json").build();

        assert_eq!(a.provider.as_str(), "azure-blob");
        assert_eq!(a.action_type, "delete_blob");
        assert_eq!(a.payload["blob_name"], "data.json");
    }

    #[test]
    fn blob_delete_with_container() {
        let a = blob::delete("ns", "t1")
            .blob_name("data.json")
            .container("my-container")
            .dedup_key("del-dedup")
            .build();

        assert_eq!(a.payload["container"], "my-container");
        assert_eq!(a.dedup_key.as_deref(), Some("del-dedup"));
    }

    #[test]
    #[should_panic(expected = "Azure Blob blob_name must be set")]
    fn blob_upload_panics_without_blob_name() {
        blob::upload("ns", "t1").build();
    }

    #[test]
    #[should_panic(expected = "Azure Blob blob_name must be set")]
    fn blob_download_panics_without_blob_name() {
        blob::download("ns", "t1").build();
    }

    #[test]
    #[should_panic(expected = "Azure Blob blob_name must be set")]
    fn blob_delete_panics_without_blob_name() {
        blob::delete("ns", "t1").build();
    }

    // =========================================================================
    // Event Hubs tests
    // =========================================================================

    #[test]
    fn eventhubs_send_basic() {
        let a = eventhubs::send_event("ns", "t1")
            .body(serde_json::json!({"key": "value"}))
            .build();

        assert_eq!(a.provider.as_str(), "azure-eventhubs");
        assert_eq!(a.action_type, "send_event");
        assert_eq!(a.payload["body"]["key"], "value");
    }

    #[test]
    fn eventhubs_send_with_all_options() {
        let mut props = HashMap::new();
        props.insert("source".to_string(), "my-app".to_string());

        let a = eventhubs::send_event("ns", "t1")
            .body(serde_json::json!({"data": 42}))
            .event_hub_name("my-hub")
            .partition_id("0")
            .properties(props)
            .dedup_key("send-dedup")
            .build();

        assert_eq!(a.payload["event_hub_name"], "my-hub");
        assert_eq!(a.payload["partition_id"], "0");
        assert_eq!(a.payload["properties"]["source"], "my-app");
        assert_eq!(a.dedup_key.as_deref(), Some("send-dedup"));
    }

    #[test]
    fn eventhubs_send_batch_basic() {
        let events = vec![
            serde_json::json!({"body": "event1"}),
            serde_json::json!({"body": "event2"}),
        ];

        let a = eventhubs::send_batch("ns", "t1").events(events).build();

        assert_eq!(a.provider.as_str(), "azure-eventhubs");
        assert_eq!(a.action_type, "send_batch");
        assert_eq!(a.payload["events"][0]["body"], "event1");
        assert_eq!(a.payload["events"][1]["body"], "event2");
    }

    #[test]
    fn eventhubs_send_batch_with_hub_name() {
        let events = vec![serde_json::json!({"body": "event1"})];

        let a = eventhubs::send_batch("ns", "t1")
            .events(events)
            .event_hub_name("my-hub")
            .dedup_key("batch-dedup")
            .build();

        assert_eq!(a.payload["event_hub_name"], "my-hub");
        assert_eq!(a.dedup_key.as_deref(), Some("batch-dedup"));
    }

    #[test]
    #[should_panic(expected = "Event Hubs events must not be empty")]
    fn eventhubs_send_batch_panics_without_events() {
        eventhubs::send_batch("ns", "t1").build();
    }
}
