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

// ── Semantic Memory types ────────────────────────────────────────────────────

/// Request to create a semantic memory record.
#[derive(Debug, Serialize)]
pub struct CreateMemoryRequest {
    pub memory_type: String,
    pub record_type: String,
    pub agent_id: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default)]
    pub topics: Vec<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
}

/// Query for semantic memory search.
#[derive(Debug, Serialize)]
pub struct MemorySearchQuery {
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_confidence: Option<f64>,
}

/// A memory record returned from `TesseraiDB`.
#[derive(Debug, Deserialize)]
pub struct MemoryRecord {
    pub id: String,
    pub memory_type: String,
    pub record_type: String,
    pub agent_id: String,
    pub content: String,
    #[serde(default)]
    pub topics: Vec<String>,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default)]
    pub relevance_score: f64,
}

/// Default confidence value for memory records (used by serde).
pub fn default_confidence() -> f64 {
    1.0
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

    // ── Semantic Memory operations ───────────────────────────────────────────

    /// Store a semantic memory record.
    pub async fn create_memory(
        &self,
        memory: &CreateMemoryRequest,
    ) -> Result<MemoryRecord, SwarmError> {
        let resp = self
            .request(reqwest::Method::POST, "/api/v1/semantic-memory")
            .json(memory)
            .send()
            .await?;
        handle_response(resp).await
    }

    /// Search semantic memories.
    pub async fn search_memories(
        &self,
        query: &MemorySearchQuery,
    ) -> Result<Vec<MemoryRecord>, SwarmError> {
        let resp = self
            .request(reqwest::Method::POST, "/api/v1/semantic-memory/search")
            .json(query)
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
