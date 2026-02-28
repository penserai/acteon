use std::collections::HashMap;

use acteon_core::{Action, ProviderResponse};
use acteon_provider::ProviderError;
use acteon_provider::provider::Provider;
use base64::Engine;
use google_cloud_storage::client::{Storage, StorageControl};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, instrument};

use crate::config::GcpBaseConfig;
use crate::error::classify_gcp_error;

/// Configuration for the GCP Cloud Storage provider.
#[derive(Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Shared GCP configuration.
    #[serde(flatten)]
    pub gcp: GcpBaseConfig,

    /// Default bucket name. Can be overridden per-action in the payload.
    pub bucket: Option<String>,

    /// Default object name prefix (e.g. `"acteon/artifacts/"`).
    pub prefix: Option<String>,
}

impl std::fmt::Debug for StorageConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageConfig")
            .field("gcp", &self.gcp)
            .field("bucket", &self.bucket)
            .field("prefix", &self.prefix)
            .finish()
    }
}

impl StorageConfig {
    /// Create a new `StorageConfig` with the given GCP project ID.
    pub fn new(project_id: impl Into<String>) -> Self {
        Self {
            gcp: GcpBaseConfig::new(project_id),
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

    /// Set the default object name prefix.
    #[must_use]
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    /// Set the endpoint URL override (for `fake-gcs-server`).
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

/// Payload for the `upload_object` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageUploadPayload {
    /// Bucket name. Overrides config default.
    pub bucket: Option<String>,

    /// Object name (path within the bucket).
    pub object_name: String,

    /// Object body as a UTF-8 string.
    /// Mutually exclusive with `body_base64`.
    pub body: Option<String>,

    /// Object body as base64-encoded bytes.
    /// Mutually exclusive with `body`.
    pub body_base64: Option<String>,

    /// Optional `Content-Type` for the object.
    pub content_type: Option<String>,

    /// Optional metadata key-value pairs attached to the object.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Payload for the `download_object` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageDownloadPayload {
    /// Bucket name. Overrides config default.
    pub bucket: Option<String>,

    /// Object name to download.
    pub object_name: String,
}

/// Payload for the `delete_object` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageDeletePayload {
    /// Bucket name. Overrides config default.
    pub bucket: Option<String>,

    /// Object name to delete.
    pub object_name: String,
}

/// Payload for the `list_objects` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageListPayload {
    /// Bucket name. Overrides config default.
    pub bucket: Option<String>,

    /// Optional prefix to filter objects.
    pub prefix: Option<String>,

    /// Maximum number of results to return.
    pub max_results: Option<i32>,
}

/// GCP Cloud Storage provider for storing and retrieving objects.
pub struct StorageProvider {
    config: StorageConfig,
    storage: Storage,
    storage_control: StorageControl,
}

impl std::fmt::Debug for StorageProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageProvider")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl StorageProvider {
    /// Create a new `StorageProvider` by building Cloud Storage clients.
    pub async fn new(config: StorageConfig) -> Result<Self, ProviderError> {
        let credentials = crate::auth::build_gcp_credentials(
            config.gcp.credentials_path.as_deref(),
            config.gcp.credentials_json.as_deref(),
        )
        .await
        .map_err(|e| ProviderError::Configuration(e.to_string()))?;

        // Build the Storage client (read/write operations).
        let mut storage_builder = Storage::builder();
        if let Some(ref endpoint) = config.gcp.endpoint_url {
            storage_builder = storage_builder.with_endpoint(endpoint);
        }
        if let Some(ref creds) = credentials {
            storage_builder = storage_builder.with_credentials(creds.clone());
        }
        let storage = storage_builder.build().await.map_err(|e| {
            ProviderError::Configuration(format!("Cloud Storage client error: {e}"))
        })?;

        // Build the StorageControl client (delete/list/metadata operations).
        let mut control_builder = StorageControl::builder();
        if let Some(ref endpoint) = config.gcp.endpoint_url {
            control_builder = control_builder.with_endpoint(endpoint);
        }
        if let Some(ref creds) = credentials {
            control_builder = control_builder.with_credentials(creds.clone());
        }
        let storage_control = control_builder.build().await.map_err(|e| {
            ProviderError::Configuration(format!("Cloud Storage control client error: {e}"))
        })?;

        Ok(Self {
            config,
            storage,
            storage_control,
        })
    }

    /// Resolve the bucket name from the payload or config default.
    fn resolve_bucket<'a>(&'a self, payload_bucket: Option<&'a str>) -> Option<&'a str> {
        payload_bucket.or(self.config.bucket.as_deref())
    }

    /// Format a bucket name into the Cloud Storage v2 resource path.
    fn bucket_path(bucket: &str) -> String {
        format!("projects/_/buckets/{bucket}")
    }

    /// Apply the configured prefix to an object name.
    fn prefixed_name(&self, object_name: &str) -> String {
        match &self.config.prefix {
            Some(prefix) => format!("{prefix}{object_name}"),
            None => object_name.to_owned(),
        }
    }
}

