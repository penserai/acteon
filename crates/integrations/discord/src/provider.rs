use acteon_core::{Action, ProviderResponse};
use acteon_provider::{DispatchContext, Provider, ProviderError};
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, info, instrument, warn};

use crate::config::DiscordConfig;
use crate::error::DiscordError;
use crate::types::{DiscordEmbed, DiscordWebhookRequest, DiscordWebhookResponse};

/// Discord provider that sends messages via Discord webhooks.
///
/// Implements the [`Provider`] trait so it can be registered in the provider
/// registry and used by the action executor.
pub struct DiscordProvider {
    config: DiscordConfig,
    client: Client,
}

/// Fields extracted from an action payload for a Discord message.
#[derive(Debug, Deserialize)]
struct MessagePayload {
    content: Option<String>,
    username: Option<String>,
    avatar_url: Option<String>,
    tts: Option<bool>,
    embeds: Option<Vec<DiscordEmbed>>,
}

impl DiscordProvider {
    /// Create a new Discord provider with the given configuration.
    pub fn new(config: DiscordConfig) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");
        Self { config, client }
    }

    /// Create a new Discord provider with a custom HTTP client.
    pub fn with_client(config: DiscordConfig, client: Client) -> Self {
        Self { config, client }
    }

    /// Build the effective webhook URL, appending `?wait=true` if configured.
    fn effective_url(&self) -> String {
        if self.config.wait {
            if self.config.webhook_url.contains('?') {
                format!("{}&wait=true", self.config.webhook_url)
            } else {
                format!("{}?wait=true", self.config.webhook_url)
            }
        } else {
            self.config.webhook_url.clone()
        }
    }

    /// Parse and validate the action payload, returning the webhook request
    /// ready for dispatch.
    fn parse_request(&self, action: &Action) -> Result<DiscordWebhookRequest, ProviderError> {
        let payload: MessagePayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| DiscordError::InvalidPayload(format!("failed to parse payload: {e}")))?;

        let has_content = payload.content.is_some();
        let has_embeds = payload.embeds.as_ref().is_some_and(|e| !e.is_empty());

        if !has_content && !has_embeds {
            return Err(DiscordError::InvalidPayload(
                "payload must contain at least one of 'content' or 'embeds'".into(),
            )
            .into());
        }

        Ok(DiscordWebhookRequest {
            content: payload.content,
            username: payload
                .username
                .or_else(|| self.config.default_username.clone()),
            avatar_url: payload
                .avatar_url
                .or_else(|| self.config.default_avatar_url.clone()),
            tts: payload.tts,
            embeds: payload.embeds,
        })
    }

    /// Interpret the Discord HTTP response, handling status codes and
    /// building the provider response with an optional attachment count.
    async fn interpret_response(
        &self,
        response: reqwest::Response,
        attachment_count: usize,
    ) -> Result<ProviderResponse, ProviderError> {
        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            warn!("Discord API rate limit hit");
            return Err(DiscordError::RateLimited.into());
        }

        if !status.is_success() {
            let response_body = response.text().await.unwrap_or_default();
            return Err(DiscordError::Api(format!("HTTP {status}: {response_body}")).into());
        }

        let mut body = if status == reqwest::StatusCode::NO_CONTENT {
            serde_json::json!({ "ok": true })
        } else {
            match response.json::<DiscordWebhookResponse>().await {
                Ok(resp) => serde_json::json!({
                    "ok": true,
                    "id": resp.id,
                    "channel_id": resp.channel_id,
                }),
                Err(_) => serde_json::json!({ "ok": true }),
            }
        };

        if attachment_count > 0 {
            body["attachment_count"] = serde_json::json!(attachment_count);
        }

        Ok(ProviderResponse::success(body))
    }
}

