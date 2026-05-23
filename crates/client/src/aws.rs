//! Convenience helpers for creating AWS-targeted actions.
//!
//! Provides builder types for each supported AWS provider:
//! [`sns`], [`lambda`], [`eventbridge`], [`sqs`], [`s3`], [`ec2`], and [`autoscaling`].
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

// =============================================================================
// EC2
// =============================================================================

/// Helpers for creating EC2 instance lifecycle actions.
pub mod ec2 {
    use acteon_core::Action;
    use std::collections::HashMap;

    /// Create a builder for an EC2 `start_instances` action.
    pub fn start_instances(
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> Ec2StartBuilder {
        Ec2StartBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            instance_ids: Vec::new(),
            dedup_key: None,
        }
    }

    /// Builder for an EC2 `start_instances` action.
    #[derive(Debug)]
    pub struct Ec2StartBuilder {
        namespace: String,
        tenant: String,
        instance_ids: Vec<String>,
        dedup_key: Option<String>,
    }

    impl Ec2StartBuilder {
        /// Set the instance IDs to start.
        #[must_use]
        pub fn instance_ids(mut self, ids: Vec<String>) -> Self {
            self.instance_ids = ids;
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the EC2 `start_instances` Action.
        ///
        /// # Panics
        ///
        /// Panics if `instance_ids` is empty.
        pub fn build(self) -> Action {
            assert!(
                !self.instance_ids.is_empty(),
                "EC2 instance_ids must not be empty"
            );

            let mut payload = serde_json::Map::new();
            payload.insert(
                "instance_ids".to_string(),
                serde_json::to_value(&self.instance_ids)
                    .expect("Vec<String> serialization cannot fail"),
            );

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "aws-ec2",
                "start_instances",
                serde_json::Value::Object(payload),
            );

            if let Some(key) = self.dedup_key {
                action.dedup_key = Some(key);
            }

            action
        }
    }

    /// Create a builder for an EC2 `stop_instances` action.
    pub fn stop_instances(
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> Ec2StopBuilder {
        Ec2StopBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            instance_ids: Vec::new(),
            hibernate: None,
            force: None,
            dedup_key: None,
        }
    }

    /// Builder for an EC2 `stop_instances` action.
    #[derive(Debug)]
    pub struct Ec2StopBuilder {
        namespace: String,
        tenant: String,
        instance_ids: Vec<String>,
        hibernate: Option<bool>,
        force: Option<bool>,
        dedup_key: Option<String>,
    }

    impl Ec2StopBuilder {
        /// Set the instance IDs to stop.
        #[must_use]
        pub fn instance_ids(mut self, ids: Vec<String>) -> Self {
            self.instance_ids = ids;
            self
        }

        /// Whether to hibernate the instances instead of stopping them.
        #[must_use]
        pub fn hibernate(mut self, hibernate: bool) -> Self {
            self.hibernate = Some(hibernate);
            self
        }

        /// Whether to force the instances to stop without a graceful shutdown.
        #[must_use]
        pub fn force(mut self, force: bool) -> Self {
            self.force = Some(force);
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the EC2 `stop_instances` Action.
        ///
        /// # Panics
        ///
        /// Panics if `instance_ids` is empty.
        pub fn build(self) -> Action {
            assert!(
                !self.instance_ids.is_empty(),
                "EC2 instance_ids must not be empty"
            );

            let mut payload = serde_json::Map::new();
            payload.insert(
                "instance_ids".to_string(),
                serde_json::to_value(&self.instance_ids)
                    .expect("Vec<String> serialization cannot fail"),
            );

            if let Some(v) = self.hibernate {
                payload.insert("hibernate".to_string(), serde_json::Value::Bool(v));
            }
            if let Some(v) = self.force {
                payload.insert("force".to_string(), serde_json::Value::Bool(v));
            }

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "aws-ec2",
                "stop_instances",
                serde_json::Value::Object(payload),
            );

            if let Some(key) = self.dedup_key {
                action.dedup_key = Some(key);
            }

            action
        }
    }

    /// Create a builder for an EC2 `reboot_instances` action.
    pub fn reboot_instances(
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> Ec2RebootBuilder {
        Ec2RebootBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            instance_ids: Vec::new(),
            dedup_key: None,
        }
    }

    /// Builder for an EC2 `reboot_instances` action.
    #[derive(Debug)]
    pub struct Ec2RebootBuilder {
        namespace: String,
        tenant: String,
        instance_ids: Vec<String>,
        dedup_key: Option<String>,
    }

