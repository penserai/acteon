//! Convenience helpers for creating GCP-targeted actions.
//!
//! Provides builder types for each supported GCP provider:
//! [`pubsub`] and [`storage`].
//!
//! # Example
//!
//! ```no_run
//! use acteon_client::gcp;
//!
//! // Publish a message to Pub/Sub
//! let action = gcp::pubsub::publish("messaging", "tenant-1")
//!     .data(r#"{"hello":"world"}"#)
//!     .topic("my-topic")
//!     .build();
//!
//! // Upload an object to Cloud Storage
//! let action = gcp::storage::upload("storage", "tenant-1")
//!     .object_name("data.json")
//!     .body(r#"{"hello":"world"}"#)
//!     .content_type("application/json")
//!     .build();
//! ```

// =============================================================================
// Pub/Sub
// =============================================================================

/// Helpers for creating GCP `Pub/Sub` actions.
pub mod pubsub {
    use acteon_core::Action;
    use std::collections::HashMap;

    /// Create a builder for a GCP `Pub/Sub` `publish` action.
    pub fn publish(
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> PubSubPublishBuilder {
        PubSubPublishBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            data: None,
            data_base64: None,
            attributes: None,
            ordering_key: None,
            topic: None,
            dedup_key: None,
        }
    }

    /// Create a builder for a GCP `Pub/Sub` `publish_batch` action.
    pub fn publish_batch(
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> PubSubPublishBatchBuilder {
        PubSubPublishBatchBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            messages: Vec::new(),
            topic: None,
            dedup_key: None,
        }
    }

    /// Builder for a GCP `Pub/Sub` `publish` action.
    #[derive(Debug)]
    pub struct PubSubPublishBuilder {
        namespace: String,
        tenant: String,
        data: Option<String>,
        data_base64: Option<String>,
        attributes: Option<HashMap<String, String>>,
        ordering_key: Option<String>,
        topic: Option<String>,
        dedup_key: Option<String>,
    }

    impl PubSubPublishBuilder {
        /// Set the message data as a UTF-8 string.
        #[must_use]
        pub fn data(mut self, data: impl Into<String>) -> Self {
            self.data = Some(data.into());
            self
        }

        /// Set the message data as base64-encoded bytes.
        #[must_use]
        pub fn data_base64(mut self, data: impl Into<String>) -> Self {
            self.data_base64 = Some(data.into());
            self
        }

        /// Set message attributes as key-value string pairs.
        #[must_use]
        pub fn attributes(mut self, attributes: HashMap<String, String>) -> Self {
            self.attributes = Some(attributes);
            self
        }

        /// Set the ordering key for ordered delivery.
        #[must_use]
        pub fn ordering_key(mut self, key: impl Into<String>) -> Self {
            self.ordering_key = Some(key.into());
            self
        }

        /// Override the topic name configured on the provider.
        #[must_use]
        pub fn topic(mut self, topic: impl Into<String>) -> Self {
            self.topic = Some(topic.into());
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the GCP `Pub/Sub` `publish` Action.
        pub fn build(self) -> Action {
            let mut payload = serde_json::Map::new();

            if let Some(v) = self.data {
                payload.insert("data".to_string(), serde_json::Value::String(v));
            }
            if let Some(v) = self.data_base64 {
                payload.insert("data_base64".to_string(), serde_json::Value::String(v));
            }
            if let Some(v) = self.attributes {
                payload.insert(
                    "attributes".to_string(),
                    serde_json::to_value(v)
                        .expect("HashMap<String, String> serialization cannot fail"),
                );
            }
            if let Some(v) = self.ordering_key {
                payload.insert("ordering_key".to_string(), serde_json::Value::String(v));
            }
            if let Some(v) = self.topic {
                payload.insert("topic".to_string(), serde_json::Value::String(v));
            }

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "gcp-pubsub",
                "publish",
                serde_json::Value::Object(payload),
            );

            if let Some(key) = self.dedup_key {
                action.dedup_key = Some(key);
            }

            action
        }
    }

    /// Builder for a GCP `Pub/Sub` `publish_batch` action.
    #[derive(Debug)]
    pub struct PubSubPublishBatchBuilder {
        namespace: String,
        tenant: String,
        messages: Vec<serde_json::Value>,
        topic: Option<String>,
        dedup_key: Option<String>,
    }