impl Provider for DiscordProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "discord"
    }

    #[instrument(skip(self, action), fields(action_id = %action.id, provider = "discord"))]
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        let request = self.parse_request(action)?;
        let url = self.effective_url();

        debug!("posting message to Discord webhook");

        let response = acteon_provider::inject_trace_context(self.client.post(&url).json(&request))
            .send()
            .await
            .map_err(DiscordError::Http)?;

        self.interpret_response(response, 0).await
    }

    fn supports_attachments(&self) -> bool {
        true
    }

    #[instrument(skip(self, action, ctx), fields(action_id = %action.id, provider = "discord"))]
    async fn execute_with_context(
        &self,
        action: &Action,
        ctx: &DispatchContext,
    ) -> Result<ProviderResponse, ProviderError> {
        let request = self.parse_request(action)?;
        let url = self.effective_url();

        debug!(
            attachment_count = ctx.attachments.len(),
            "posting message to Discord webhook with attachments"
        );

        // Discord webhooks support multipart/form-data with payload_json + file parts.
        let payload_json = serde_json::to_string(&request).map_err(|e| {
            DiscordError::InvalidPayload(format!("failed to serialize request: {e}"))
        })?;

        let mut form = reqwest::multipart::Form::new().text("payload_json", payload_json);

        for (i, resolved) in ctx.attachments.iter().enumerate() {
            let part = reqwest::multipart::Part::bytes(resolved.data.clone())
                .file_name(resolved.filename.clone())
                .mime_str(&resolved.content_type)
                .unwrap_or_else(|_| {
                    reqwest::multipart::Part::bytes(resolved.data.clone())
                        .file_name(resolved.filename.clone())
                });
            form = form.part(format!("files[{i}]"), part);
        }

        let response = self
            .client
            .post(&url)
            .multipart(form)
            .send()
            .await
            .map_err(DiscordError::Http)?;

        let attachment_count = ctx.attachments.len();
        let result = self.interpret_response(response, attachment_count).await;

        if result.is_ok() {
            info!(attachment_count, "Discord message with attachments sent");
        }

        result
    }

    #[instrument(skip(self), fields(provider = "discord"))]
    async fn health_check(&self) -> Result<(), ProviderError> {
        debug!("performing Discord health check via webhook GET");

        // GET on a Discord webhook URL returns the webhook object.
        let response = self
            .client
            .get(&self.config.webhook_url)
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

        debug!("Discord health check passed");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use acteon_core::Action;
    use acteon_provider::{Provider, ProviderError};

    use super::*;
    use crate::config::DiscordConfig;

    struct MockDiscordServer {
        listener: tokio::net::TcpListener,
        base_url: String,
    }

    impl MockDiscordServer {
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

            let content_type = if body.is_empty() {
                "text/plain"
            } else {
                "application/json"
            };

            let response = format!(
                "HTTP/1.1 {status_code} OK\r\n\
                 Content-Type: {content_type}\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n\
                 {body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
        }

        async fn respond_no_content(self) {
            self.respond_once(204, "").await;
        }

        async fn respond_rate_limited(self) {
            self.respond_once(429, r#"{"message":"rate limited"}"#)
                .await;
        }
    }

    fn make_action(payload: serde_json::Value) -> Action {
        Action::new(
            "notifications",
            "tenant-1",
            "discord",
            "send_message",
            payload,
        )
    }

    #[test]
    fn provider_name() {
        let config = DiscordConfig::new("https://discord.com/api/webhooks/123/abc");
        let provider = DiscordProvider::new(config);
        assert_eq!(provider.name(), "discord");
    }

    #[tokio::test]
    async fn execute_success_no_content_response() {
        let server = MockDiscordServer::start().await;
        let config = DiscordConfig::new(&server.base_url);
        let provider = DiscordProvider::new(config);

        let action = make_action(serde_json::json!({
            "content": "Hello from Acteon!"
        }));

        let server_handle = tokio::spawn(async move {
            server.respond_no_content().await;
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        let response = result.expect("execute should succeed");
        assert_eq!(response.status, acteon_core::ResponseStatus::Success);
        assert_eq!(response.body["ok"], true);
    }

    #[tokio::test]
    async fn execute_success_with_wait() {
        let server = MockDiscordServer::start().await;
        let config = DiscordConfig::new(&server.base_url).with_wait(true);
        let provider = DiscordProvider::new(config);

        let action = make_action(serde_json::json!({
            "content": "Hello!"
        }));

        let response_body = r#"{"id":"12345","channel_id":"67890"}"#;
        let server_handle = tokio::spawn(async move {
            server.respond_once(200, response_body).await;
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        let response = result.expect("execute should succeed");
        assert_eq!(response.body["ok"], true);
        assert_eq!(response.body["id"], "12345");
    }

    #[tokio::test]
    async fn execute_with_embeds() {
        let server = MockDiscordServer::start().await;
        let config = DiscordConfig::new(&server.base_url);
        let provider = DiscordProvider::new(config);

        let action = make_action(serde_json::json!({
            "embeds": [{
                "title": "Alert",
                "description": "Something happened",
                "color": 16711680
            }]
        }));

        let server_handle = tokio::spawn(async move {
            server.respond_no_content().await;
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn execute_with_defaults_applied() {
        let server = MockDiscordServer::start().await;
        let config = DiscordConfig::new(&server.base_url)
            .with_default_username("Acteon Bot")
            .with_default_avatar_url("https://example.com/avatar.png");
        let provider = DiscordProvider::new(config);

        let action = make_action(serde_json::json!({
            "content": "Hello!"
        }));

        let server_handle = tokio::spawn(async move {
            server.respond_no_content().await;
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn execute_missing_content_and_embeds() {
        let config = DiscordConfig::new("http://localhost:1");
        let provider = DiscordProvider::new(config);

        let action = make_action(serde_json::json!({
            "username": "Bot"
        }));

        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn execute_empty_embeds_fails() {
        let config = DiscordConfig::new("http://localhost:1");
        let provider = DiscordProvider::new(config);

        let action = make_action(serde_json::json!({
            "embeds": []
        }));

        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
    }

    #[tokio::test]
    async fn execute_rate_limited() {
        let server = MockDiscordServer::start().await;
        let config = DiscordConfig::new(&server.base_url);
        let provider = DiscordProvider::new(config);

        let action = make_action(serde_json::json!({
            "content": "Hello!"
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
        let server = MockDiscordServer::start().await;
        let config = DiscordConfig::new(&server.base_url);
        let provider = DiscordProvider::new(config);

        let action = make_action(serde_json::json!({
            "content": "Hello!"
        }));

        let server_handle = tokio::spawn(async move {
            server
                .respond_once(400, r#"{"message":"Invalid Webhook Token"}"#)
                .await;
        });

        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();

        assert!(matches!(err, ProviderError::ExecutionFailed(_)));
    }

    #[tokio::test]
    async fn health_check_success() {
        let server = MockDiscordServer::start().await;
        let config = DiscordConfig::new(&server.base_url);
        let provider = DiscordProvider::new(config);

        let response_body = r#"{"type":1,"id":"12345","name":"Acteon","channel_id":"67890"}"#;
        let server_handle = tokio::spawn(async move {
            server.respond_once(200, response_body).await;
        });

        let result = provider.health_check().await;
        server_handle.await.unwrap();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn health_check_rate_limited() {
        let server = MockDiscordServer::start().await;
        let config = DiscordConfig::new(&server.base_url);
        let provider = DiscordProvider::new(config);

        let server_handle = tokio::spawn(async move {
            server.respond_rate_limited().await;
        });

        let err = provider.health_check().await.unwrap_err();
        server_handle.await.unwrap();

        assert!(matches!(err, ProviderError::RateLimited));
    }

    #[test]
    fn effective_url_without_wait() {
        let config = DiscordConfig::new("https://discord.com/api/webhooks/123/abc");
        let provider = DiscordProvider::new(config);
        assert_eq!(
            provider.effective_url(),
            "https://discord.com/api/webhooks/123/abc"
        );
    }

    #[test]
    fn effective_url_with_wait() {
        let config = DiscordConfig::new("https://discord.com/api/webhooks/123/abc").with_wait(true);
        let provider = DiscordProvider::new(config);
        assert_eq!(
            provider.effective_url(),
            "https://discord.com/api/webhooks/123/abc?wait=true"
        );
    }
}