impl Provider for StorageProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "gcp-storage"
    }

    #[instrument(skip(self, action), fields(action_id = %action.id, provider = "gcp-storage"))]
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        match action.action_type.as_str() {
            "upload_object" => self.upload_object(action).await,
            "download_object" => self.download_object(action).await,
            "delete_object" => self.delete_object(action).await,
            "list_objects" => self.list_objects(action).await,
            other => Err(ProviderError::Configuration(format!(
                "unknown Cloud Storage action type '{other}' (expected 'upload_object', 'download_object', 'delete_object', or 'list_objects')"
            ))),
        }
    }

    #[instrument(skip(self), fields(provider = "gcp-storage"))]
    async fn health_check(&self) -> Result<(), ProviderError> {
        debug!("performing Cloud Storage health check");
        // Verify connectivity by listing objects (limited to 1).
        if let Some(bucket) = self.config.bucket.as_deref() {
            let bucket_path = Self::bucket_path(bucket);
            self.storage_control
                .list_objects()
                .set_parent(&bucket_path)
                .set_page_size(1_i32)
                .send()
                .await
                .map_err(|e| {
                    error!(error = %e, bucket = %bucket, "Cloud Storage health check failed");
                    ProviderError::Connection(format!("Cloud Storage health check failed: {e}"))
                })?;
        }
        info!("Cloud Storage health check passed");
        Ok(())
    }
}

impl StorageProvider {
    async fn upload_object(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing Cloud Storage upload_object payload");
        let payload: StorageUploadPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let bucket = self
            .resolve_bucket(payload.bucket.as_deref())
            .ok_or_else(|| {
                ProviderError::Configuration("no bucket in payload or provider config".to_owned())
            })?;

        let object_name = self.prefixed_name(&payload.object_name);

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

        let content_length = body_bytes.len();
        let bucket_path = Self::bucket_path(bucket);
        debug!(bucket = %bucket, object_name = %object_name, size = content_length, "uploading object");

        let mut write_request =
            self.storage
                .write_object(&bucket_path, &object_name, bytes::Bytes::from(body_bytes));

        if let Some(ref content_type) = payload.content_type {
            write_request = write_request.set_content_type(content_type);
        }

        if !payload.metadata.is_empty() {
            write_request = write_request.set_metadata(payload.metadata);
        }

        Box::pin(write_request.send_buffered()).await.map_err(|e| {
            let err_str = e.to_string();
            error!(error = %err_str, "Cloud Storage upload failed");
            let gcp_err: ProviderError = classify_gcp_error(&err_str).into();
            gcp_err
        })?;

        info!(bucket = %bucket, object_name = %object_name, "object uploaded");

        Ok(ProviderResponse::success(serde_json::json!({
            "bucket": bucket,
            "object_name": object_name,
            "status": "uploaded"
        })))
    }

