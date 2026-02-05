use acteon_core::{Action, ProviderResponse};
use acteon_provider::{Provider, ProviderError};
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, instrument, warn};

use crate::config::SlackConfig;
use crate::error::SlackError;
use crate::types::{SlackApiResponse, SlackAuthTestResponse, SlackPostMessageRequest};

/// Slack provider that sends messages via the Slack Web API.
///
/// Implements the [`Provider`] trait so it can be registered in the provider
/// registry and used by the action executor.
pub struct SlackProvider {
    config: SlackConfig,
    client: Client,
}

/// Fields extracted from an action payload for the `chat.postMessage` call.
#[derive(Debug, Deserialize)]
struct MessagePayload {
    channel: Option<String>,
    text: Option<String>,
    blocks: Option<serde_json::Value>,
}

impl SlackProvider {
    /// Create a new Slack provider with the given configuration.
    ///
    /// Uses a default `reqwest::Client` with reasonable timeouts.
    pub fn new(config: SlackConfig) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");
        Self { config, client }
    }

    /// Create a new Slack provider with a custom HTTP client.
    ///
    /// Useful for testing or for sharing a connection pool across providers.
    pub fn with_client(config: SlackConfig, client: Client) -> Self {
        Self { config, client }
    }

    /// Resolve the target channel from the action payload, falling back to
    /// the configured default.
    fn resolve_channel(&self, payload_channel: Option<&str>) -> Result<String, SlackError> {
        payload_channel
            .map(String::from)
            .or_else(|| self.config.default_channel.clone())
            .ok_or_else(|| {
                SlackError::InvalidPayload(
                    "no channel specified in payload and no default channel configured".into(),
                )
            })
    }

    /// Build the full URL for a Slack API method.
    fn api_url(&self, method: &str) -> String {
        format!("{}/{method}", self.config.api_base_url)
    }

    /// Send a `chat.postMessage` request to the Slack Web API and interpret
    /// the response.
    async fn post_message(
        &self,
        request: &SlackPostMessageRequest,
    ) -> Result<SlackApiResponse, SlackError> {
        let url = self.api_url("chat.postMessage");

        debug!(channel = %request.channel, "posting message to Slack");

        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.config.token)
            .json(request)
            .send()
            .await?;

        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            warn!("Slack API rate limit hit");
            return Err(SlackError::RateLimited);
        }

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(SlackError::Api(format!("HTTP {status}: {body}")));
        }

        let api_response: SlackApiResponse = response.json().await?;

        if !api_response.ok {
            let error_code = api_response
                .error
                .unwrap_or_else(|| "unknown_error".to_owned());
            return Err(SlackError::Api(error_code));
        }

        Ok(api_response)
    }
}