    impl Ec2RebootBuilder {
        /// Set the instance IDs to reboot.
        #[must_use]
        pub fn instance_ids(mut self, ids: Vec<String>) -> Self {
            self.instance_ids = ids;
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the EC2 `reboot_instances` Action.
        ///
        /// # Panics
        ///
        /// Panics if `instance_ids` is empty.
        pub fn build(self) -> Action {
            assert!(
                !self.instance_ids.is_empty(),
                "EC2 instance_ids must not be empty"
            );

            let mut payload = serde_json::Map::new();
            payload.insert(
                "instance_ids".to_string(),
                serde_json::to_value(&self.instance_ids)
                    .expect("Vec<String> serialization cannot fail"),
            );

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "aws-ec2",
                "reboot_instances",
                serde_json::Value::Object(payload),
            );

            if let Some(key) = self.dedup_key {
                action.dedup_key = Some(key);
            }

            action
        }
    }

    /// Create a builder for an EC2 `terminate_instances` action.
    pub fn terminate_instances(
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> Ec2TerminateBuilder {
        Ec2TerminateBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            instance_ids: Vec::new(),
            dedup_key: None,
        }
    }

    /// Builder for an EC2 `terminate_instances` action.
    #[derive(Debug)]
    pub struct Ec2TerminateBuilder {
        namespace: String,
        tenant: String,
        instance_ids: Vec<String>,
        dedup_key: Option<String>,
    }

    impl Ec2TerminateBuilder {
        /// Set the instance IDs to terminate.
        #[must_use]
        pub fn instance_ids(mut self, ids: Vec<String>) -> Self {
            self.instance_ids = ids;
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the EC2 `terminate_instances` Action.
        ///
        /// # Panics
        ///
        /// Panics if `instance_ids` is empty.
        pub fn build(self) -> Action {
            assert!(
                !self.instance_ids.is_empty(),
                "EC2 instance_ids must not be empty"
            );

            let mut payload = serde_json::Map::new();
            payload.insert(
                "instance_ids".to_string(),
                serde_json::to_value(&self.instance_ids)
                    .expect("Vec<String> serialization cannot fail"),
            );

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "aws-ec2",
                "terminate_instances",
                serde_json::Value::Object(payload),
            );

            if let Some(key) = self.dedup_key {
                action.dedup_key = Some(key);
            }

            action
        }
    }

    /// Create a builder for an EC2 `hibernate_instances` action.
    pub fn hibernate_instances(
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> Ec2HibernateBuilder {
        Ec2HibernateBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            instance_ids: Vec::new(),
            dedup_key: None,
        }
    }

    /// Builder for an EC2 `hibernate_instances` action.
    #[derive(Debug)]
    pub struct Ec2HibernateBuilder {
        namespace: String,
        tenant: String,
        instance_ids: Vec<String>,
        dedup_key: Option<String>,
    }

    impl Ec2HibernateBuilder {
        /// Set the instance IDs to hibernate.
        #[must_use]
        pub fn instance_ids(mut self, ids: Vec<String>) -> Self {
            self.instance_ids = ids;
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the EC2 `hibernate_instances` Action.
        ///
        /// # Panics
        ///
        /// Panics if `instance_ids` is empty.
        pub fn build(self) -> Action {
            assert!(
                !self.instance_ids.is_empty(),
                "EC2 instance_ids must not be empty"
            );

            let mut payload = serde_json::Map::new();
            payload.insert(
                "instance_ids".to_string(),
                serde_json::to_value(&self.instance_ids)
                    .expect("Vec<String> serialization cannot fail"),
            );

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "aws-ec2",
                "hibernate_instances",
                serde_json::Value::Object(payload),
            );

            if let Some(key) = self.dedup_key {
                action.dedup_key = Some(key);
            }

            action
        }
    }

