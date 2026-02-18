use std::collections::HashMap;

use acteon_core::{Action, ProviderResponse};
use acteon_provider::ProviderError;
use acteon_provider::provider::Provider;
use base64::Engine;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, instrument};

use crate::auth::build_sdk_config;
use crate::config::AwsBaseConfig;
use crate::error::classify_sdk_error;

/// Configuration for the AWS S3 provider.
#[derive(Clone, Serialize, Deserialize)]
pub struct S3Config {
    /// Shared AWS configuration (region, role ARN, endpoint URL).
    #[serde(flatten)]
    pub aws: AwsBaseConfig,

    /// Default S3 bucket name. Can be overridden per-action in the payload.
    pub bucket: Option<String>,

    /// Default key prefix for all objects (e.g. `"acteon/artifacts/"`).
    pub prefix: Option<String>,
}

impl std::fmt::Debug for S3Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Config")
            .field("aws", &self.aws)
            .field("bucket", &self.bucket)
            .field("prefix", &self.prefix)
            .finish()
    }
}

impl S3Config {
    /// Create a new `S3Config` with the given AWS region.
    pub fn new(region: impl Into<String>) -> Self {
        Self {
            aws: AwsBaseConfig::new(region),
            bucket: None,
            prefix: None,
        }
    }

    /// Set the default bucket name.
    #[must_use]
    pub fn with_bucket(mut self, bucket: impl Into<String>) -> Self {
        self.bucket = Some(bucket.into());
        self
    }

    /// Set the default key prefix.
    #[must_use]
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
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

/// Payload for the `put_object` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3PutPayload {
    /// S3 bucket name. Overrides config default.
    pub bucket: Option<String>,

    /// Object key (path within the bucket).
    pub key: String,

    /// Object body as a UTF-8 string.
    /// Mutually exclusive with `body_base64`.
    pub body: Option<String>,

    /// Object body as base64-encoded bytes.
    /// Mutually exclusive with `body`.
    pub body_base64: Option<String>,

    /// Optional `Content-Type` header for the object.
    pub content_type: Option<String>,

    /// Optional metadata key-value pairs attached to the object.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Payload for the `get_object` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3GetPayload {
    /// S3 bucket name. Overrides config default.
    pub bucket: Option<String>,

    /// Object key to retrieve.
    pub key: String,
}

/// Payload for the `delete_object` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3DeletePayload {
    /// S3 bucket name. Overrides config default.
    pub bucket: Option<String>,

    /// Object key to delete.
    pub key: String,
}

/// AWS S3 provider for storing and retrieving objects.
pub struct S3Provider {
    config: S3Config,
    client: aws_sdk_s3::Client,
}

impl std::fmt::Debug for S3Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Provider")
            .field("config", &self.config)
            .field("client", &"<S3Client>")
            .finish()
    }
}

impl S3Provider {
    /// Create a new `S3Provider` by building an AWS SDK client.
    pub async fn new(config: S3Config) -> Self {
        let sdk_config = build_sdk_config(&config.aws).await;
        let client = aws_sdk_s3::Client::new(&sdk_config);
        Self { config, client }
    }

    /// Create an `S3Provider` with a pre-built client (for testing).
    pub fn with_client(config: S3Config, client: aws_sdk_s3::Client) -> Self {
        Self { config, client }
    }

    /// Resolve the bucket name from the payload or config default.
    fn resolve_bucket<'a>(&'a self, payload_bucket: Option<&'a str>) -> Option<&'a str> {
        payload_bucket.or(self.config.bucket.as_deref())
    }

    /// Apply the configured prefix to a key.
    fn prefixed_key(&self, key: &str) -> String {
        match &self.config.prefix {
            Some(prefix) => format!("{prefix}{key}"),
            None => key.to_owned(),
        }
    }
}

impl Provider for S3Provider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "aws-s3"
    }

    #[instrument(skip(self, action), fields(action_id = %action.id, provider = "aws-s3"))]
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        match action.action_type.as_str() {
            "put_object" => self.put_object(action).await,
            "get_object" => self.get_object(action).await,
            "delete_object" => self.delete_object(action).await,
            other => Err(ProviderError::Configuration(format!(
                "unknown S3 action type '{other}' (expected 'put_object', 'get_object', or 'delete_object')"
            ))),
        }
    }

    #[instrument(skip(self), fields(provider = "aws-s3"))]
    async fn health_check(&self) -> Result<(), ProviderError> {
        debug!("performing S3 health check");
        self.client
            .list_buckets()
            .max_buckets(1)
            .send()
            .await
            .map_err(|e| {
                error!(error = %e, "S3 health check failed");
                ProviderError::Connection(format!("S3 health check failed: {e}"))
            })?;
        info!("S3 health check passed");
        Ok(())
    }
}