    impl PubSubPublishBatchBuilder {
        /// Set the list of messages to publish as a batch.
        #[must_use]
        pub fn messages(mut self, messages: Vec<serde_json::Value>) -> Self {
            self.messages = messages;
            self
        }

        /// Override the topic name configured on the provider.
        #[must_use]
        pub fn topic(mut self, topic: impl Into<String>) -> Self {
            self.topic = Some(topic.into());
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the GCP `Pub/Sub` `publish_batch` Action.
        ///
        /// # Panics
        ///
        /// Panics if `messages` is empty.
        pub fn build(self) -> Action {
            assert!(
                !self.messages.is_empty(),
                "Pub/Sub messages must not be empty"
            );

            let mut payload = serde_json::Map::new();
            payload.insert(
                "messages".to_string(),
                serde_json::to_value(&self.messages).expect("Vec<Value> serialization cannot fail"),
            );

            if let Some(v) = self.topic {
                payload.insert("topic".to_string(), serde_json::Value::String(v));
            }

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "gcp-pubsub",
                "publish_batch",
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
// Cloud Storage
// =============================================================================

/// Helpers for creating GCP Cloud Storage actions.
pub mod storage {
    use acteon_core::Action;
    use std::collections::HashMap;

    /// Create a builder for a GCP Cloud Storage `upload_object` action.
    pub fn upload(namespace: impl Into<String>, tenant: impl Into<String>) -> StorageUploadBuilder {
        StorageUploadBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            object_name: String::new(),
            bucket: None,
            body: None,
            body_base64: None,
            content_type: None,
            metadata: None,
            dedup_key: None,
        }
    }

    /// Create a builder for a GCP Cloud Storage `download_object` action.
    pub fn download(
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> StorageDownloadBuilder {
        StorageDownloadBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            object_name: String::new(),
            bucket: None,
            dedup_key: None,
        }
    }

    /// Create a builder for a GCP Cloud Storage `delete_object` action.
    pub fn delete(namespace: impl Into<String>, tenant: impl Into<String>) -> StorageDeleteBuilder {
        StorageDeleteBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            object_name: String::new(),
            bucket: None,
            dedup_key: None,
        }
    }

    /// Builder for a GCP Cloud Storage `upload_object` action.
    #[derive(Debug)]
    pub struct StorageUploadBuilder {
        namespace: String,
        tenant: String,
        object_name: String,
        bucket: Option<String>,
        body: Option<String>,
        body_base64: Option<String>,
        content_type: Option<String>,
        metadata: Option<HashMap<String, String>>,
        dedup_key: Option<String>,
    }

    impl StorageUploadBuilder {
        /// Set the object name.
        #[must_use]
        pub fn object_name(mut self, name: impl Into<String>) -> Self {
            self.object_name = name.into();
            self
        }

        /// Override the bucket name configured on the provider.
        #[must_use]
        pub fn bucket(mut self, bucket: impl Into<String>) -> Self {
            self.bucket = Some(bucket.into());
            self
        }

        /// Set the object body as a UTF-8 string.
        #[must_use]
        pub fn body(mut self, body: impl Into<String>) -> Self {
            self.body = Some(body.into());
            self
        }

        /// Set the object body as base64-encoded bytes.
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

        /// Set object metadata as key-value string pairs.
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

        /// Build the GCP Cloud Storage `upload_object` Action.
        ///
        /// # Panics
        ///
        /// Panics if `object_name` has not been set.
        pub fn build(self) -> Action {
            assert!(
                !self.object_name.is_empty(),
                "GCP Storage object_name must be set"
            );

            let mut payload = serde_json::Map::new();
            payload.insert(
                "object_name".to_string(),
                serde_json::Value::String(self.object_name),
            );

            if let Some(v) = self.bucket {
                payload.insert("bucket".to_string(), serde_json::Value::String(v));
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
                "gcp-storage",
                "upload_object",
                serde_json::Value::Object(payload),
            );

            if let Some(key) = self.dedup_key {
                action.dedup_key = Some(key);
            }

            action
        }
    }

    /// Builder for a GCP Cloud Storage `download_object` action.
    #[derive(Debug)]
    pub struct StorageDownloadBuilder {
        namespace: String,
        tenant: String,
        object_name: String,
        bucket: Option<String>,
        dedup_key: Option<String>,
    }

    impl StorageDownloadBuilder {
        /// Set the object name.
        #[must_use]
        pub fn object_name(mut self, name: impl Into<String>) -> Self {
            self.object_name = name.into();
            self
        }

        /// Override the bucket name configured on the provider.
        #[must_use]
        pub fn bucket(mut self, bucket: impl Into<String>) -> Self {
            self.bucket = Some(bucket.into());
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the GCP Cloud Storage `download_object` Action.
        ///
        /// # Panics
        ///
        /// Panics if `object_name` has not been set.
        pub fn build(self) -> Action {
            assert!(
                !self.object_name.is_empty(),
                "GCP Storage object_name must be set"
            );

            let mut payload = serde_json::Map::new();
            payload.insert(
                "object_name".to_string(),
                serde_json::Value::String(self.object_name),
            );

            if let Some(v) = self.bucket {
                payload.insert("bucket".to_string(), serde_json::Value::String(v));
            }

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "gcp-storage",
                "download_object",
                serde_json::Value::Object(payload),
            );

            if let Some(key) = self.dedup_key {
                action.dedup_key = Some(key);
            }

            action
        }
    }

    /// Builder for a GCP Cloud Storage `delete_object` action.
    #[derive(Debug)]
    pub struct StorageDeleteBuilder {
        namespace: String,
        tenant: String,
        object_name: String,
        bucket: Option<String>,
        dedup_key: Option<String>,
    }

    impl StorageDeleteBuilder {
        /// Set the object name.
        #[must_use]
        pub fn object_name(mut self, name: impl Into<String>) -> Self {
            self.object_name = name.into();
            self
        }

        /// Override the bucket name configured on the provider.
        #[must_use]
        pub fn bucket(mut self, bucket: impl Into<String>) -> Self {
            self.bucket = Some(bucket.into());
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the GCP Cloud Storage `delete_object` Action.
        ///
        /// # Panics
        ///
        /// Panics if `object_name` has not been set.
        pub fn build(self) -> Action {
            assert!(
                !self.object_name.is_empty(),
                "GCP Storage object_name must be set"
            );

            let mut payload = serde_json::Map::new();
            payload.insert(
                "object_name".to_string(),
                serde_json::Value::String(self.object_name),
            );

            if let Some(v) = self.bucket {
                payload.insert("bucket".to_string(), serde_json::Value::String(v));
            }

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "gcp-storage",
                "delete_object",
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
    // Pub/Sub tests
    // =========================================================================

    #[test]
    fn pubsub_publish_basic() {
        let a = pubsub::publish("ns", "t1")
            .data(r#"{"hello":"world"}"#)
            .build();

        assert_eq!(a.provider.as_str(), "gcp-pubsub");
        assert_eq!(a.action_type, "publish");
        assert_eq!(a.payload["data"], r#"{"hello":"world"}"#);
    }

    #[test]
    fn pubsub_publish_with_all_options() {
        let mut attrs = HashMap::new();
        attrs.insert("env".to_string(), "staging".to_string());

        let a = pubsub::publish("ns", "t1")
            .data("hello")
            .data_base64("SGVsbG8gV29ybGQ=")
            .attributes(attrs)
            .ordering_key("order-1")
            .topic("my-topic")
            .dedup_key("pub-dedup")
            .build();

        assert_eq!(a.payload["data"], "hello");
        assert_eq!(a.payload["data_base64"], "SGVsbG8gV29ybGQ=");
        assert_eq!(a.payload["attributes"]["env"], "staging");
        assert_eq!(a.payload["ordering_key"], "order-1");
        assert_eq!(a.payload["topic"], "my-topic");
        assert_eq!(a.dedup_key.as_deref(), Some("pub-dedup"));
    }

    #[test]
    fn pubsub_publish_minimal() {
        let a = pubsub::publish("ns", "t1").build();

        assert_eq!(a.provider.as_str(), "gcp-pubsub");
        assert_eq!(a.action_type, "publish");
    }

    #[test]
    fn pubsub_publish_batch_basic() {
        let messages = vec![
            serde_json::json!({"data": "msg1"}),
            serde_json::json!({"data": "msg2"}),
        ];

        let a = pubsub::publish_batch("ns", "t1").messages(messages).build();

        assert_eq!(a.provider.as_str(), "gcp-pubsub");
        assert_eq!(a.action_type, "publish_batch");
        assert_eq!(a.payload["messages"][0]["data"], "msg1");
        assert_eq!(a.payload["messages"][1]["data"], "msg2");
    }

    #[test]
    fn pubsub_publish_batch_with_topic() {
        let messages = vec![serde_json::json!({"data": "msg1"})];

        let a = pubsub::publish_batch("ns", "t1")
            .messages(messages)
            .topic("my-topic")
            .dedup_key("batch-dedup")
            .build();

        assert_eq!(a.payload["topic"], "my-topic");
        assert_eq!(a.dedup_key.as_deref(), Some("batch-dedup"));
    }

    #[test]
    #[should_panic(expected = "Pub/Sub messages must not be empty")]
    fn pubsub_publish_batch_panics_without_messages() {
        pubsub::publish_batch("ns", "t1").build();
    }

    // =========================================================================
    // Cloud Storage tests
    // =========================================================================

    #[test]
    fn storage_upload_basic() {
        let a = storage::upload("ns", "t1")
            .object_name("data.json")
            .body(r#"{"hello":"world"}"#)
            .build();

        assert_eq!(a.provider.as_str(), "gcp-storage");
        assert_eq!(a.action_type, "upload_object");
        assert_eq!(a.payload["object_name"], "data.json");
        assert_eq!(a.payload["body"], r#"{"hello":"world"}"#);
    }

    #[test]
    fn storage_upload_with_all_options() {
        let mut meta = HashMap::new();
        meta.insert("env".to_string(), "staging".to_string());

        let a = storage::upload("ns", "t1")
            .object_name("data.bin")
            .bucket("my-bucket")
            .body_base64("SGVsbG8gV29ybGQ=")
            .content_type("application/octet-stream")
            .metadata(meta)
            .dedup_key("upload-dedup")
            .build();

        assert_eq!(a.payload["object_name"], "data.bin");
        assert_eq!(a.payload["bucket"], "my-bucket");
        assert_eq!(a.payload["body_base64"], "SGVsbG8gV29ybGQ=");
        assert_eq!(a.payload["content_type"], "application/octet-stream");
        assert_eq!(a.payload["metadata"]["env"], "staging");
        assert_eq!(a.dedup_key.as_deref(), Some("upload-dedup"));
    }

    #[test]
    fn storage_download_basic() {
        let a = storage::download("ns", "t1")
            .object_name("data.json")
            .build();

        assert_eq!(a.provider.as_str(), "gcp-storage");
        assert_eq!(a.action_type, "download_object");
        assert_eq!(a.payload["object_name"], "data.json");
    }

    #[test]
    fn storage_download_with_bucket() {
        let a = storage::download("ns", "t1")
            .object_name("data.json")
            .bucket("my-bucket")
            .build();

        assert_eq!(a.payload["bucket"], "my-bucket");
    }

    #[test]
    fn storage_delete_basic() {
        let a = storage::delete("ns", "t1").object_name("data.json").build();

        assert_eq!(a.provider.as_str(), "gcp-storage");
        assert_eq!(a.action_type, "delete_object");
        assert_eq!(a.payload["object_name"], "data.json");
    }

    #[test]
    fn storage_delete_with_bucket() {
        let a = storage::delete("ns", "t1")
            .object_name("data.json")
            .bucket("my-bucket")
            .dedup_key("del-dedup")
            .build();

        assert_eq!(a.payload["bucket"], "my-bucket");
        assert_eq!(a.dedup_key.as_deref(), Some("del-dedup"));
    }

    #[test]
    #[should_panic(expected = "GCP Storage object_name must be set")]
    fn storage_upload_panics_without_object_name() {
        storage::upload("ns", "t1").build();
    }

    #[test]
    #[should_panic(expected = "GCP Storage object_name must be set")]
    fn storage_download_panics_without_object_name() {
        storage::download("ns", "t1").build();
    }

    #[test]
    #[should_panic(expected = "GCP Storage object_name must be set")]
    fn storage_delete_panics_without_object_name() {
        storage::delete("ns", "t1").build();
    }
}