    async fn download_object(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing Cloud Storage download_object payload");
        let payload: StorageDownloadPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let bucket = self
            .resolve_bucket(payload.bucket.as_deref())
            .ok_or_else(|| {
                ProviderError::Configuration("no bucket in payload or provider config".to_owned())
            })?;

        let object_name = self.prefixed_name(&payload.object_name);
        let bucket_path = Self::bucket_path(bucket);

        debug!(bucket = %bucket, object_name = %object_name, "downloading object");

        let mut response = self
            .storage
            .read_object(&bucket_path, &object_name)
            .send()
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                error!(error = %err_str, "Cloud Storage download failed");
                let gcp_err: ProviderError = classify_gcp_error(&err_str).into();
                gcp_err
            })?;

        // Collect all chunks into a single buffer.
        let mut body_bytes = Vec::new();
        while let Some(chunk) = response.next().await {
            let chunk = chunk.map_err(|e| {
                ProviderError::ExecutionFailed(format!("failed to read object body: {e}"))
            })?;
            body_bytes.extend_from_slice(&chunk);
        }

        let (body_field, body_value) = match std::str::from_utf8(&body_bytes) {
            Ok(text) => ("body", serde_json::Value::String(text.to_owned())),
            Err(_) => (
                "body_base64",
                serde_json::Value::String(
                    base64::engine::general_purpose::STANDARD.encode(&body_bytes),
                ),
            ),
        };

        info!(bucket = %bucket, object_name = %object_name, size = body_bytes.len(), "object downloaded");

        Ok(ProviderResponse::success(serde_json::json!({
            "bucket": bucket,
            "object_name": object_name,
            "content_length": body_bytes.len(),
            body_field: body_value,
            "status": "downloaded"
        })))
    }

    async fn delete_object(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing Cloud Storage delete_object payload");
        let payload: StorageDeletePayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let bucket = self
            .resolve_bucket(payload.bucket.as_deref())
            .ok_or_else(|| {
                ProviderError::Configuration("no bucket in payload or provider config".to_owned())
            })?;

        let object_name = self.prefixed_name(&payload.object_name);
        let bucket_path = Self::bucket_path(bucket);

        debug!(bucket = %bucket, object_name = %object_name, "deleting object");

        self.storage_control
            .delete_object()
            .set_bucket(&bucket_path)
            .set_object(&object_name)
            .send()
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                error!(error = %err_str, "Cloud Storage delete failed");
                let gcp_err: ProviderError = classify_gcp_error(&err_str).into();
                gcp_err
            })?;

        info!(bucket = %bucket, object_name = %object_name, "object deleted");

        Ok(ProviderResponse::success(serde_json::json!({
            "bucket": bucket,
            "object_name": object_name,
            "status": "deleted"
        })))
    }

    async fn list_objects(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing Cloud Storage list_objects payload");
        let payload: StorageListPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let bucket = self
            .resolve_bucket(payload.bucket.as_deref())
            .ok_or_else(|| {
                ProviderError::Configuration("no bucket in payload or provider config".to_owned())
            })?;

        let prefix = match (payload.prefix.as_deref(), self.config.prefix.as_deref()) {
            (Some(p), Some(base)) => format!("{base}{p}"),
            (Some(p), None) => p.to_owned(),
            (None, Some(base)) => base.to_owned(),
            (None, None) => String::new(),
        };

        let bucket_path = Self::bucket_path(bucket);
        debug!(bucket = %bucket, prefix = %prefix, "listing objects");

        let mut request = self.storage_control.list_objects().set_parent(&bucket_path);
        if !prefix.is_empty() {
            request = request.set_prefix(&prefix);
        }
        if let Some(max) = payload.max_results {
            request = request.set_page_size(max);
        }

        let response = request.send().await.map_err(|e| {
            let err_str = e.to_string();
            error!(error = %err_str, "Cloud Storage list failed");
            let gcp_err: ProviderError = classify_gcp_error(&err_str).into();
            gcp_err
        })?;

        let objects: Vec<_> = response
            .objects
            .into_iter()
            .map(|obj| {
                serde_json::json!({
                    "name": obj.name,
                    "size": obj.size,
                    "content_type": obj.content_type,
                    "update_time": obj.update_time.map(|t| t.seconds()),
                })
            })
            .collect();

        info!(bucket = %bucket, count = objects.len(), "objects listed");

        Ok(ProviderResponse::success(serde_json::json!({
            "bucket": bucket,
            "prefix": prefix,
            "objects": objects
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_new_sets_project_id() {
        let config = StorageConfig::new("my-project");
        assert_eq!(config.gcp.project_id, "my-project");
        assert!(config.bucket.is_none());
        assert!(config.prefix.is_none());
    }

    #[test]
    fn config_with_bucket() {
        let config = StorageConfig::new("my-project").with_bucket("my-bucket");
        assert_eq!(config.bucket.as_deref(), Some("my-bucket"));
    }

    #[test]
    fn config_with_prefix() {
        let config = StorageConfig::new("my-project").with_prefix("acteon/");
        assert_eq!(config.prefix.as_deref(), Some("acteon/"));
    }

    #[test]
    fn config_builder_chain() {
        let config = StorageConfig::new("test-project")
            .with_bucket("data-lake")
            .with_prefix("logs/")
            .with_endpoint_url("http://localhost:4443")
            .with_credentials_path("/path/to/sa.json");
        assert_eq!(config.bucket.as_deref(), Some("data-lake"));
        assert_eq!(config.prefix.as_deref(), Some("logs/"));
        assert_eq!(
            config.gcp.endpoint_url.as_deref(),
            Some("http://localhost:4443")
        );
        assert!(config.gcp.credentials_path.is_some());
    }

    #[test]
    fn config_debug_format() {
        let config = StorageConfig::new("my-project")
            .with_bucket("my-bucket")
            .with_credentials_path("/private/key.json");
        let debug = format!("{config:?}");
        assert!(debug.contains("StorageConfig"));
        assert!(debug.contains("[REDACTED]"));
        assert!(debug.contains("my-bucket"));
    }

    #[test]
    fn config_serde_roundtrip() {
        let config = StorageConfig::new("serde-project")
            .with_bucket("archive")
            .with_prefix("data/");

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: StorageConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.gcp.project_id, "serde-project");
        assert_eq!(deserialized.bucket.as_deref(), Some("archive"));
        assert_eq!(deserialized.prefix.as_deref(), Some("data/"));
    }

    #[test]
    fn deserialize_upload_payload() {
        let json = serde_json::json!({
            "object_name": "reports/2026/02/daily.json",
            "body": "{\"total\": 42}",
            "content_type": "application/json",
            "bucket": "data-lake",
            "metadata": {
                "source": "acteon"
            }
        });
        let payload: StorageUploadPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.object_name, "reports/2026/02/daily.json");
        assert_eq!(payload.body.as_deref(), Some("{\"total\": 42}"));
        assert_eq!(payload.content_type.as_deref(), Some("application/json"));
        assert_eq!(payload.bucket.as_deref(), Some("data-lake"));
        assert_eq!(payload.metadata.get("source").unwrap(), "acteon");
    }

    #[test]
    fn deserialize_upload_payload_base64() {
        let json = serde_json::json!({
            "object_name": "images/logo.png",
            "body_base64": "iVBORw0KGgo=",
            "content_type": "image/png"
        });
        let payload: StorageUploadPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.object_name, "images/logo.png");
        assert!(payload.body.is_none());
        assert_eq!(payload.body_base64.as_deref(), Some("iVBORw0KGgo="));
    }

    #[test]
    fn deserialize_minimal_upload_payload() {
        let json = serde_json::json!({
            "object_name": "test.txt"
        });
        let payload: StorageUploadPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.object_name, "test.txt");
        assert!(payload.body.is_none());
        assert!(payload.body_base64.is_none());
        assert!(payload.content_type.is_none());
        assert!(payload.bucket.is_none());
        assert!(payload.metadata.is_empty());
    }

    #[test]
    fn deserialize_download_payload() {
        let json = serde_json::json!({
            "object_name": "reports/latest.json",
            "bucket": "data-bucket"
        });
        let payload: StorageDownloadPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.object_name, "reports/latest.json");
        assert_eq!(payload.bucket.as_deref(), Some("data-bucket"));
    }

    #[test]
    fn deserialize_delete_payload() {
        let json = serde_json::json!({
            "object_name": "old/data.csv"
        });
        let payload: StorageDeletePayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.object_name, "old/data.csv");
        assert!(payload.bucket.is_none());
    }

    #[test]
    fn bucket_path_format() {
        assert_eq!(
            StorageProvider::bucket_path("my-bucket"),
            "projects/_/buckets/my-bucket"
        );
    }
}