impl S3Provider {
    async fn put_object(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing S3 put_object payload");
        let payload: S3PutPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let bucket = self
            .resolve_bucket(payload.bucket.as_deref())
            .ok_or_else(|| {
                ProviderError::Configuration("no bucket in payload or provider config".to_owned())
            })?;

        let key = self.prefixed_key(&payload.key);

        // Resolve the body from either string or base64.
        let body_bytes: Vec<u8> = if let Some(ref b64) = payload.body_base64 {
            base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| ProviderError::Serialization(format!("invalid base64 body: {e}")))?
        } else if let Some(ref text) = payload.body {
            text.as_bytes().to_vec()
        } else {
            Vec::new()
        };

        debug!(bucket = %bucket, key = %key, size = body_bytes.len(), "uploading object to S3");

        let mut request = self
            .client
            .put_object()
            .bucket(bucket)
            .key(&key)
            .body(aws_sdk_s3::primitives::ByteStream::from(body_bytes));

        if let Some(ref ct) = payload.content_type {
            request = request.content_type(ct);
        }

        for (mk, mv) in &payload.metadata {
            request = request.metadata(mk, mv);
        }

        request.send().await.map_err(|e| {
            let err_str = e.to_string();
            error!(error = %err_str, "S3 put_object failed");
            let aws_err: ProviderError = classify_sdk_error(&err_str).into();
            aws_err
        })?;

        info!(bucket = %bucket, key = %key, "S3 object uploaded");

