//! Convenience helpers for creating AWS-targeted actions.
//!
//! Provides builder types for each supported AWS provider:
//! [`sns`], [`lambda`], [`eventbridge`], [`sqs`], and [`s3`].
//!
//! # Example
//!
//! ```no_run
//! use acteon_client::aws;
//!
//! // Publish to SNS
//! let action = aws::sns::publish("notifications", "tenant-1")
//!     .message("Server alert: CPU high")
//!     .subject("Alert")
//!     .build();
//!
//! // Invoke Lambda
//! let action = aws::lambda::invoke("compute", "tenant-1")
//!     .payload(serde_json::json!({"key": "value"}))
//!     .build();
//!
//! // Put to S3
//! let action = aws::s3::put_object("storage", "tenant-1")
//!     .bucket("my-bucket")
//!     .key("path/to/object.json")
//!     .body(r#"{"hello":"world"}"#)
//!     .content_type("application/json")
//!     .build();
//! ```

// =============================================================================
// SNS
// =============================================================================

/// Helpers for creating SNS actions.
pub mod sns {
    use acteon_core::Action;

    /// Create a builder for an SNS publish action.
    pub fn publish(namespace: impl Into<String>, tenant: impl Into<String>) -> SnsPublishBuilder {
        SnsPublishBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            message: String::new(),
            subject: None,
            topic_arn: None,
            message_group_id: None,
            message_dedup_id: None,
            dedup_key: None,
        }
    }

    /// Builder for an SNS publish action.
    #[derive(Debug)]
    pub struct SnsPublishBuilder {
        namespace: String,
        tenant: String,
        message: String,
        subject: Option<String>,
        topic_arn: Option<String>,
        message_group_id: Option<String>,
        message_dedup_id: Option<String>,
        dedup_key: Option<String>,
    }

    impl SnsPublishBuilder {
        /// Set the message body.
        #[must_use]
        pub fn message(mut self, message: impl Into<String>) -> Self {
            self.message = message.into();
            self
        }

        /// Set the message subject (for email-protocol subscriptions).
        #[must_use]
        pub fn subject(mut self, subject: impl Into<String>) -> Self {
            self.subject = Some(subject.into());
            self
        }

        /// Override the topic ARN configured on the provider.
        #[must_use]
        pub fn topic_arn(mut self, arn: impl Into<String>) -> Self {
            self.topic_arn = Some(arn.into());
            self
        }

        /// Set the message group ID (for FIFO topics).
        #[must_use]
        pub fn message_group_id(mut self, id: impl Into<String>) -> Self {
            self.message_group_id = Some(id.into());
            self
        }

        /// Set the message deduplication ID (for FIFO topics).
        #[must_use]
        pub fn message_dedup_id(mut self, id: impl Into<String>) -> Self {
            self.message_dedup_id = Some(id.into());
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the SNS publish Action.
        ///
        /// # Panics
        ///
        /// Panics if `message` has not been set.
        pub fn build(self) -> Action {
            assert!(!self.message.is_empty(), "SNS message must be set");

            let mut payload = serde_json::Map::new();
            payload.insert(
                "message".to_string(),
                serde_json::Value::String(self.message),
            );

            if let Some(v) = self.subject {
                payload.insert("subject".to_string(), serde_json::Value::String(v));
            }
            if let Some(v) = self.topic_arn {
                payload.insert("topic_arn".to_string(), serde_json::Value::String(v));
            }
            if let Some(v) = self.message_group_id {
                payload.insert("message_group_id".to_string(), serde_json::Value::String(v));
            }
            if let Some(v) = self.message_dedup_id {
                payload.insert("message_dedup_id".to_string(), serde_json::Value::String(v));
            }

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "aws-sns",
                "publish",
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
// Lambda
// =============================================================================

/// Helpers for creating Lambda actions.
pub mod lambda {
    use acteon_core::Action;

    /// Create a builder for a Lambda invoke action.
    pub fn invoke(namespace: impl Into<String>, tenant: impl Into<String>) -> LambdaInvokeBuilder {
        LambdaInvokeBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            function_name: None,
            payload: serde_json::Value::Object(serde_json::Map::new()),
            invocation_type: None,
            dedup_key: None,
        }
    }

    /// Builder for a Lambda invoke action.
    #[derive(Debug)]
    pub struct LambdaInvokeBuilder {
        namespace: String,
        tenant: String,
        function_name: Option<String>,
        payload: serde_json::Value,
        invocation_type: Option<String>,
        dedup_key: Option<String>,
    }

    impl LambdaInvokeBuilder {
        /// Override the function name configured on the provider.
        #[must_use]
        pub fn function_name(mut self, name: impl Into<String>) -> Self {
            self.function_name = Some(name.into());
            self
        }

        /// Set the JSON payload to pass to the Lambda function.
        #[must_use]
        pub fn payload(mut self, payload: serde_json::Value) -> Self {
            self.payload = payload;
            self
        }

        /// Set the invocation type: `"RequestResponse"`, `"Event"`, or `"DryRun"`.
        #[must_use]
        pub fn invocation_type(mut self, typ: impl Into<String>) -> Self {
            self.invocation_type = Some(typ.into());
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the Lambda invoke Action.
        pub fn build(self) -> Action {
            let mut payload = serde_json::Map::new();
            payload.insert("payload".to_string(), self.payload);

            if let Some(v) = self.function_name {
                payload.insert("function_name".to_string(), serde_json::Value::String(v));
            }
            if let Some(v) = self.invocation_type {
                payload.insert("invocation_type".to_string(), serde_json::Value::String(v));
            }

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "aws-lambda",
                "invoke",
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
// EventBridge
// =============================================================================

/// Helpers for creating `EventBridge` actions.
pub mod eventbridge {
    use acteon_core::Action;

    /// Create a builder for an `EventBridge` put-event action.
    pub fn put_event(
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> EventBridgePutBuilder {
        EventBridgePutBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            source: String::new(),
            detail_type: String::new(),
            detail: serde_json::Value::Object(serde_json::Map::new()),
            event_bus_name: None,
            resources: None,
            dedup_key: None,
        }
    }

    /// Builder for an `EventBridge` put-event action.
    #[derive(Debug)]
    pub struct EventBridgePutBuilder {
        namespace: String,
        tenant: String,
        source: String,
        detail_type: String,
        detail: serde_json::Value,
        event_bus_name: Option<String>,
        resources: Option<Vec<String>>,
        dedup_key: Option<String>,
    }

    impl EventBridgePutBuilder {
        /// Set the event source (e.g., `"com.myapp.orders"`).
        #[must_use]
        pub fn source(mut self, source: impl Into<String>) -> Self {
            self.source = source.into();
            self
        }

        /// Set the detail type (e.g., `"OrderCreated"`).
        #[must_use]
        pub fn detail_type(mut self, detail_type: impl Into<String>) -> Self {
            self.detail_type = detail_type.into();
            self
        }

        /// Set the event detail as a JSON value.
        #[must_use]
        pub fn detail(mut self, detail: serde_json::Value) -> Self {
            self.detail = detail;
            self
        }

        /// Override the event bus name configured on the provider.
        #[must_use]
        pub fn event_bus_name(mut self, name: impl Into<String>) -> Self {
            self.event_bus_name = Some(name.into());
            self
        }

        /// Set the list of resource ARNs associated with this event.
        #[must_use]
        pub fn resources(mut self, resources: Vec<String>) -> Self {
            self.resources = Some(resources);
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the `EventBridge` put-event Action.
        ///
        /// # Panics
        ///
        /// Panics if `source` or `detail_type` have not been set.
        pub fn build(self) -> Action {
            assert!(!self.source.is_empty(), "EventBridge source must be set");
            assert!(
                !self.detail_type.is_empty(),
                "EventBridge detail_type must be set"
            );

            let mut payload = serde_json::Map::new();
            payload.insert("source".to_string(), serde_json::Value::String(self.source));
            payload.insert(
                "detail_type".to_string(),
                serde_json::Value::String(self.detail_type),
            );
            payload.insert("detail".to_string(), self.detail);

            if let Some(v) = self.event_bus_name {
                payload.insert("event_bus_name".to_string(), serde_json::Value::String(v));
            }
            if let Some(v) = self.resources {
                payload.insert(
                    "resources".to_string(),
                    serde_json::to_value(v).expect("Vec<String> serialization cannot fail"),
                );
            }

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "aws-eventbridge",
                "put_event",
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
// SQS
// =============================================================================

/// Helpers for creating SQS actions.
pub mod sqs {
    use acteon_core::Action;
    use std::collections::HashMap;

    /// Create a builder for an SQS send-message action.
    pub fn send_message(namespace: impl Into<String>, tenant: impl Into<String>) -> SqsSendBuilder {
        SqsSendBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            message_body: String::new(),
            queue_url: None,
            delay_seconds: None,
            message_group_id: None,
            message_dedup_id: None,
            message_attributes: None,
            dedup_key: None,
        }
    }

    /// Builder for an SQS send-message action.
    #[derive(Debug)]
    pub struct SqsSendBuilder {
        namespace: String,
        tenant: String,
        message_body: String,
        queue_url: Option<String>,
        delay_seconds: Option<u32>,
        message_group_id: Option<String>,
        message_dedup_id: Option<String>,
        message_attributes: Option<HashMap<String, String>>,
        dedup_key: Option<String>,
    }

    impl SqsSendBuilder {
        /// Set the message body.
        #[must_use]
        pub fn message_body(mut self, body: impl Into<String>) -> Self {
            self.message_body = body.into();
            self
        }

        /// Override the queue URL configured on the provider.
        #[must_use]
        pub fn queue_url(mut self, url: impl Into<String>) -> Self {
            self.queue_url = Some(url.into());
            self
        }

        /// Set the message delivery delay in seconds (0-900).
        #[must_use]
        pub fn delay_seconds(mut self, seconds: u32) -> Self {
            self.delay_seconds = Some(seconds);
            self
        }

        /// Set the message group ID (for FIFO queues).
        #[must_use]
        pub fn message_group_id(mut self, id: impl Into<String>) -> Self {
            self.message_group_id = Some(id.into());
            self
        }

        /// Set the message deduplication ID (for FIFO queues).
        #[must_use]
        pub fn message_dedup_id(mut self, id: impl Into<String>) -> Self {
            self.message_dedup_id = Some(id.into());
            self
        }

        /// Set message attributes as key-value string pairs.
        #[must_use]
        pub fn message_attributes(mut self, attrs: HashMap<String, String>) -> Self {
            self.message_attributes = Some(attrs);
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the SQS send-message Action.
        ///
        /// # Panics
        ///
        /// Panics if `message_body` has not been set.
        pub fn build(self) -> Action {
            assert!(
                !self.message_body.is_empty(),
                "SQS message_body must be set"
            );

            let mut payload = serde_json::Map::new();
            payload.insert(
                "message_body".to_string(),
                serde_json::Value::String(self.message_body),
            );

            if let Some(v) = self.queue_url {
                payload.insert("queue_url".to_string(), serde_json::Value::String(v));
            }
            if let Some(v) = self.delay_seconds {
                payload.insert(
                    "delay_seconds".to_string(),
                    serde_json::Value::Number(v.into()),
                );
            }
            if let Some(v) = self.message_group_id {
                payload.insert("message_group_id".to_string(), serde_json::Value::String(v));
            }
            if let Some(v) = self.message_dedup_id {
                payload.insert("message_dedup_id".to_string(), serde_json::Value::String(v));
            }
            if let Some(v) = self.message_attributes {
                payload.insert(
                    "message_attributes".to_string(),
                    serde_json::to_value(v)
                        .expect("HashMap<String, String> serialization cannot fail"),
                );
            }

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "aws-sqs",
                "send_message",
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
// S3
// =============================================================================

/// Helpers for creating S3 actions.
pub mod s3 {
    use acteon_core::Action;
    use std::collections::HashMap;

    /// Create a builder for an S3 put-object action.
    pub fn put_object(
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> S3PutObjectBuilder {
        S3PutObjectBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            bucket: None,
            key: String::new(),
            body: None,
            body_base64: None,
            content_type: None,
            metadata: None,
            dedup_key: None,
        }
    }

    /// Create a builder for an S3 get-object action.
    pub fn get_object(
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> S3GetObjectBuilder {
        S3GetObjectBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            bucket: None,
            key: String::new(),
            dedup_key: None,
        }
    }

    /// Create a builder for an S3 delete-object action.
    pub fn delete_object(
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> S3DeleteObjectBuilder {
        S3DeleteObjectBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            bucket: None,
            key: String::new(),
            dedup_key: None,
        }
    }

    /// Builder for an S3 put-object action.
    #[derive(Debug)]
    pub struct S3PutObjectBuilder {
        namespace: String,
        tenant: String,
        bucket: Option<String>,
        key: String,
        body: Option<String>,
        body_base64: Option<String>,
        content_type: Option<String>,
        metadata: Option<HashMap<String, String>>,
        dedup_key: Option<String>,
    }

    impl S3PutObjectBuilder {
        /// Override the bucket name configured on the provider.
        #[must_use]
        pub fn bucket(mut self, bucket: impl Into<String>) -> Self {
            self.bucket = Some(bucket.into());
            self
        }

        /// Set the object key.
        #[must_use]
        pub fn key(mut self, key: impl Into<String>) -> Self {
            self.key = key.into();
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

        /// Build the S3 put-object Action.
        ///
        /// # Panics
        ///
        /// Panics if `key` has not been set.
        pub fn build(self) -> Action {
            assert!(!self.key.is_empty(), "S3 object key must be set");

            let mut payload = serde_json::Map::new();
            payload.insert("key".to_string(), serde_json::Value::String(self.key));

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
                "aws-s3",
                "put_object",
                serde_json::Value::Object(payload),
            );

            if let Some(key) = self.dedup_key {
                action.dedup_key = Some(key);
            }

            action
        }
    }

    /// Builder for an S3 get-object action.
    #[derive(Debug)]
    pub struct S3GetObjectBuilder {
        namespace: String,
        tenant: String,
        bucket: Option<String>,
        key: String,
        dedup_key: Option<String>,
    }

    impl S3GetObjectBuilder {
        /// Override the bucket name configured on the provider.
        #[must_use]
        pub fn bucket(mut self, bucket: impl Into<String>) -> Self {
            self.bucket = Some(bucket.into());
            self
        }

        /// Set the object key.
        #[must_use]
        pub fn key(mut self, key: impl Into<String>) -> Self {
            self.key = key.into();
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the S3 get-object Action.
        ///
        /// # Panics
        ///
        /// Panics if `key` has not been set.
        pub fn build(self) -> Action {
            assert!(!self.key.is_empty(), "S3 object key must be set");

            let mut payload = serde_json::Map::new();
            payload.insert("key".to_string(), serde_json::Value::String(self.key));

            if let Some(v) = self.bucket {
                payload.insert("bucket".to_string(), serde_json::Value::String(v));
            }

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "aws-s3",
                "get_object",
                serde_json::Value::Object(payload),
            );

            if let Some(key) = self.dedup_key {
                action.dedup_key = Some(key);
            }

            action
        }
    }

    /// Builder for an S3 delete-object action.
    #[derive(Debug)]
    pub struct S3DeleteObjectBuilder {
        namespace: String,
        tenant: String,
        bucket: Option<String>,
        key: String,
        dedup_key: Option<String>,
    }

    impl S3DeleteObjectBuilder {
        /// Override the bucket name configured on the provider.
        #[must_use]
        pub fn bucket(mut self, bucket: impl Into<String>) -> Self {
            self.bucket = Some(bucket.into());
            self
        }

        /// Set the object key.
        #[must_use]
        pub fn key(mut self, key: impl Into<String>) -> Self {
            self.key = key.into();
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the S3 delete-object Action.
        ///
        /// # Panics
        ///
        /// Panics if `key` has not been set.
        pub fn build(self) -> Action {
            assert!(!self.key.is_empty(), "S3 object key must be set");

            let mut payload = serde_json::Map::new();
            payload.insert("key".to_string(), serde_json::Value::String(self.key));

            if let Some(v) = self.bucket {
                payload.insert("bucket".to_string(), serde_json::Value::String(v));
            }

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "aws-s3",
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
    use super::{eventbridge, lambda, s3, sns, sqs};
    use std::collections::HashMap;

    #[test]
    fn sns_publish_basic() {
        let a = sns::publish("ns", "t1").message("hello world").build();

        assert_eq!(a.provider.as_str(), "aws-sns");
        assert_eq!(a.action_type, "publish");
        assert_eq!(a.payload["message"], "hello world");
    }

    #[test]
    fn sns_publish_with_options() {
        let a = sns::publish("ns", "t1")
            .message("alert")
            .subject("Important")
            .topic_arn("arn:aws:sns:us-east-1:123456789012:my-topic")
            .message_group_id("group-1")
            .message_dedup_id("dedup-1")
            .dedup_key("action-dedup")
            .build();

        assert_eq!(a.payload["subject"], "Important");
        assert_eq!(
            a.payload["topic_arn"],
            "arn:aws:sns:us-east-1:123456789012:my-topic"
        );
        assert_eq!(a.payload["message_group_id"], "group-1");
        assert_eq!(a.payload["message_dedup_id"], "dedup-1");
        assert_eq!(a.dedup_key.as_deref(), Some("action-dedup"));
    }

    #[test]
    #[should_panic(expected = "SNS message must be set")]
    fn sns_panics_without_message() {
        sns::publish("ns", "t1").build();
    }

    #[test]
    fn lambda_invoke_basic() {
        let a = lambda::invoke("ns", "t1")
            .payload(serde_json::json!({"key": "value"}))
            .build();

        assert_eq!(a.provider.as_str(), "aws-lambda");
        assert_eq!(a.action_type, "invoke");
        assert_eq!(a.payload["payload"]["key"], "value");
    }

    #[test]
    fn lambda_invoke_with_options() {
        let a = lambda::invoke("ns", "t1")
            .function_name("my-function")
            .payload(serde_json::json!({}))
            .invocation_type("Event")
            .build();

        assert_eq!(a.payload["function_name"], "my-function");
        assert_eq!(a.payload["invocation_type"], "Event");
    }

    #[test]
    fn eventbridge_put_event() {
        let a = eventbridge::put_event("ns", "t1")
            .source("com.myapp")
            .detail_type("OrderCreated")
            .detail(serde_json::json!({"order_id": "123"}))
            .resources(vec!["arn:aws:resource:1".to_string()])
            .build();

        assert_eq!(a.provider.as_str(), "aws-eventbridge");
        assert_eq!(a.action_type, "put_event");
        assert_eq!(a.payload["source"], "com.myapp");
        assert_eq!(a.payload["detail_type"], "OrderCreated");
        assert_eq!(a.payload["detail"]["order_id"], "123");
        assert_eq!(a.payload["resources"][0], "arn:aws:resource:1");
    }

    #[test]
    #[should_panic(expected = "EventBridge source must be set")]
    fn eventbridge_panics_without_source() {
        eventbridge::put_event("ns", "t1").detail_type("X").build();
    }

    #[test]
    #[should_panic(expected = "EventBridge detail_type must be set")]
    fn eventbridge_panics_without_detail_type() {
        eventbridge::put_event("ns", "t1").source("src").build();
    }

    #[test]
    fn sqs_send_message_basic() {
        let a = sqs::send_message("ns", "t1")
            .message_body(r#"{"event":"test"}"#)
            .build();

        assert_eq!(a.provider.as_str(), "aws-sqs");
        assert_eq!(a.action_type, "send_message");
        assert_eq!(a.payload["message_body"], r#"{"event":"test"}"#);
    }

    #[test]
    fn sqs_send_message_with_options() {
        let a = sqs::send_message("ns", "t1")
            .message_body("hello")
            .queue_url("https://sqs.us-east-1.amazonaws.com/123456789012/my-queue")
            .delay_seconds(10)
            .message_group_id("g1")
            .message_dedup_id("d1")
            .build();

        assert_eq!(
            a.payload["queue_url"],
            "https://sqs.us-east-1.amazonaws.com/123456789012/my-queue"
        );
        assert_eq!(a.payload["delay_seconds"], 10);
        assert_eq!(a.payload["message_group_id"], "g1");
        assert_eq!(a.payload["message_dedup_id"], "d1");
    }

    #[test]
    #[should_panic(expected = "SQS message_body must be set")]
    fn sqs_panics_without_body() {
        sqs::send_message("ns", "t1").build();
    }

    #[test]
    fn s3_put_object_basic() {
        let a = s3::put_object("ns", "t1")
            .key("path/to/file.txt")
            .body("file contents")
            .build();

        assert_eq!(a.provider.as_str(), "aws-s3");
        assert_eq!(a.action_type, "put_object");
        assert_eq!(a.payload["key"], "path/to/file.txt");
        assert_eq!(a.payload["body"], "file contents");
    }

    #[test]
    fn s3_put_object_with_options() {
        let mut meta = HashMap::new();
        meta.insert("author".to_string(), "test".to_string());

        let a = s3::put_object("ns", "t1")
            .bucket("my-bucket")
            .key("data.json")
            .body(r#"{"key":"value"}"#)
            .content_type("application/json")
            .metadata(meta)
            .build();

        assert_eq!(a.payload["bucket"], "my-bucket");
        assert_eq!(a.payload["content_type"], "application/json");
        assert_eq!(a.payload["metadata"]["author"], "test");
    }

    #[test]
    fn s3_get_object() {
        let a = s3::get_object("ns", "t1")
            .bucket("my-bucket")
            .key("path/file.txt")
            .build();

        assert_eq!(a.action_type, "get_object");
        assert_eq!(a.payload["bucket"], "my-bucket");
        assert_eq!(a.payload["key"], "path/file.txt");
    }

    #[test]
    fn s3_delete_object() {
        let a = s3::delete_object("ns", "t1").key("path/file.txt").build();

        assert_eq!(a.action_type, "delete_object");
        assert_eq!(a.payload["key"], "path/file.txt");
    }

    #[test]
    #[should_panic(expected = "S3 object key must be set")]
    fn s3_panics_without_key() {
        s3::put_object("ns", "t1").build();
    }
}