    /// Create a builder for an EC2 `run_instances` action.
    pub fn run_instances(namespace: impl Into<String>, tenant: impl Into<String>) -> Ec2RunBuilder {
        Ec2RunBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            image_id: String::new(),
            instance_type: String::new(),
            min_count: None,
            max_count: None,
            key_name: None,
            security_group_ids: None,
            subnet_id: None,
            user_data: None,
            tags: None,
            iam_instance_profile: None,
            dedup_key: None,
        }
    }

    /// Builder for an EC2 `run_instances` action.
    #[derive(Debug)]
    pub struct Ec2RunBuilder {
        namespace: String,
        tenant: String,
        image_id: String,
        instance_type: String,
        min_count: Option<i32>,
        max_count: Option<i32>,
        key_name: Option<String>,
        security_group_ids: Option<Vec<String>>,
        subnet_id: Option<String>,
        user_data: Option<String>,
        tags: Option<HashMap<String, String>>,
        iam_instance_profile: Option<String>,
        dedup_key: Option<String>,
    }

    impl Ec2RunBuilder {
        /// Set the AMI ID.
        #[must_use]
        pub fn image_id(mut self, image_id: impl Into<String>) -> Self {
            self.image_id = image_id.into();
            self
        }

        /// Set the instance type (e.g., `"t3.micro"`).
        #[must_use]
        pub fn instance_type(mut self, instance_type: impl Into<String>) -> Self {
            self.instance_type = instance_type.into();
            self
        }

        /// Set the minimum number of instances to launch.
        #[must_use]
        pub fn min_count(mut self, count: i32) -> Self {
            self.min_count = Some(count);
            self
        }

        /// Set the maximum number of instances to launch.
        #[must_use]
        pub fn max_count(mut self, count: i32) -> Self {
            self.max_count = Some(count);
            self
        }

        /// Set the key-pair name.
        #[must_use]
        pub fn key_name(mut self, key_name: impl Into<String>) -> Self {
            self.key_name = Some(key_name.into());
            self
        }

        /// Set the security group IDs.
        #[must_use]
        pub fn security_group_ids(mut self, ids: Vec<String>) -> Self {
            self.security_group_ids = Some(ids);
            self
        }

        /// Set the subnet ID.
        #[must_use]
        pub fn subnet_id(mut self, subnet_id: impl Into<String>) -> Self {
            self.subnet_id = Some(subnet_id.into());
            self
        }

        /// Set the base64-encoded user data script.
        #[must_use]
        pub fn user_data(mut self, user_data: impl Into<String>) -> Self {
            self.user_data = Some(user_data.into());
            self
        }

        /// Set tags to apply to the launched instances.
        #[must_use]
        pub fn tags(mut self, tags: HashMap<String, String>) -> Self {
            self.tags = Some(tags);
            self
        }

        /// Set the IAM instance profile name or ARN.
        #[must_use]
        pub fn iam_instance_profile(mut self, profile: impl Into<String>) -> Self {
            self.iam_instance_profile = Some(profile.into());
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the EC2 `run_instances` Action.
        ///
        /// # Panics
        ///
        /// Panics if `image_id` or `instance_type` have not been set.
        pub fn build(self) -> Action {
            assert!(
                !self.image_id.is_empty(),
                "EC2 image_id must be set for run_instances"
            );
            assert!(
                !self.instance_type.is_empty(),
                "EC2 instance_type must be set for run_instances"
            );

            let mut payload = serde_json::Map::new();
            payload.insert(
                "image_id".to_string(),
                serde_json::Value::String(self.image_id),
            );
            payload.insert(
                "instance_type".to_string(),
                serde_json::Value::String(self.instance_type),
            );

            if let Some(v) = self.min_count {
                payload.insert("min_count".to_string(), serde_json::Value::Number(v.into()));
            }
            if let Some(v) = self.max_count {
                payload.insert("max_count".to_string(), serde_json::Value::Number(v.into()));
            }
            if let Some(v) = self.key_name {
                payload.insert("key_name".to_string(), serde_json::Value::String(v));
            }
            if let Some(v) = self.security_group_ids {
                payload.insert(
                    "security_group_ids".to_string(),
                    serde_json::to_value(v).expect("Vec<String> serialization cannot fail"),
                );
            }
            if let Some(v) = self.subnet_id {
                payload.insert("subnet_id".to_string(), serde_json::Value::String(v));
            }
            if let Some(v) = self.user_data {
                payload.insert("user_data".to_string(), serde_json::Value::String(v));
            }
            if let Some(v) = self.tags {
                payload.insert(
                    "tags".to_string(),
                    serde_json::to_value(v)
                        .expect("HashMap<String, String> serialization cannot fail"),
                );
            }
            if let Some(v) = self.iam_instance_profile {
                payload.insert(
                    "iam_instance_profile".to_string(),
                    serde_json::Value::String(v),
                );
            }

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "aws-ec2",
                "run_instances",
                serde_json::Value::Object(payload),
            );

            if let Some(key) = self.dedup_key {
                action.dedup_key = Some(key);
            }

            action
        }
    }

    /// Create a builder for an EC2 `attach_volume` action.
    pub fn attach_volume(
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> Ec2AttachVolumeBuilder {
        Ec2AttachVolumeBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            volume_id: String::new(),
            instance_id: String::new(),
            device: String::new(),
            dedup_key: None,
        }
    }

    /// Builder for an EC2 `attach_volume` action.
    #[derive(Debug)]
    pub struct Ec2AttachVolumeBuilder {
        namespace: String,
        tenant: String,
        volume_id: String,
        instance_id: String,
        device: String,
        dedup_key: Option<String>,
    }

    impl Ec2AttachVolumeBuilder {
        /// Set the EBS volume ID.
        #[must_use]
        pub fn volume_id(mut self, volume_id: impl Into<String>) -> Self {
            self.volume_id = volume_id.into();
            self
        }

        /// Set the EC2 instance ID to attach to.
        #[must_use]
        pub fn instance_id(mut self, instance_id: impl Into<String>) -> Self {
            self.instance_id = instance_id.into();
            self
        }

        /// Set the device name (e.g., `"/dev/sdf"`).
        #[must_use]
        pub fn device(mut self, device: impl Into<String>) -> Self {
            self.device = device.into();
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the EC2 `attach_volume` Action.
        ///
        /// # Panics
        ///
        /// Panics if `volume_id`, `instance_id`, or `device` have not been set.
        pub fn build(self) -> Action {
            assert!(
                !self.volume_id.is_empty(),
                "EC2 volume_id must be set for attach_volume"
            );
            assert!(
                !self.instance_id.is_empty(),
                "EC2 instance_id must be set for attach_volume"
            );
            assert!(
                !self.device.is_empty(),
                "EC2 device must be set for attach_volume"
            );

            let mut payload = serde_json::Map::new();
            payload.insert(
                "volume_id".to_string(),
                serde_json::Value::String(self.volume_id),
            );
            payload.insert(
                "instance_id".to_string(),
                serde_json::Value::String(self.instance_id),
            );
            payload.insert("device".to_string(), serde_json::Value::String(self.device));

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "aws-ec2",
                "attach_volume",
                serde_json::Value::Object(payload),
            );

            if let Some(key) = self.dedup_key {
                action.dedup_key = Some(key);
            }

            action
        }
    }

    /// Create a builder for an EC2 `detach_volume` action.
    pub fn detach_volume(
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> Ec2DetachVolumeBuilder {
        Ec2DetachVolumeBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            volume_id: String::new(),
            instance_id: None,
            device: None,
            force: None,
            dedup_key: None,
        }
    }

    /// Builder for an EC2 `detach_volume` action.
    #[derive(Debug)]
    pub struct Ec2DetachVolumeBuilder {
        namespace: String,
        tenant: String,
        volume_id: String,
        instance_id: Option<String>,
        device: Option<String>,
        force: Option<bool>,
        dedup_key: Option<String>,
    }

    impl Ec2DetachVolumeBuilder {
        /// Set the EBS volume ID.
        #[must_use]
        pub fn volume_id(mut self, volume_id: impl Into<String>) -> Self {
            self.volume_id = volume_id.into();
            self
        }

        /// Set the EC2 instance ID to detach from.
        #[must_use]
        pub fn instance_id(mut self, instance_id: impl Into<String>) -> Self {
            self.instance_id = Some(instance_id.into());
            self
        }

        /// Set the device name.
        #[must_use]
        pub fn device(mut self, device: impl Into<String>) -> Self {
            self.device = Some(device.into());
            self
        }

        /// Whether to force the detachment.
        #[must_use]
        pub fn force(mut self, force: bool) -> Self {
            self.force = Some(force);
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the EC2 `detach_volume` Action.
        ///
        /// # Panics
        ///
        /// Panics if `volume_id` has not been set.
        pub fn build(self) -> Action {
            assert!(
                !self.volume_id.is_empty(),
                "EC2 volume_id must be set for detach_volume"
            );

            let mut payload = serde_json::Map::new();
            payload.insert(
                "volume_id".to_string(),
                serde_json::Value::String(self.volume_id),
            );

            if let Some(v) = self.instance_id {
                payload.insert("instance_id".to_string(), serde_json::Value::String(v));
            }
            if let Some(v) = self.device {
                payload.insert("device".to_string(), serde_json::Value::String(v));
            }
            if let Some(v) = self.force {
                payload.insert("force".to_string(), serde_json::Value::Bool(v));
            }

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "aws-ec2",
                "detach_volume",
                serde_json::Value::Object(payload),
            );

            if let Some(key) = self.dedup_key {
                action.dedup_key = Some(key);
            }

            action
        }
    }

    /// Create a builder for an EC2 `describe_instances` action.
    pub fn describe_instances(
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> Ec2DescribeBuilder {
        Ec2DescribeBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            instance_ids: None,
            dedup_key: None,
        }
    }

    /// Builder for an EC2 `describe_instances` action.
    #[derive(Debug)]
    pub struct Ec2DescribeBuilder {
        namespace: String,
        tenant: String,
        instance_ids: Option<Vec<String>>,
        dedup_key: Option<String>,
    }

    impl Ec2DescribeBuilder {
        /// Set the instance IDs to describe. If not set, describes all instances.
        #[must_use]
        pub fn instance_ids(mut self, ids: Vec<String>) -> Self {
            self.instance_ids = Some(ids);
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the EC2 `describe_instances` Action.
        pub fn build(self) -> Action {
            let mut payload = serde_json::Map::new();

            if let Some(ids) = self.instance_ids {
                payload.insert(
                    "instance_ids".to_string(),
                    serde_json::to_value(ids).expect("Vec<String> serialization cannot fail"),
                );
            }

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "aws-ec2",
                "describe_instances",
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
// Auto Scaling
// =============================================================================

/// Helpers for creating Auto Scaling Group actions.
pub mod autoscaling {
    use acteon_core::Action;

    /// Create a builder for an Auto Scaling `describe_auto_scaling_groups` action.
    pub fn describe_groups(
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> AsgDescribeBuilder {
        AsgDescribeBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            group_names: None,
            dedup_key: None,
        }
    }

    /// Builder for an Auto Scaling `describe_auto_scaling_groups` action.
    #[derive(Debug)]
    pub struct AsgDescribeBuilder {
        namespace: String,
        tenant: String,
        group_names: Option<Vec<String>>,
        dedup_key: Option<String>,
    }

    impl AsgDescribeBuilder {
        /// Set the Auto Scaling Group names to describe.
        #[must_use]
        pub fn group_names(mut self, names: Vec<String>) -> Self {
            self.group_names = Some(names);
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the Auto Scaling `describe_auto_scaling_groups` Action.
        pub fn build(self) -> Action {
            let mut payload = serde_json::Map::new();

            if let Some(names) = self.group_names {
                payload.insert(
                    "auto_scaling_group_names".to_string(),
                    serde_json::to_value(names).expect("Vec<String> serialization cannot fail"),
                );
            }

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "aws-autoscaling",
                "describe_auto_scaling_groups",
                serde_json::Value::Object(payload),
            );

            if let Some(key) = self.dedup_key {
                action.dedup_key = Some(key);
            }

            action
        }
    }

    /// Create a builder for an Auto Scaling `set_desired_capacity` action.
    pub fn set_desired_capacity(
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> AsgSetCapacityBuilder {
        AsgSetCapacityBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            group_name: String::new(),
            desired_capacity: 0,
            honor_cooldown: None,
            dedup_key: None,
        }
    }

    /// Builder for an Auto Scaling `set_desired_capacity` action.
    #[derive(Debug)]
    pub struct AsgSetCapacityBuilder {
        namespace: String,
        tenant: String,
        group_name: String,
        desired_capacity: i32,
        honor_cooldown: Option<bool>,
        dedup_key: Option<String>,
    }

    impl AsgSetCapacityBuilder {
        /// Set the Auto Scaling Group name.
        #[must_use]
        pub fn group_name(mut self, name: impl Into<String>) -> Self {
            self.group_name = name.into();
            self
        }

        /// Set the desired capacity.
        #[must_use]
        pub fn desired_capacity(mut self, capacity: i32) -> Self {
            self.desired_capacity = capacity;
            self
        }

        /// Whether to honor the group's cooldown period.
        #[must_use]
        pub fn honor_cooldown(mut self, honor: bool) -> Self {
            self.honor_cooldown = Some(honor);
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the Auto Scaling `set_desired_capacity` Action.
        ///
        /// # Panics
        ///
        /// Panics if `group_name` has not been set.
        pub fn build(self) -> Action {
            assert!(
                !self.group_name.is_empty(),
                "Auto Scaling group_name must be set"
            );

            let mut payload = serde_json::Map::new();
            payload.insert(
                "auto_scaling_group_name".to_string(),
                serde_json::Value::String(self.group_name),
            );
            payload.insert(
                "desired_capacity".to_string(),
                serde_json::Value::Number(self.desired_capacity.into()),
            );

            if let Some(v) = self.honor_cooldown {
                payload.insert("honor_cooldown".to_string(), serde_json::Value::Bool(v));
            }

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "aws-autoscaling",
                "set_desired_capacity",
                serde_json::Value::Object(payload),
            );

            if let Some(key) = self.dedup_key {
                action.dedup_key = Some(key);
            }

            action
        }
    }

    /// Create a builder for an Auto Scaling `update_auto_scaling_group` action.
    pub fn update_group(
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> AsgUpdateBuilder {
        AsgUpdateBuilder {
            namespace: namespace.into(),
            tenant: tenant.into(),
            group_name: String::new(),
            min_size: None,
            max_size: None,
            desired_capacity: None,
            default_cooldown: None,
            health_check_type: None,
            health_check_grace_period: None,
            dedup_key: None,
        }
    }

    /// Builder for an Auto Scaling `update_auto_scaling_group` action.
    #[derive(Debug)]
    pub struct AsgUpdateBuilder {
        namespace: String,
        tenant: String,
        group_name: String,
        min_size: Option<i32>,
        max_size: Option<i32>,
        desired_capacity: Option<i32>,
        default_cooldown: Option<i32>,
        health_check_type: Option<String>,
        health_check_grace_period: Option<i32>,
        dedup_key: Option<String>,
    }

    impl AsgUpdateBuilder {
        /// Set the Auto Scaling Group name.
        #[must_use]
        pub fn group_name(mut self, name: impl Into<String>) -> Self {
            self.group_name = name.into();
            self
        }

        /// Set the new minimum size.
        #[must_use]
        pub fn min_size(mut self, size: i32) -> Self {
            self.min_size = Some(size);
            self
        }

        /// Set the new maximum size.
        #[must_use]
        pub fn max_size(mut self, size: i32) -> Self {
            self.max_size = Some(size);
            self
        }

        /// Set the new desired capacity.
        #[must_use]
        pub fn desired_capacity(mut self, capacity: i32) -> Self {
            self.desired_capacity = Some(capacity);
            self
        }

        /// Set the new default cooldown period in seconds.
        #[must_use]
        pub fn default_cooldown(mut self, cooldown: i32) -> Self {
            self.default_cooldown = Some(cooldown);
            self
        }

        /// Set the new health check type (`"EC2"` or `"ELB"`).
        #[must_use]
        pub fn health_check_type(mut self, hct: impl Into<String>) -> Self {
            self.health_check_type = Some(hct.into());
            self
        }

        /// Set the new health check grace period in seconds.
        #[must_use]
        pub fn health_check_grace_period(mut self, grace: i32) -> Self {
            self.health_check_grace_period = Some(grace);
            self
        }

        /// Set a deduplication key on the Action.
        #[must_use]
        pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
            self.dedup_key = Some(key.into());
            self
        }

        /// Build the Auto Scaling `update_auto_scaling_group` Action.
        ///
        /// # Panics
        ///
        /// Panics if `group_name` has not been set.
        pub fn build(self) -> Action {
            assert!(
                !self.group_name.is_empty(),
                "Auto Scaling group_name must be set"
            );

            let mut payload = serde_json::Map::new();
            payload.insert(
                "auto_scaling_group_name".to_string(),
                serde_json::Value::String(self.group_name),
            );

            if let Some(v) = self.min_size {
                payload.insert("min_size".to_string(), serde_json::Value::Number(v.into()));
            }
            if let Some(v) = self.max_size {
                payload.insert("max_size".to_string(), serde_json::Value::Number(v.into()));
            }
            if let Some(v) = self.desired_capacity {
                payload.insert(
                    "desired_capacity".to_string(),
                    serde_json::Value::Number(v.into()),
                );
            }
            if let Some(v) = self.default_cooldown {
                payload.insert(
                    "default_cooldown".to_string(),
                    serde_json::Value::Number(v.into()),
                );
            }
            if let Some(v) = self.health_check_type {
                payload.insert(
                    "health_check_type".to_string(),
                    serde_json::Value::String(v),
                );
            }
            if let Some(v) = self.health_check_grace_period {
                payload.insert(
                    "health_check_grace_period".to_string(),
                    serde_json::Value::Number(v.into()),
                );
            }

            let mut action = Action::new(
                self.namespace,
                self.tenant,
                "aws-autoscaling",
                "update_auto_scaling_group",
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
    use super::{autoscaling, ec2, eventbridge, lambda, s3, sns, sqs};
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

    // =========================================================================
    // EC2 tests
    // =========================================================================

    #[test]
    fn ec2_start_instances_basic() {
        let a = ec2::start_instances("ns", "t1")
            .instance_ids(vec!["i-abc123".to_string(), "i-def456".to_string()])
            .build();

        assert_eq!(a.provider.as_str(), "aws-ec2");
        assert_eq!(a.action_type, "start_instances");
        assert_eq!(a.payload["instance_ids"][0], "i-abc123");
        assert_eq!(a.payload["instance_ids"][1], "i-def456");
    }

    #[test]
    fn ec2_stop_instances_with_options() {
        let a = ec2::stop_instances("ns", "t1")
            .instance_ids(vec!["i-abc123".to_string()])
            .hibernate(true)
            .force(true)
            .build();

        assert_eq!(a.provider.as_str(), "aws-ec2");
        assert_eq!(a.action_type, "stop_instances");
        assert_eq!(a.payload["instance_ids"][0], "i-abc123");
        assert_eq!(a.payload["hibernate"], true);
        assert_eq!(a.payload["force"], true);
    }

    #[test]
    fn ec2_run_instances_basic() {
        let a = ec2::run_instances("ns", "t1")
            .image_id("ami-12345678")
            .instance_type("t3.micro")
            .build();

        assert_eq!(a.provider.as_str(), "aws-ec2");
        assert_eq!(a.action_type, "run_instances");
        assert_eq!(a.payload["image_id"], "ami-12345678");
        assert_eq!(a.payload["instance_type"], "t3.micro");
    }

    #[test]
    fn ec2_run_instances_with_all_options() {
        let mut tags = HashMap::new();
        tags.insert("env".to_string(), "staging".to_string());

        let a = ec2::run_instances("ns", "t1")
            .image_id("ami-12345678")
            .instance_type("t3.micro")
            .min_count(2)
            .max_count(5)
            .key_name("my-key")
            .security_group_ids(vec!["sg-111".to_string(), "sg-222".to_string()])
            .subnet_id("subnet-aaa")
            .user_data("IyEvYmluL2Jhc2g=")
            .tags(tags)
            .iam_instance_profile("arn:aws:iam::123:instance-profile/app")
            .dedup_key("run-dedup")
            .build();

        assert_eq!(a.payload["min_count"], 2);
        assert_eq!(a.payload["max_count"], 5);
        assert_eq!(a.payload["key_name"], "my-key");
        assert_eq!(a.payload["security_group_ids"][0], "sg-111");
        assert_eq!(a.payload["security_group_ids"][1], "sg-222");
        assert_eq!(a.payload["subnet_id"], "subnet-aaa");
        assert_eq!(a.payload["user_data"], "IyEvYmluL2Jhc2g=");
        assert_eq!(a.payload["tags"]["env"], "staging");
        assert_eq!(
            a.payload["iam_instance_profile"],
            "arn:aws:iam::123:instance-profile/app"
        );
        assert_eq!(a.dedup_key.as_deref(), Some("run-dedup"));
    }

    #[test]
    fn ec2_attach_volume() {
        let a = ec2::attach_volume("ns", "t1")
            .volume_id("vol-abc123")
            .instance_id("i-def456")
            .device("/dev/sdf")
            .build();

        assert_eq!(a.provider.as_str(), "aws-ec2");
        assert_eq!(a.action_type, "attach_volume");
        assert_eq!(a.payload["volume_id"], "vol-abc123");
        assert_eq!(a.payload["instance_id"], "i-def456");
        assert_eq!(a.payload["device"], "/dev/sdf");
    }

    #[test]
    fn ec2_detach_volume() {
        let a = ec2::detach_volume("ns", "t1")
            .volume_id("vol-abc123")
            .instance_id("i-def456")
            .device("/dev/sdf")
            .force(true)
            .build();

        assert_eq!(a.provider.as_str(), "aws-ec2");
        assert_eq!(a.action_type, "detach_volume");
        assert_eq!(a.payload["volume_id"], "vol-abc123");
        assert_eq!(a.payload["instance_id"], "i-def456");
        assert_eq!(a.payload["device"], "/dev/sdf");
        assert_eq!(a.payload["force"], true);
    }

    #[test]
    fn ec2_describe_instances() {
        let a = ec2::describe_instances("ns", "t1")
            .instance_ids(vec!["i-abc123".to_string()])
            .build();

        assert_eq!(a.provider.as_str(), "aws-ec2");
        assert_eq!(a.action_type, "describe_instances");
        assert_eq!(a.payload["instance_ids"][0], "i-abc123");
    }

    #[test]
    fn ec2_hibernate_instances() {
        let a = ec2::hibernate_instances("ns", "t1")
            .instance_ids(vec!["i-abc123".to_string()])
            .build();

        assert_eq!(a.provider.as_str(), "aws-ec2");
        assert_eq!(a.action_type, "hibernate_instances");
        assert_eq!(a.payload["instance_ids"][0], "i-abc123");
    }

    #[test]
    fn ec2_reboot_instances() {
        let a = ec2::reboot_instances("ns", "t1")
            .instance_ids(vec!["i-abc123".to_string()])
            .build();

        assert_eq!(a.provider.as_str(), "aws-ec2");
        assert_eq!(a.action_type, "reboot_instances");
        assert_eq!(a.payload["instance_ids"][0], "i-abc123");
    }

    #[test]
    fn ec2_terminate_instances() {
        let a = ec2::terminate_instances("ns", "t1")
            .instance_ids(vec!["i-abc123".to_string()])
            .build();

        assert_eq!(a.provider.as_str(), "aws-ec2");
        assert_eq!(a.action_type, "terminate_instances");
        assert_eq!(a.payload["instance_ids"][0], "i-abc123");
    }

    #[test]
    #[should_panic(expected = "EC2 instance_ids must not be empty")]
    fn ec2_panics_without_instance_ids() {
        ec2::start_instances("ns", "t1").build();
    }

    #[test]
    #[should_panic(expected = "EC2 image_id must be set")]
    fn ec2_run_panics_without_image_id() {
        ec2::run_instances("ns", "t1")
            .instance_type("t3.micro")
            .build();
    }

    // =========================================================================
    // Auto Scaling tests
    // =========================================================================

    #[test]
    fn asg_describe_groups() {
        let a = autoscaling::describe_groups("ns", "t1")
            .group_names(vec!["my-asg".to_string()])
            .build();

        assert_eq!(a.provider.as_str(), "aws-autoscaling");
        assert_eq!(a.action_type, "describe_auto_scaling_groups");
        assert_eq!(a.payload["auto_scaling_group_names"][0], "my-asg");
    }

    #[test]
    fn asg_set_desired_capacity() {
        let a = autoscaling::set_desired_capacity("ns", "t1")
            .group_name("my-asg")
            .desired_capacity(5)
            .honor_cooldown(true)
            .build();

        assert_eq!(a.provider.as_str(), "aws-autoscaling");
        assert_eq!(a.action_type, "set_desired_capacity");
        assert_eq!(a.payload["auto_scaling_group_name"], "my-asg");
        assert_eq!(a.payload["desired_capacity"], 5);
        assert_eq!(a.payload["honor_cooldown"], true);
    }

    #[test]
    fn asg_update_group() {
        let a = autoscaling::update_group("ns", "t1")
            .group_name("my-asg")
            .min_size(2)
            .max_size(10)
            .desired_capacity(5)
            .default_cooldown(300)
            .health_check_type("ELB")
            .health_check_grace_period(120)
            .build();

        assert_eq!(a.provider.as_str(), "aws-autoscaling");
        assert_eq!(a.action_type, "update_auto_scaling_group");
        assert_eq!(a.payload["auto_scaling_group_name"], "my-asg");
        assert_eq!(a.payload["min_size"], 2);
        assert_eq!(a.payload["max_size"], 10);
        assert_eq!(a.payload["desired_capacity"], 5);
        assert_eq!(a.payload["default_cooldown"], 300);
        assert_eq!(a.payload["health_check_type"], "ELB");
        assert_eq!(a.payload["health_check_grace_period"], 120);
    }

    #[test]
    #[should_panic(expected = "Auto Scaling group_name must be set")]
    fn asg_panics_without_group_name() {
        autoscaling::set_desired_capacity("ns", "t1")
            .desired_capacity(5)
            .build();
    }
}
