use acteon_core::{Action, ProviderResponse};
use acteon_provider::{Provider, ProviderError};
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, instrument, warn};

use crate::config::TwilioConfig;
use crate::error::TwilioError;
use crate::types::{TwilioApiResponse, TwilioSendMessageRequest};

/// Twilio provider that sends SMS messages via the Twilio REST API.
///
/// Implements the [`Provider`] trait so it can be registered in the provider
/// registry and used by the action executor.
pub struct TwilioProvider {
    config: TwilioConfig,
    client: Client,
}

/// Fields extracted from an action payload for sending an SMS.
#[derive(Debug, Deserialize)]
struct MessagePayload {
    to: Option<String>,
    body: Option<String>,
    from: Option<String>,
    media_url: Option<String>,
}

impl TwilioProvider {
    /// Create a new Twilio provider with the given configuration.
    ///
    /// Uses a default `reqwest::Client` with reasonable timeouts.
    pub fn new(config: TwilioConfig) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");
        Self { config, client }
    }

    /// Create a new Twilio provider with a custom HTTP client.
    ///
    /// Useful for testing or for sharing a connection pool across providers.
    pub fn with_client(config: TwilioConfig, client: Client) -> Self {
        Self { config, client }
    }

    /// Resolve the "From" phone number from the action payload, falling back to
    /// the configured default.
    fn resolve_from(&self, payload_from: Option<&str>) -> Result<String, TwilioError> {
        payload_from
            .map(String::from)
            .or_else(|| self.config.from_number.clone())
            .ok_or_else(|| {
                TwilioError::InvalidPayload(
                    "no 'from' specified in payload and no default from_number configured".into(),
                )
            })
    }

    /// Build the Messages API URL for this account.
    fn messages_url(&self) -> String {
        format!(
            "{}/2010-04-01/Accounts/{}/Messages.json",
            self.config.api_base_url, self.config.account_sid
        )
    }

    /// Build the Account info URL (used for health checks).
    fn account_url(&self) -> String {
        format!(
            "{}/2010-04-01/Accounts/{}.json",
            self.config.api_base_url, self.config.account_sid
        )
    }

    /// Send an SMS message via the Twilio REST API.
    async fn send_message(
        &self,
        request: &TwilioSendMessageRequest,
    ) -> Result<TwilioApiResponse, TwilioError> {
        let url = self.messages_url();

        debug!(to = %request.to, "sending SMS via Twilio");

        let response = acteon_provider::inject_trace_context(
            self.client
                .post(&url)
                .basic_auth(&self.config.account_sid, Some(&self.config.auth_token))
                .form(request),
        )
        .send()
        .await?;

        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            warn!("Twilio API rate limit hit");
            return Err(TwilioError::RateLimited);
        }

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(TwilioError::Api(format!("HTTP {status}: {body}")));
        }

        let api_response: TwilioApiResponse = response.json().await?;

        if let Some(code) = api_response.error_code {
            let msg = api_response
                .error_message
                .unwrap_or_else(|| format!("error code {code}"));
            return Err(TwilioError::Api(msg));
        }

        Ok(api_response)
    }
}

