use std::collections::HashMap;
use std::time::Duration;

use acteon_core::{Action, ProviderResponse};
use acteon_http::{GuardedClient, OutboundPolicy};

use crate::error::ProviderError;
use crate::provider::Provider;

/// Default request timeout for the webhook provider (30 seconds).
///
/// Prevents indefinite hangs when a remote endpoint accepts the TCP
/// connection but never sends a response (tarpit attack).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

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
    client: GuardedClient,
    /// Additional headers to include in every request.
    headers: HashMap<String, String>,
}

impl WebhookProvider {
    /// Create a new `WebhookProvider` with the given name and target URL.
    ///
    /// Uses a default `reqwest::Client` with a 30-second timeout and no
    /// extra headers.
    pub fn new(name: impl Into<String>, url: impl Into<String>) -> Self {
        let client = GuardedClient::new(OutboundPolicy::default(), DEFAULT_TIMEOUT)
            .expect("failed to build guarded webhook client");
        Self {
            name: name.into(),
            url: url.into(),
            client,
            headers: HashMap::new(),
        }
    }

    /// Set a custom `reqwest::Client` (e.g. with timeouts or TLS
    /// configuration).
    #[must_use]
    pub fn with_client(mut self, client: GuardedClient) -> Self {
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

        let mut request = self
            .client
            .post(&self.url)
            .map_err(|e| ProviderError::Configuration(e.to_string()))?
            .json(&body);

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
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| ProviderError::ExecutionFailed(e.to_string()))?
        {
            if bytes.len().saturating_add(chunk.len()) > 1_048_576 {
                return Err(ProviderError::ExecutionFailed(
                    "webhook response exceeds 1 MiB".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let response_body: serde_json::Value = serde_json::from_slice(&bytes)
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
            .map_err(|e| ProviderError::Configuration(e.to_string()))?
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
        let client =
            GuardedClient::new(OutboundPolicy::default(), Duration::from_secs(10)).unwrap();

        let provider =
            WebhookProvider::new("custom", "https://example.com/webhook").with_client(client);

        assert_eq!(Provider::name(&provider), "custom");
    }
}
