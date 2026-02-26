use std::collections::HashMap;
use std::sync::Arc;

use acteon_core::{Action, ProviderResponse};
use acteon_provider::ProviderError;
use acteon_provider::provider::Provider;
use azure_core::credentials::TokenCredential;
use azure_storage_blob::BlobServiceClient;
use base64::Engine;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, instrument};

use crate::auth::build_azure_credential;
use crate::config::AzureBaseConfig;
use crate::error::classify_azure_error;

/// Configuration for the Azure Blob Storage provider.
#[derive(Clone, Serialize, Deserialize)]
pub struct BlobConfig {
    /// Shared Azure configuration.
    #[serde(flatten)]
    pub azure: AzureBaseConfig,

    /// Azure Storage account name.
    pub account_name: Option<String>,

    /// Default container name. Can be overridden per-action in the payload.
    pub container_name: Option<String>,

    /// Default blob name prefix (e.g. `"acteon/artifacts/"`).
    pub prefix: Option<String>,
}

impl std::fmt::Debug for BlobConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlobConfig")
            .field("azure", &self.azure)
            .field("account_name", &self.account_name)
            .field("container_name", &self.container_name)
            .field("prefix", &self.prefix)
            .finish()
    }
}

impl BlobConfig {
    /// Create a new `BlobConfig` with the given Azure location.
    pub fn new(location: impl Into<String>) -> Self {
        Self {
            azure: AzureBaseConfig::new(location),
            account_name: None,
            container_name: None,
            prefix: None,
        }
    }

    /// Set the storage account name.
    #[must_use]
    pub fn with_account_name(mut self, account_name: impl Into<String>) -> Self {
        self.account_name = Some(account_name.into());
        self
    }

    /// Set the default container name.
    #[must_use]
    pub fn with_container_name(mut self, container_name: impl Into<String>) -> Self {
        self.container_name = Some(container_name.into());
        self
    }

    /// Set the default blob name prefix.
    #[must_use]
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    /// Set the endpoint URL override (for `Azurite`).
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

/// Payload for the `upload_blob` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobUploadPayload {
    /// Container name. Overrides config default.
    pub container: Option<String>,

    /// Blob name (path within the container).
    pub blob_name: String,

    /// Blob body as a UTF-8 string.
    /// Mutually exclusive with `body_base64`.
    pub body: Option<String>,

    /// Blob body as base64-encoded bytes.
    /// Mutually exclusive with `body`.
    pub body_base64: Option<String>,

    /// Optional `Content-Type` for the blob.
    pub content_type: Option<String>,

    /// Optional metadata key-value pairs attached to the blob.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Payload for the `download_blob` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobDownloadPayload {
    /// Container name. Overrides config default.
    pub container: Option<String>,

    /// Blob name to download.
    pub blob_name: String,
}

/// Payload for the `delete_blob` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobDeletePayload {
    /// Container name. Overrides config default.
    pub container: Option<String>,

    /// Blob name to delete.
    pub blob_name: String,
}

/// Azure Blob Storage provider for storing and retrieving blobs.
pub struct BlobProvider {
    config: BlobConfig,
    service_client: BlobServiceClient,
    credential: Arc<dyn TokenCredential>,
    endpoint: String,
}

impl std::fmt::Debug for BlobProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlobProvider")
            .field("config", &self.config)
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl BlobProvider {
    /// Create a new `BlobProvider` by building an Azure Blob Storage client.
    pub async fn new(config: BlobConfig) -> Result<Self, ProviderError> {
        let account_name = config.account_name.as_deref().ok_or_else(|| {
            ProviderError::Configuration("azure blob: account_name is required".to_owned())
        })?;

        let credential = build_azure_credential(&config.azure)
            .await
            .map_err(|e| ProviderError::Configuration(e.to_string()))?;

        let endpoint = config
            .azure
            .endpoint_url
            .clone()
            .unwrap_or_else(|| format!("https://{account_name}.blob.core.windows.net"));

        let service_client = BlobServiceClient::new(&endpoint, Some(Arc::clone(&credential)), None)
            .map_err(|e| ProviderError::Configuration(format!("blob client error: {e}")))?;

        Ok(Self {
            config,
            service_client,
            credential,
            endpoint,
        })
    }

    /// Resolve the container name from the payload or config default.
    fn resolve_container<'a>(&'a self, payload_container: Option<&'a str>) -> Option<&'a str> {
        payload_container.or(self.config.container_name.as_deref())
    }

    /// Apply the configured prefix to a blob name.
    fn prefixed_name(&self, blob_name: &str) -> String {
        match &self.config.prefix {
            Some(prefix) => format!("{prefix}{blob_name}"),
            None => blob_name.to_owned(),
        }
    }
}

