use acteon_core::{Action, ProviderResponse};
use acteon_provider::{Provider, ProviderError};
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, instrument, warn};

use crate::config::TeamsConfig;
use crate::error::TeamsError;
use crate::types::TeamsMessageCard;

/// Microsoft Teams provider that sends messages via incoming webhooks.
///
/// Implements the [`Provider`] trait so it can be registered in the provider
/// registry and used by the action executor.
pub struct TeamsProvider {
    config: TeamsConfig,
    client: Client,
}

/// Fields extracted from an action payload for a Teams message.
#[derive(Debug, Deserialize)]
struct MessagePayload {
    text: Option<String>,
    title: Option<String>,
    summary: Option<String>,
    theme_color: Option<String>,
    adaptive_card: Option<serde_json::Value>,
}

impl TeamsProvider {
    /// Create a new Teams provider with the given configuration.
    pub fn new(config: TeamsConfig) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");
        Self { config, client }
    }

    /// Create a new Teams provider with a custom HTTP client.
    pub fn with_client(config: TeamsConfig, client: Client) -> Self {
        Self { config, client }
    }

    /// Build a JSON body for the webhook request.
    fn build_body(payload: &MessagePayload) -> Result<serde_json::Value, TeamsError> {
        if let Some(ref card) = payload.adaptive_card {
            // Wrap the adaptive card in the Teams attachment envelope.
            return Ok(serde_json::json!({
                "type": "message",
                "attachments": [{
                    "contentType": "application/vnd.microsoft.card.adaptive",
                    "content": card
                }]
            }));
        }

        if let Some(ref text) = payload.text {
            let mut card = TeamsMessageCard::new(text);
            if let Some(ref title) = payload.title {
                card = card.with_title(title);
            }
            if let Some(ref summary) = payload.summary {
                card = card.with_summary(summary);
            }
            if let Some(ref color) = payload.theme_color {
                card = card.with_theme_color(color);
            }
            return serde_json::to_value(&card)
                .map_err(|e| TeamsError::InvalidPayload(format!("failed to serialize card: {e}")));
        }

        Err(TeamsError::InvalidPayload(
            "payload must contain at least one of 'text' or 'adaptive_card'".into(),
        ))
    }
}