impl Provider for SlackProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "slack"
    }

    #[instrument(skip(self, action), fields(action_id = %action.id, provider = "slack"))]
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        let payload: MessagePayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| SlackError::InvalidPayload(format!("failed to parse payload: {e}")))?;

        let channel = self.resolve_channel(payload.channel.as_deref())?;

        if payload.text.is_none() && payload.blocks.is_none() {
            return Err(SlackError::InvalidPayload(
                "payload must contain at least one of 'text' or 'blocks'".into(),
            )
            .into());
        }

        let request = SlackPostMessageRequest {
            channel,
            text: payload.text,
            blocks: payload.blocks,
        };

        let api_response = self.post_message(&request).await?;

        let body = serde_json::json!({
            "ok": api_response.ok,
            "channel": api_response.channel,
            "ts": api_response.ts,
        });

        Ok(ProviderResponse::success(body))
    }

    #[instrument(skip(self), fields(provider = "slack"))]
    async fn health_check(&self) -> Result<(), ProviderError> {
        let url = self.api_url("auth.test");

        debug!("performing Slack health check via auth.test");

        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.config.token)
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

        let auth_response: SlackAuthTestResponse = response.json().await.map_err(|e| {
            ProviderError::Connection(format!("failed to parse auth.test response: {e}"))
        })?;

        if !auth_response.ok {
            let error_code = auth_response
                .error
                .unwrap_or_else(|| "unknown_error".to_owned());
            return Err(ProviderError::Configuration(format!(
                "Slack auth.test failed: {error_code}"
            )));
        }

        debug!(
            user_id = auth_response.user_id.as_deref().unwrap_or("unknown"),
            team_id = auth_response.team_id.as_deref().unwrap_or("unknown"),
            "Slack health check passed"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use acteon_core::Action;
    use acteon_provider::{Provider, ProviderError};

    use super::*;
    use crate::config::SlackConfig;

    /// A minimal mock HTTP server built on tokio that returns canned responses.
    struct MockSlackServer {
        listener: tokio::net::TcpListener,
        base_url: String,
    }

    impl MockSlackServer {
        async fn start() -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("failed to bind mock server");
            let port = listener.local_addr().unwrap().port();
            let base_url = format!("http://127.0.0.1:{port}");
            Self { listener, base_url }
        }

        /// Accept one connection and respond with the given status code and JSON
        /// body, then shut down.
        async fn respond_once(self, status_code: u16, body: &str) {
            let body = body.to_owned();
            let (mut stream, _) = self.listener.accept().await.unwrap();

            use tokio::io::{AsyncReadExt, AsyncWriteExt};

            // Read the full request (we don't parse it -- just drain it).
            let mut buf = vec![0u8; 8192];
            let _ = stream.read(&mut buf).await.unwrap();

            let response = format!(
                "HTTP/1.1 {status_code} OK\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n\
                 {body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
        }

        /// Accept one connection and respond with HTTP 429 (rate limited).
        async fn respond_rate_limited(self) {
            let body = r#"{"ok":false,"error":"rate_limited"}"#;
            self.respond_once(429, body).await;
        }
    }

    fn make_action(payload: serde_json::Value) -> Action {
        Action::new(
            "notifications",
            "tenant-1",
            "slack",
            "send_message",
            payload,
        )
    }

    #[test]
    fn provider_name() {
        let config = SlackConfig::new("xoxb-test");
        let provider = SlackProvider::new(config);
        assert_eq!(provider.name(), "slack");
    }

    #[tokio::test]
    async fn execute_success() {
        let server = MockSlackServer::start().await;
        let config = SlackConfig::new("xoxb-test").with_api_base_url(&server.base_url);
        let provider = SlackProvider::new(config);

        let action = make_action(serde_json::json!({
            "channel": "#general",
            "text": "Hello from Acteon!"
        }));

        let response_body = r#"{"ok":true,"channel":"C12345","ts":"1234567890.123456"}"#;
        let server_handle = tokio::spawn(async move {
            server.respond_once(200, response_body).await;
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        let response = result.expect("execute should succeed");
        assert_eq!(response.status, acteon_core::ResponseStatus::Success);
        assert_eq!(response.body["ok"], true);
        assert_eq!(response.body["channel"], "C12345");
    }

    #[tokio::test]
    async fn execute_with_blocks() {
        let server = MockSlackServer::start().await;
        let config = SlackConfig::new("xoxb-test").with_api_base_url(&server.base_url);
        let provider = SlackProvider::new(config);

        let action = make_action(serde_json::json!({
            "channel": "#general",
            "blocks": [{"type": "section", "text": {"type": "mrkdwn", "text": "hello"}}]
        }));

        let response_body = r#"{"ok":true,"channel":"C12345","ts":"111.222"}"#;
        let server_handle = tokio::spawn(async move {
            server.respond_once(200, response_body).await;
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn execute_uses_default_channel() {
        let server = MockSlackServer::start().await;
        let config = SlackConfig::new("xoxb-test")
            .with_api_base_url(&server.base_url)
            .with_default_channel("#fallback");
        let provider = SlackProvider::new(config);

        // Payload without an explicit channel.
        let action = make_action(serde_json::json!({
            "text": "Hello!"
        }));

        let response_body = r#"{"ok":true,"channel":"C99999","ts":"999.000"}"#;
        let server_handle = tokio::spawn(async move {
            server.respond_once(200, response_body).await;
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn execute_missing_channel_and_no_default() {
        let config = SlackConfig::new("xoxb-test").with_api_base_url("http://localhost:1");
        let provider = SlackProvider::new(config);

        let action = make_action(serde_json::json!({
            "text": "Hello!"
        }));

        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn execute_missing_text_and_blocks() {
        let config = SlackConfig::new("xoxb-test").with_api_base_url("http://localhost:1");
        let provider = SlackProvider::new(config);

        let action = make_action(serde_json::json!({
            "channel": "#general"
        }));

        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn execute_rate_limited_is_retryable() {
        let server = MockSlackServer::start().await;
        let config = SlackConfig::new("xoxb-test").with_api_base_url(&server.base_url);
        let provider = SlackProvider::new(config);

        let action = make_action(serde_json::json!({
            "channel": "#general",
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
    async fn execute_slack_api_error_not_retryable() {
        let server = MockSlackServer::start().await;
        let config = SlackConfig::new("xoxb-bad-token").with_api_base_url(&server.base_url);
        let provider = SlackProvider::new(config);

        let action = make_action(serde_json::json!({
            "channel": "#general",
            "text": "Hello!"
        }));

        let response_body = r#"{"ok":false,"error":"invalid_auth"}"#;
        let server_handle = tokio::spawn(async move {
            server.respond_once(200, response_body).await;
        });

        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();

        assert!(matches!(err, ProviderError::ExecutionFailed(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn health_check_success() {
        let server = MockSlackServer::start().await;
        let config = SlackConfig::new("xoxb-test").with_api_base_url(&server.base_url);
        let provider = SlackProvider::new(config);

        let response_body = r#"{"ok":true,"user_id":"U12345","team_id":"T12345"}"#;
        let server_handle = tokio::spawn(async move {
            server.respond_once(200, response_body).await;
        });

        let result = provider.health_check().await;
        server_handle.await.unwrap();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn health_check_invalid_token() {
        let server = MockSlackServer::start().await;
        let config = SlackConfig::new("xoxb-bad").with_api_base_url(&server.base_url);
        let provider = SlackProvider::new(config);

        let response_body = r#"{"ok":false,"error":"invalid_auth"}"#;
        let server_handle = tokio::spawn(async move {
            server.respond_once(200, response_body).await;
        });

        let err = provider.health_check().await.unwrap_err();
        server_handle.await.unwrap();

        assert!(matches!(err, ProviderError::Configuration(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn health_check_rate_limited() {
        let server = MockSlackServer::start().await;
        let config = SlackConfig::new("xoxb-test").with_api_base_url(&server.base_url);
        let provider = SlackProvider::new(config);

        let server_handle = tokio::spawn(async move {
            server.respond_rate_limited().await;
        });

        let err = provider.health_check().await.unwrap_err();
        server_handle.await.unwrap();

        assert!(matches!(err, ProviderError::RateLimited));
        assert!(err.is_retryable());
    }
}