impl Provider for BlobProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "azure-blob"
    }

    #[instrument(skip(self, action), fields(action_id = %action.id, provider = "azure-blob"))]
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        match action.action_type.as_str() {
            "upload_blob" => self.upload_blob(action).await,
            "download_blob" => self.download_blob(action).await,
            "delete_blob" => self.delete_blob(action).await,
            other => Err(ProviderError::Configuration(format!(
                "unknown Blob action type '{other}' (expected 'upload_blob', 'download_blob', or 'delete_blob')"
            ))),
        }
    }

    #[instrument(skip(self), fields(provider = "azure-blob"))]
    async fn health_check(&self) -> Result<(), ProviderError> {
        debug!("performing Azure Blob health check");
        // List containers to verify connectivity.
        let _pager = self.service_client.list_containers(None).map_err(|e| {
            error!(error = %e, "Azure Blob health check failed");
            ProviderError::Connection(format!("Azure Blob health check failed: {e}"))
        })?;
        info!("Azure Blob health check passed");
        Ok(())
    }
}

impl BlobProvider {
    async fn upload_blob(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing Blob upload_blob payload");
        let payload: BlobUploadPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let container = self
            .resolve_container(payload.container.as_deref())
            .ok_or_else(|| {
                ProviderError::Configuration(
                    "no container in payload or provider config".to_owned(),
                )
            })?;

        let blob_name = self.prefixed_name(&payload.blob_name);

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

        let content_length = body_bytes.len() as u64;
        debug!(container = %container, blob_name = %blob_name, size = content_length, "uploading blob");

        let blob_client = self.service_client.blob_client(container, &blob_name);
        let data: azure_core::Bytes = body_bytes.into();

        blob_client
            .upload(data.into(), true, content_length, None)
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                error!(error = %err_str, "Blob upload failed");
                let azure_err: ProviderError = classify_azure_error(&err_str).into();
                azure_err
            })?;

        info!(container = %container, blob_name = %blob_name, "blob uploaded");

        Ok(ProviderResponse::success(serde_json::json!({
            "container": container,
            "blob_name": blob_name,
            "status": "uploaded"
        })))
    }

    async fn download_blob(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing Blob download_blob payload");
        let payload: BlobDownloadPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let container = self
            .resolve_container(payload.container.as_deref())
            .ok_or_else(|| {
                ProviderError::Configuration(
                    "no container in payload or provider config".to_owned(),
                )
            })?;

        let blob_name = self.prefixed_name(&payload.blob_name);

        debug!(container = %container, blob_name = %blob_name, "downloading blob");

        let blob_client = azure_storage_blob::BlobClient::new(
            &self.endpoint,
            container,
            &blob_name,
            Some(Arc::clone(&self.credential)),
            None,
        )
        .map_err(|e| ProviderError::Configuration(format!("blob client error: {e}")))?;

        let response = blob_client.download(None).await.map_err(|e| {
            let err_str = e.to_string();
            error!(error = %err_str, "Blob download failed");
            let azure_err: ProviderError = classify_azure_error(&err_str).into();
            azure_err
        })?;

        let body_bytes: azure_core::Bytes = response.into_body().collect().await.map_err(|e| {
            ProviderError::ExecutionFailed(format!("failed to read blob body: {e}"))
        })?;

        let (body_field, body_value) = match std::str::from_utf8(&body_bytes) {
            Ok(text) => ("body", serde_json::Value::String(text.to_owned())),
            Err(_) => (
                "body_base64",
                serde_json::Value::String(
                    base64::engine::general_purpose::STANDARD.encode(&body_bytes),
                ),
            ),
        };

        info!(container = %container, blob_name = %blob_name, size = body_bytes.len(), "blob downloaded");

        Ok(ProviderResponse::success(serde_json::json!({
            "container": container,
            "blob_name": blob_name,
            "content_length": body_bytes.len(),
            body_field: body_value,
            "status": "downloaded"
        })))
    }

    async fn delete_blob(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing Blob delete_blob payload");
        let payload: BlobDeletePayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let container = self
            .resolve_container(payload.container.as_deref())
            .ok_or_else(|| {
                ProviderError::Configuration(
                    "no container in payload or provider config".to_owned(),
                )
            })?;

        let blob_name = self.prefixed_name(&payload.blob_name);

        debug!(container = %container, blob_name = %blob_name, "deleting blob");

        let blob_client = self.service_client.blob_client(container, &blob_name);

        blob_client.delete(None).await.map_err(|e| {
            let err_str = e.to_string();
            error!(error = %err_str, "Blob delete failed");
            let azure_err: ProviderError = classify_azure_error(&err_str).into();
            azure_err
        })?;

        info!(container = %container, blob_name = %blob_name, "blob deleted");

        Ok(ProviderResponse::success(serde_json::json!({
            "container": container,
            "blob_name": blob_name,
            "status": "deleted"
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_new_sets_location() {
        let config = BlobConfig::new("westeurope");
        assert_eq!(config.azure.location, "westeurope");
        assert!(config.account_name.is_none());
        assert!(config.container_name.is_none());
        assert!(config.prefix.is_none());
    }

    #[test]
    fn config_with_account_name() {
        let config = BlobConfig::new("eastus").with_account_name("mystorageaccount");
        assert_eq!(config.account_name.as_deref(), Some("mystorageaccount"));
    }

    #[test]
    fn config_with_container_name() {
        let config = BlobConfig::new("eastus").with_container_name("my-container");
        assert_eq!(config.container_name.as_deref(), Some("my-container"));
    }

    #[test]
    fn config_with_prefix() {
        let config = BlobConfig::new("eastus").with_prefix("acteon/");
        assert_eq!(config.prefix.as_deref(), Some("acteon/"));
    }

    #[test]
    fn config_builder_chain() {
        let config = BlobConfig::new("eastus2")
            .with_account_name("teststorage")
            .with_container_name("data")
            .with_prefix("logs/")
            .with_endpoint_url("http://127.0.0.1:10000")
            .with_tenant_id("tid-123")
            .with_client_id("cid-456")
            .with_client_credential("cred-789");
        assert_eq!(config.account_name.as_deref(), Some("teststorage"));
        assert_eq!(config.container_name.as_deref(), Some("data"));
        assert_eq!(config.prefix.as_deref(), Some("logs/"));
        assert_eq!(
            config.azure.endpoint_url.as_deref(),
            Some("http://127.0.0.1:10000")
        );
        assert!(config.azure.tenant_id.is_some());
    }

    #[test]
    fn config_debug_format() {
        let config = BlobConfig::new("eastus")
            .with_account_name("mystorageaccount")
            .with_client_credential("private-val");
        let debug = format!("{config:?}");
        assert!(debug.contains("BlobConfig"));
        assert!(debug.contains("[REDACTED]"));
        assert!(debug.contains("mystorageaccount"));
    }

    #[test]
    fn config_serde_roundtrip() {
        let config = BlobConfig::new("northeurope")
            .with_account_name("archive")
            .with_container_name("backups")
            .with_prefix("data/");

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: BlobConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.azure.location, "northeurope");
        assert_eq!(deserialized.account_name.as_deref(), Some("archive"));
        assert_eq!(deserialized.container_name.as_deref(), Some("backups"));
        assert_eq!(deserialized.prefix.as_deref(), Some("data/"));
    }

    #[test]
    fn deserialize_upload_payload() {
        let json = serde_json::json!({
            "blob_name": "reports/2026/report.json",
            "body": "{\"total\": 42}",
            "content_type": "application/json",
            "container": "my-container",
            "metadata": {
                "source": "acteon"
            }
        });
        let payload: BlobUploadPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.blob_name, "reports/2026/report.json");
        assert_eq!(payload.body.as_deref(), Some("{\"total\": 42}"));
        assert_eq!(payload.content_type.as_deref(), Some("application/json"));
        assert_eq!(payload.container.as_deref(), Some("my-container"));
        assert_eq!(payload.metadata.get("source").unwrap(), "acteon");
    }

    #[test]
    fn deserialize_upload_payload_base64() {
        let json = serde_json::json!({
            "blob_name": "images/logo.png",
            "body_base64": "iVBORw0KGgo=",
            "content_type": "image/png"
        });
        let payload: BlobUploadPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.blob_name, "images/logo.png");
        assert!(payload.body.is_none());
        assert_eq!(payload.body_base64.as_deref(), Some("iVBORw0KGgo="));
    }

    #[test]
    fn deserialize_minimal_upload_payload() {
        let json = serde_json::json!({
            "blob_name": "test.txt"
        });
        let payload: BlobUploadPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.blob_name, "test.txt");
        assert!(payload.body.is_none());
        assert!(payload.body_base64.is_none());
        assert!(payload.content_type.is_none());
        assert!(payload.container.is_none());
        assert!(payload.metadata.is_empty());
    }

    #[test]
    fn deserialize_download_payload() {
        let json = serde_json::json!({
            "blob_name": "reports/latest.json",
            "container": "data-container"
        });
        let payload: BlobDownloadPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.blob_name, "reports/latest.json");
        assert_eq!(payload.container.as_deref(), Some("data-container"));
    }

    #[test]
    fn deserialize_delete_payload() {
        let json = serde_json::json!({
            "blob_name": "old/data.csv"
        });
        let payload: BlobDeletePayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.blob_name, "old/data.csv");
        assert!(payload.container.is_none());
    }
}