impl Provider for TeamsProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "teams"
    }

    #[instrument(skip(self, action), fields(action_id = %action.id, provider = "teams"))]
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        let payload: MessagePayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| TeamsError::InvalidPayload(format!("failed to parse payload: {e}")))?;

        let body = Self::build_body(&payload)?;

        debug!("posting message to Teams webhook");

        let response = acteon_provider::inject_trace_context(
            self.client.post(&self.config.webhook_url).json(&body),
        )
        .send()
        .await
        .map_err(TeamsError::Http)?;

        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            warn!("Teams webhook rate limit hit");
            return Err(TeamsError::RateLimited.into());
        }

        if !status.is_success() {
            let response_body = response.text().await.unwrap_or_default();
            return Err(TeamsError::Api(format!("HTTP {status}: {response_body}")).into());
        }

        // Teams returns literal "1" with HTTP 200 on success (not JSON).
        let response_text = response.text().await.unwrap_or_default();

        let response_body = serde_json::json!({
            "ok": true,
            "response": response_text,
        });

        Ok(ProviderResponse::success(response_body))
    }

    #[instrument(skip(self), fields(provider = "teams"))]
    async fn health_check(&self) -> Result<(), ProviderError> {
        debug!("performing Teams health check via webhook ping");

        // Teams webhooks don't have a dedicated health endpoint. We send a
        // minimal JSON payload — any HTTP response from the host means the
        // webhook URL is reachable.
        let response = self
            .client
            .post(&self.config.webhook_url)
            .json(&serde_json::json!({ "text": "health check" }))
            .send()
            .await
            .map_err(|e| ProviderError::Connection(e.to_string()))?;

        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ProviderError::RateLimited);
        }

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::Connection(format!("HTTP {status}: {body}")));
        }

        debug!("Teams health check passed");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use acteon_core::Action;
    use acteon_provider::{Provider, ProviderError};

    use super::*;
    use crate::config::TeamsConfig;

    struct MockTeamsServer {
        listener: tokio::net::TcpListener,
        base_url: String,
    }

    impl MockTeamsServer {
        async fn start() -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("failed to bind mock server");
            let port = listener.local_addr().unwrap().port();
            let base_url = format!("http://127.0.0.1:{port}");
            Self { listener, base_url }
        }

        async fn respond_once(self, status_code: u16, body: &str) {
            let body = body.to_owned();
            let (mut stream, _) = self.listener.accept().await.unwrap();

            use tokio::io::{AsyncReadExt, AsyncWriteExt};

            let mut buf = vec![0u8; 8192];
            let _ = stream.read(&mut buf).await.unwrap();

            let response = format!(
                "HTTP/1.1 {status_code} OK\r\n\
                 Content-Type: text/plain\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n\
                 {body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
        }

        async fn respond_rate_limited(self) {
            self.respond_once(429, "rate limited").await;
        }
    }

    fn make_action(payload: serde_json::Value) -> Action {
        Action::new(
            "notifications",
            "tenant-1",
            "teams",
            "send_message",
            payload,
        )
    }

    #[test]
    fn provider_name() {
        let config = TeamsConfig::new("https://example.com/webhook");
        let provider = TeamsProvider::new(config);
        assert_eq!(provider.name(), "teams");
    }

    #[tokio::test]
    async fn execute_success_with_text() {
        let server = MockTeamsServer::start().await;
        let config = TeamsConfig::new(&server.base_url);
        let provider = TeamsProvider::new(config);

        let action = make_action(serde_json::json!({
            "text": "Hello from Acteon!",
            "title": "Alert",
            "theme_color": "FF0000"
        }));

        let server_handle = tokio::spawn(async move {
            server.respond_once(200, "1").await;
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        let response = result.expect("execute should succeed");
        assert_eq!(response.status, acteon_core::ResponseStatus::Success);
        assert_eq!(response.body["ok"], true);
    }

    #[tokio::test]
    async fn execute_success_with_adaptive_card() {
        let server = MockTeamsServer::start().await;
        let config = TeamsConfig::new(&server.base_url);
        let provider = TeamsProvider::new(config);

        let action = make_action(serde_json::json!({
            "adaptive_card": {
                "type": "AdaptiveCard",
                "version": "1.4",
                "body": [{"type": "TextBlock", "text": "Hello!"}]
            }
        }));

        let server_handle = tokio::spawn(async move {
            server.respond_once(200, "1").await;
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn execute_missing_text_and_card() {
        let config = TeamsConfig::new("http://localhost:1");
        let provider = TeamsProvider::new(config);

        let action = make_action(serde_json::json!({
            "title": "No content"
        }));

        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn execute_rate_limited() {
        let server = MockTeamsServer::start().await;
        let config = TeamsConfig::new(&server.base_url);
        let provider = TeamsProvider::new(config);

        let action = make_action(serde_json::json!({
            "text": "Hello!"
        }));

        let server_handle = tokio::spawn(async move {
            server.respond_rate_limited().await;
        });

        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();

        assert!(matches!(err, ProviderError::RateLimited));
        assert!(err.is_retryable());
    }

    #[tokio::test]
    async fn execute_api_error() {
        let server = MockTeamsServer::start().await;
        let config = TeamsConfig::new(&server.base_url);
        let provider = TeamsProvider::new(config);

        let action = make_action(serde_json::json!({
            "text": "Hello!"
        }));

        let server_handle = tokio::spawn(async move {
            server.respond_once(400, "Bad Request").await;
        });

        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();

        assert!(matches!(err, ProviderError::ExecutionFailed(_)));
    }

    #[tokio::test]
    async fn health_check_success() {
        let server = MockTeamsServer::start().await;
        let config = TeamsConfig::new(&server.base_url);
        let provider = TeamsProvider::new(config);

        let server_handle = tokio::spawn(async move {
            server.respond_once(200, "1").await;
        });

        let result = provider.health_check().await;
        server_handle.await.unwrap();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn health_check_rate_limited() {
        let server = MockTeamsServer::start().await;
        let config = TeamsConfig::new(&server.base_url);
        let provider = TeamsProvider::new(config);

        let server_handle = tokio::spawn(async move {
            server.respond_rate_limited().await;
        });

        let err = provider.health_check().await.unwrap_err();
        server_handle.await.unwrap();

        assert!(matches!(err, ProviderError::RateLimited));
    }
}
