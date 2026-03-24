use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::TesseraiConnectionConfig;
use crate::error::SwarmError;

/// HTTP client for the `TesseraiDB` REST API.
#[derive(Clone)]
pub struct TesseraiClient {
    client: Client,
    base_url: String,
    tenant_id: String,
    auth_header: Option<String>,
}

// ── JSON Twin types ──────────────────────────────────────────────────────────

/// Request to create a JSON twin.
#[derive(Debug, Serialize)]
pub struct CreateTwinRequest {
    pub id: String,
    #[serde(rename = "type")]
    pub twin_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub properties: Value,
}

/// Response from twin operations.
#[derive(Debug, Deserialize)]
pub struct TwinResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub twin_type: Option<String>,
    #[serde(default)]
    pub properties: Value,
}

// ── Client implementation ────────────────────────────────────────────────────

impl TesseraiClient {
    /// Create a new client from connection configuration.
    pub fn new(config: &TesseraiConnectionConfig) -> Result<Self, SwarmError> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| SwarmError::Tesserai(format!("failed to build HTTP client: {e}")))?;

        Ok(Self {
            client,
            base_url: config.endpoint.trim_end_matches('/').to_string(),
            tenant_id: config.tenant_id.clone(),
            auth_header: config.api_key.clone().map(|k| format!("Bearer {k}")),
        })
    }

    /// Create a client with an existing `reqwest::Client` (for sharing connections).
    pub fn with_client(client: Client, config: &TesseraiConnectionConfig) -> Self {
        Self {
            client,
            base_url: config.endpoint.trim_end_matches('/').to_string(),
            tenant_id: config.tenant_id.clone(),
            auth_header: config.api_key.clone().map(|k| format!("Bearer {k}")),
        }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{path}", self.base_url);
        let mut req = self.client.request(method, &url);
        req = req.header("X-Tenant-ID", &self.tenant_id);
        if let Some(ref auth) = self.auth_header {
            req = req.header("Authorization", auth);
        }
        req
    }

    // ── JSON Twin operations ─────────────────────────────────────────────────

    /// Create a JSON twin.
    pub async fn create_twin(&self, twin: &CreateTwinRequest) -> Result<TwinResponse, SwarmError> {
        let resp = self
            .request(reqwest::Method::POST, "/api/v1/twins/json")
            .json(twin)
            .send()
            .await?;
        handle_response(resp).await
    }

    /// Get a JSON twin by ID.
    pub async fn get_twin(&self, twin_id: &str) -> Result<TwinResponse, SwarmError> {
        let resp = self
            .request(
                reqwest::Method::GET,
                &format!("/api/v1/twins/json/{twin_id}"),
            )
            .send()
            .await?;
        handle_response(resp).await
    }

    /// List JSON twins with optional type and search filters.
    pub async fn list_twins(
        &self,
        twin_type: Option<&str>,
        search: Option<&str>,
    ) -> Result<Vec<TwinResponse>, SwarmError> {
        use std::fmt::Write as _;
        let mut path = "/api/v1/twins/json?limit=100".to_string();
        if let Some(t) = twin_type {
            let _ = write!(path, "&type={t}");
        }
        if let Some(s) = search {
            let _ = write!(path, "&search={s}");
        }
        let resp = self
            .request(reqwest::Method::GET, &path)
            .send()
            .await?;

        // The list endpoint returns {"data": [...], "total": N}
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| SwarmError::Tesserai(format!("failed to parse list response: {e}")))?;

        let items = body
            .get("data")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();

        items
            .into_iter()
            .map(|v| {
                serde_json::from_value(v)
                    .map_err(|e| SwarmError::Tesserai(format!("failed to parse twin: {e}")))
            })
            .collect()
    }

    /// Update a JSON twin's properties.
    pub async fn patch_twin(
        &self,
        twin_id: &str,
        properties: &Value,
    ) -> Result<TwinResponse, SwarmError> {
        let resp = self
            .request(
                reqwest::Method::PATCH,
                &format!("/api/v1/twins/json/{twin_id}"),
            )
            .json(properties)
            .send()
            .await?;
        handle_response(resp).await
    }

    /// Check if `TesseraiDB` is reachable.
    pub async fn health_check(&self) -> Result<(), SwarmError> {
        let resp = self
            .request(reqwest::Method::GET, "/health/ping")
            .send()
            .await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(SwarmError::Tesserai(format!(
                "health check failed: HTTP {}",
                resp.status()
            )))
        }
    }
}

async fn handle_response<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
) -> Result<T, SwarmError> {
    let status = resp.status();
    if status.is_success() {
        resp.json()
            .await
            .map_err(|e| SwarmError::Tesserai(format!("failed to parse response: {e}")))
    } else {
        let body = resp.text().await.unwrap_or_default();
        Err(SwarmError::Tesserai(format!("HTTP {status}: {body}")))
    }
}
