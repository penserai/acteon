use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::config::EmbeddingConfig;
use crate::error::EmbeddingError;
use crate::provider::EmbeddingProvider;

/// An embedding provider that calls an OpenAI-compatible `/v1/embeddings` API.
pub struct HttpEmbeddingProvider {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    api_key: String,
}

impl HttpEmbeddingProvider {
    /// Create a new HTTP embedding provider from the given configuration.
    pub fn new(config: EmbeddingConfig) -> Result<Self, EmbeddingError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .build()
            .map_err(|e| EmbeddingError::HttpError(e.to_string()))?;

        Ok(Self {
            client,
            endpoint: config.endpoint,
            model: config.model,
            api_key: config.api_key,
        })
    }
}

/// `OpenAI` embeddings API request body.
#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a str,
}

/// `OpenAI` embeddings API response.
#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

/// A single embedding result.
#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

#[async_trait]
impl EmbeddingProvider for HttpEmbeddingProvider {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        debug!(model = %self.model, text_len = text.len(), "requesting embedding");

        let request_body = EmbeddingRequest {
            model: &self.model,
            input: text,
        };

        let response = self
            .client
            .post(&self.endpoint)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request_body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    EmbeddingError::Timeout
                } else {
                    EmbeddingError::HttpError(e.to_string())
                }
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "failed to read body".to_owned());
            return Err(EmbeddingError::ApiError(format!("status {status}: {body}")));
        }

        let result: EmbeddingResponse = response
            .json()
            .await
            .map_err(|e| EmbeddingError::ParseError(e.to_string()))?;

        result
            .data
            .into_iter()
            .next()
            .map(|d| d.embedding)
            .ok_or_else(|| EmbeddingError::ParseError("empty response data".to_owned()))
    }
}