        Ok(ProviderResponse::success(serde_json::json!({
            "bucket": bucket,
            "key": key,
            "status": "uploaded"
        })))
    }

    async fn get_object(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing S3 get_object payload");
        let payload: S3GetPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let bucket = self
            .resolve_bucket(payload.bucket.as_deref())
            .ok_or_else(|| {
                ProviderError::Configuration("no bucket in payload or provider config".to_owned())
            })?;

        let key = self.prefixed_key(&payload.key);

        debug!(bucket = %bucket, key = %key, "downloading object from S3");

        let result = self
            .client
            .get_object()
            .bucket(bucket)
            .key(&key)
            .send()
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                error!(error = %err_str, "S3 get_object failed");
                let aws_err: ProviderError = classify_sdk_error(&err_str).into();
                aws_err
            })?;

        let content_type = result
            .content_type()
            .unwrap_or("application/octet-stream")
            .to_owned();
        let content_length = result.content_length().unwrap_or(0);

        // Read the body as bytes and return as a UTF-8 string if possible,
        // otherwise base64-encode it.
        let body_bytes = result
            .body
            .collect()
            .await
            .map_err(|e| ProviderError::ExecutionFailed(format!("failed to read S3 body: {e}")))?
            .into_bytes();

        let (body_field, body_value) = match std::str::from_utf8(&body_bytes) {
            Ok(text) => ("body", serde_json::Value::String(text.to_owned())),
            Err(_) => (
                "body_base64",
                serde_json::Value::String(
                    base64::engine::general_purpose::STANDARD.encode(&body_bytes),
                ),
            ),
        };

        info!(bucket = %bucket, key = %key, content_length = content_length, "S3 object downloaded");

        Ok(ProviderResponse::success(serde_json::json!({
            "bucket": bucket,
            "key": key,
            "content_type": content_type,
            "content_length": content_length,
            body_field: body_value,
            "status": "downloaded"
        })))
    }

    async fn delete_object(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing S3 delete_object payload");
        let payload: S3DeletePayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let bucket = self
            .resolve_bucket(payload.bucket.as_deref())
            .ok_or_else(|| {
                ProviderError::Configuration("no bucket in payload or provider config".to_owned())
            })?;

        let key = self.prefixed_key(&payload.key);

        debug!(bucket = %bucket, key = %key, "deleting object from S3");

        self.client
            .delete_object()
            .bucket(bucket)
            .key(&key)
            .send()
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                error!(error = %err_str, "S3 delete_object failed");
                let aws_err: ProviderError = classify_sdk_error(&err_str).into();
                aws_err
            })?;

        info!(bucket = %bucket, key = %key, "S3 object deleted");

        Ok(ProviderResponse::success(serde_json::json!({
            "bucket": bucket,
            "key": key,
            "status": "deleted"
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_new_sets_region() {
        let config = S3Config::new("us-west-2");
        assert_eq!(config.aws.region, "us-west-2");
        assert!(config.bucket.is_none());
        assert!(config.prefix.is_none());
    }

    #[test]
    fn config_with_bucket() {
        let config = S3Config::new("us-east-1").with_bucket("my-bucket");
        assert_eq!(config.bucket.as_deref(), Some("my-bucket"));
    }

    #[test]
    fn config_with_prefix() {
        let config = S3Config::new("us-east-1").with_prefix("acteon/");
        assert_eq!(config.prefix.as_deref(), Some("acteon/"));
    }

    #[test]
    fn config_builder_chain() {
        let config = S3Config::new("eu-west-1")
            .with_bucket("data-bucket")
            .with_prefix("logs/")
            .with_endpoint_url("http://localhost:4566")
            .with_role_arn("arn:aws:iam::123:role/s3-access")
            .with_session_name("test-session")
            .with_external_id("ext-123");
        assert_eq!(config.bucket.as_deref(), Some("data-bucket"));
        assert_eq!(config.prefix.as_deref(), Some("logs/"));
        assert_eq!(
            config.aws.endpoint_url.as_deref(),
            Some("http://localhost:4566")
        );
        assert!(config.aws.role_arn.is_some());
        assert_eq!(config.aws.session_name.as_deref(), Some("test-session"));
        assert_eq!(config.aws.external_id.as_deref(), Some("ext-123"));
    }

    #[test]
    fn config_debug_format() {
        let config = S3Config::new("us-east-1")
            .with_bucket("my-bucket")
            .with_role_arn("arn:aws:iam::123:role/test");
        let debug = format!("{config:?}");
        assert!(debug.contains("S3Config"));
        assert!(debug.contains("[REDACTED]"));
        assert!(debug.contains("my-bucket"));
    }

    #[test]
    fn config_serde_roundtrip() {
        let config = S3Config::new("ap-southeast-1")
            .with_bucket("archive-bucket")
            .with_prefix("data/");

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: S3Config = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.aws.region, "ap-southeast-1");
        assert_eq!(deserialized.bucket.as_deref(), Some("archive-bucket"));
        assert_eq!(deserialized.prefix.as_deref(), Some("data/"));
    }

    #[test]
    fn deserialize_put_payload() {
        let json = serde_json::json!({
            "key": "reports/2026/report.json",
            "body": "{\"total\": 42}",
            "content_type": "application/json",
            "bucket": "my-bucket",
            "metadata": {
                "source": "acteon"
            }
        });
        let payload: S3PutPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.key, "reports/2026/report.json");
        assert_eq!(payload.body.as_deref(), Some("{\"total\": 42}"));
        assert_eq!(payload.content_type.as_deref(), Some("application/json"));
        assert_eq!(payload.bucket.as_deref(), Some("my-bucket"));
        assert_eq!(payload.metadata.get("source").unwrap(), "acteon");
    }

    #[test]
    fn deserialize_put_payload_base64() {
        let json = serde_json::json!({
            "key": "images/logo.png",
            "body_base64": "iVBORw0KGgo=",
            "content_type": "image/png"
        });
        let payload: S3PutPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.key, "images/logo.png");
        assert!(payload.body.is_none());
        assert_eq!(payload.body_base64.as_deref(), Some("iVBORw0KGgo="));
    }

    #[test]
    fn deserialize_minimal_put_payload() {
        let json = serde_json::json!({
            "key": "test.txt"
        });
        let payload: S3PutPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.key, "test.txt");
        assert!(payload.body.is_none());
        assert!(payload.body_base64.is_none());
        assert!(payload.content_type.is_none());
        assert!(payload.bucket.is_none());
        assert!(payload.metadata.is_empty());
    }

    #[test]
    fn deserialize_get_payload() {
        let json = serde_json::json!({
            "key": "reports/latest.json",
            "bucket": "data-bucket"
        });
        let payload: S3GetPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.key, "reports/latest.json");
        assert_eq!(payload.bucket.as_deref(), Some("data-bucket"));
    }

    #[test]
    fn deserialize_delete_payload() {
        let json = serde_json::json!({
            "key": "old/data.csv"
        });
        let payload: S3DeletePayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.key, "old/data.csv");
        assert!(payload.bucket.is_none());
    }
}