impl Provider for TwilioProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "twilio"
    }

    #[instrument(skip(self, action), fields(action_id = %action.id, provider = "twilio"))]
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        let payload: MessagePayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| TwilioError::InvalidPayload(format!("failed to parse payload: {e}")))?;

        let to = payload.to.ok_or_else(|| {
            TwilioError::InvalidPayload("payload must contain a 'to' phone number".into())
        })?;

        let body = payload.body.ok_or_else(|| {
            TwilioError::InvalidPayload("payload must contain a 'body' message text".into())
        })?;

        let from = self.resolve_from(payload.from.as_deref())?;

        let request = TwilioSendMessageRequest {
            to,
            from,
            body,
            media_url: payload.media_url,
        };

        let api_response = self.send_message(&request).await?;

        let response_body = serde_json::json!({
            "sid": api_response.sid,
            "status": api_response.status,
        });

        Ok(ProviderResponse::success(response_body))
    }

    #[instrument(skip(self), fields(provider = "twilio"))]
    async fn health_check(&self) -> Result<(), ProviderError> {
        let url = self.account_url();

        debug!("performing Twilio health check via account lookup");

        let response = self
            .client
            .get(&url)
            .basic_auth(&self.config.account_sid, Some(&self.config.auth_token))
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

        debug!("Twilio health check passed");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use acteon_core::Action;
    use acteon_provider::{Provider, ProviderError};

    use super::*;
    use crate::config::TwilioConfig;

    /// A minimal mock HTTP server built on tokio that returns canned responses.
    struct MockTwilioServer {
        listener: tokio::net::TcpListener,
        base_url: String,
    }

    impl MockTwilioServer {
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

        async fn respond_rate_limited(self) {
            let body = r#"{"error_code":429,"error_message":"rate limited"}"#;
            self.respond_once(429, body).await;
        }
    }

    fn make_action(payload: serde_json::Value) -> Action {
        Action::new("notifications", "tenant-1", "twilio", "send_sms", payload)
    }

    #[test]
    fn provider_name() {
        let config = TwilioConfig::new("AC123", "token");
        let provider = TwilioProvider::new(config);
        assert_eq!(provider.name(), "twilio");
    }

    #[tokio::test]
    async fn execute_success() {
        let server = MockTwilioServer::start().await;
        let config = TwilioConfig::new("AC123", "token")
            .with_api_base_url(&server.base_url)
            .with_from_number("+15551234567");
        let provider = TwilioProvider::new(config);

        let action = make_action(serde_json::json!({
            "to": "+15559876543",
            "body": "Hello from Acteon!"
        }));

        let response_body =
            r#"{"sid":"SM123","status":"queued","error_code":null,"error_message":null}"#;
        let server_handle = tokio::spawn(async move {
            server.respond_once(200, response_body).await;
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        let response = result.expect("execute should succeed");
        assert_eq!(response.status, acteon_core::ResponseStatus::Success);
        assert_eq!(response.body["sid"], "SM123");
        assert_eq!(response.body["status"], "queued");
    }

    #[tokio::test]
    async fn execute_with_explicit_from() {
        let server = MockTwilioServer::start().await;
        let config = TwilioConfig::new("AC123", "token").with_api_base_url(&server.base_url);
        let provider = TwilioProvider::new(config);

        let action = make_action(serde_json::json!({
            "to": "+15559876543",
            "from": "+15550001111",
            "body": "Hello!"
        }));

        let response_body =
            r#"{"sid":"SM456","status":"queued","error_code":null,"error_message":null}"#;
        let server_handle = tokio::spawn(async move {
            server.respond_once(200, response_body).await;
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn execute_missing_to_field() {
        let config = TwilioConfig::new("AC123", "token").with_api_base_url("http://localhost:1");
        let provider = TwilioProvider::new(config);

        let action = make_action(serde_json::json!({
            "body": "Hello!"
        }));

        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn execute_missing_body_field() {
        let config = TwilioConfig::new("AC123", "token").with_api_base_url("http://localhost:1");
        let provider = TwilioProvider::new(config);

        let action = make_action(serde_json::json!({
            "to": "+15559876543",
            "from": "+15551234567"
        }));

        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn execute_missing_from_and_no_default() {
        let config = TwilioConfig::new("AC123", "token").with_api_base_url("http://localhost:1");
        let provider = TwilioProvider::new(config);

        let action = make_action(serde_json::json!({
            "to": "+15559876543",
            "body": "Hello!"
        }));

        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn execute_rate_limited_is_retryable() {
        let server = MockTwilioServer::start().await;
        let config = TwilioConfig::new("AC123", "token")
            .with_api_base_url(&server.base_url)
            .with_from_number("+15551234567");
        let provider = TwilioProvider::new(config);

        let action = make_action(serde_json::json!({
            "to": "+15559876543",
            "body": "Hello!"
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
    async fn execute_api_error_not_retryable() {
        let server = MockTwilioServer::start().await;
        let config = TwilioConfig::new("AC123", "bad-token")
            .with_api_base_url(&server.base_url)
            .with_from_number("+15551234567");
        let provider = TwilioProvider::new(config);

        let action = make_action(serde_json::json!({
            "to": "+15559876543",
            "body": "Hello!"
        }));

        let response_body = r#"{"sid":null,"status":null,"error_code":20003,"error_message":"Authentication Error"}"#;
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
        let server = MockTwilioServer::start().await;
        let config = TwilioConfig::new("AC123", "token").with_api_base_url(&server.base_url);
        let provider = TwilioProvider::new(config);

        let response_body = r#"{"sid":"AC123","friendly_name":"My Account","status":"active"}"#;
        let server_handle = tokio::spawn(async move {
            server.respond_once(200, response_body).await;
        });

        let result = provider.health_check().await;
        server_handle.await.unwrap();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn health_check_rate_limited() {
        let server = MockTwilioServer::start().await;
        let config = TwilioConfig::new("AC123", "token").with_api_base_url(&server.base_url);
        let provider = TwilioProvider::new(config);

        let server_handle = tokio::spawn(async move {
            server.respond_rate_limited().await;
        });

        let err = provider.health_check().await.unwrap_err();
        server_handle.await.unwrap();

        assert!(matches!(err, ProviderError::RateLimited));
        assert!(err.is_retryable());
    }
}
