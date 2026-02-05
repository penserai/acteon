use std::collections::HashMap;

use acteon_core::{Action, ProviderResponse};
use reqwest::Client;

use crate::error::ProviderError;
use crate::provider::Provider;

/// A provider that dispatches actions by posting their JSON payload to a
/// configurable HTTP endpoint.
///
/// The webhook provider is feature-gated behind `webhook` and depends on
/// `reqwest`.
pub struct WebhookProvider {
    /// Unique name for this provider instance.
    name: String,
    /// The target URL to POST actions to.
    url: String,
    /// HTTP client used for outgoing requests.
    client: Client,
    /// Additional headers to include in every request.
    headers: HashMap<String, String>,
}

impl WebhookProvider {
    /// Create a new `WebhookProvider` with the given name and target URL.
    ///
    /// Uses a default `reqwest::Client` and no extra headers.
    pub fn new(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            client: Client::new(),
            headers: HashMap::new(),
        }
    }

    /// Set a custom `reqwest::Client` (e.g. with timeouts or TLS
    /// configuration).
    #[must_use]
    pub fn with_client(mut self, client: Client) -> Self {
        self.client = client;
        self
    }

    /// Add extra headers to send with every webhook request.
    #[must_use]
    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = headers;
        self
    }
}

impl Provider for WebhookProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        let body = serde_json::to_value(action)
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let mut request = self.client.post(&self.url).json(&body);

        for (key, value) in &self.headers {
            request = request.header(key, value);
        }

        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                ProviderError::Timeout(std::time::Duration::from_secs(0))
            } else if e.is_connect() {
                ProviderError::Connection(e.to_string())
            } else {
                ProviderError::ExecutionFailed(e.to_string())
            }
        })?;

        let status = response.status();
        let response_body: serde_json::Value = response
            .json()
            .await
            .unwrap_or_else(|_| serde_json::json!({"status_code": status.as_u16()}));

        if status.is_success() {
            Ok(ProviderResponse::success(response_body))
        } else {
            Ok(ProviderResponse::failure(response_body))
        }
    }

    async fn health_check(&self) -> Result<(), ProviderError> {
        // Attempt a HEAD request to verify the endpoint is reachable.
        self.client
            .head(&self.url)
            .send()
            .await
            .map_err(|e| ProviderError::Connection(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhook_provider_creation() {
        let provider = WebhookProvider::new("test-hook", "https://example.com/webhook");
        assert_eq!(Provider::name(&provider), "test-hook");
    }

    #[test]
    fn webhook_provider_with_headers() {
        let mut headers = HashMap::new();
        headers.insert("Authorization".into(), "Bearer token123".into());

        let provider =
            WebhookProvider::new("auth-hook", "https://example.com/webhook").with_headers(headers);

        assert_eq!(provider.headers.len(), 1);
        assert_eq!(
            provider.headers.get("Authorization").unwrap(),
            "Bearer token123"
        );
    }

    #[test]
    fn webhook_provider_with_custom_client() {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap();

        let provider =
            WebhookProvider::new("custom", "https://example.com/webhook").with_client(client);

        assert_eq!(Provider::name(&provider), "custom");
    }
}
