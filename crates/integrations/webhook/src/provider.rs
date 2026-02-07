use acteon_core::{Action, ProviderResponse};
use acteon_provider::{Provider, ProviderError};
use hmac::{Hmac, Mac};
use reqwest::Client;
use sha2::Sha256;
use tracing::{debug, instrument, warn};

use crate::config::{AuthMethod, HttpMethod, PayloadMode, WebhookConfig};
use crate::error::WebhookError;

type HmacSha256 = Hmac<Sha256>;

/// Generic HTTP webhook provider that dispatches actions to any HTTP endpoint.
///
/// Implements the [`Provider`] trait so it can be registered in the provider
/// registry and used by the action executor. Supports multiple authentication
/// methods, configurable HTTP methods, payload transforms, and response
/// validation.
pub struct WebhookProvider {
    /// Unique name for this provider instance.
    provider_name: String,
    config: WebhookConfig,
    client: Client,
}

impl WebhookProvider {
    /// Create a new webhook provider with the given name and configuration.
    ///
    /// Uses a default `reqwest::Client` with the configured timeout.
    pub fn new(name: impl Into<String>, config: WebhookConfig) -> Self {
        let client = Client::builder()
            .timeout(config.timeout)
            .redirect(if config.follow_redirects {
                reqwest::redirect::Policy::default()
            } else {
                reqwest::redirect::Policy::none()
            })
            .build()
            .expect("failed to build HTTP client");

        Self {
            provider_name: name.into(),
            config,
            client,
        }
    }

    /// Create a new webhook provider with a custom HTTP client.
    ///
    /// Useful for testing or for sharing a connection pool across providers.
    pub fn with_client(name: impl Into<String>, config: WebhookConfig, client: Client) -> Self {
        Self {
            provider_name: name.into(),
            config,
            client,
        }
    }

    /// Build the request body based on the configured payload mode.
    fn build_body(&self, action: &Action) -> Result<serde_json::Value, WebhookError> {
        match self.config.payload_mode {
            PayloadMode::FullAction => serde_json::to_value(action).map_err(|e| {
                WebhookError::InvalidPayload(format!("failed to serialize action: {e}"))
            }),
            PayloadMode::PayloadOnly => Ok(action.payload.clone()),
        }
    }

    /// Compute the HMAC-SHA256 signature of the request body.
    fn compute_hmac(secret: &str, body: &[u8]) -> Result<String, WebhookError> {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|e| WebhookError::SigningError(format!("invalid HMAC key: {e}")))?;
        mac.update(body);
        let result = mac.finalize();
        Ok(hex::encode(result.into_bytes()))
    }

    /// Apply authentication to the request builder.
    fn apply_auth(
        &self,
        mut request: reqwest::RequestBuilder,
        body_bytes: &[u8],
    ) -> Result<reqwest::RequestBuilder, WebhookError> {
        match &self.config.auth {
            Some(AuthMethod::Bearer(token)) => {
                request = request.bearer_auth(token);
            }
            Some(AuthMethod::Basic { username, password }) => {
                request = request.basic_auth(username, Some(password));
            }
            Some(AuthMethod::ApiKey { header, value }) => {
                request = request.header(header, value);
            }
            Some(AuthMethod::HmacSha256 { secret, header }) => {
                let signature = Self::compute_hmac(secret, body_bytes)?;
                request = request.header(header, format!("sha256={signature}"));
            }
            None => {}
        }
        Ok(request)
    }

    /// Check whether the status code indicates success based on configuration.
    fn is_success_status(&self, status: u16) -> bool {
        if self.config.success_status_codes.is_empty() {
            (200..300).contains(&status)
        } else {
            self.config.success_status_codes.contains(&status)
        }
    }

    /// Build a `reqwest::RequestBuilder` for the configured method and URL.
    fn build_request(&self) -> reqwest::RequestBuilder {
        match self.config.method {
            HttpMethod::Get => self.client.get(&self.config.url),
            HttpMethod::Post => self.client.post(&self.config.url),
            HttpMethod::Put => self.client.put(&self.config.url),
            HttpMethod::Patch => self.client.patch(&self.config.url),
            HttpMethod::Delete => self.client.delete(&self.config.url),
        }
    }
}

impl Provider for WebhookProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        &self.provider_name
    }

    #[instrument(skip(self, action), fields(action_id = %action.id, provider = %self.provider_name))]
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        let body = self.build_body(action)?;
        let body_bytes =
            serde_json::to_vec(&body).map_err(|e| WebhookError::InvalidPayload(e.to_string()))?;

        debug!(
            method = self.config.method.as_str(),
            url = %self.config.url,
            "dispatching webhook"
        );

        let mut request = self.build_request();

        // Set content type and body.
        request = request
            .header("Content-Type", "application/json")
            .body(body_bytes.clone());

        // Apply static headers.
        for (key, value) in &self.config.headers {
            request = request.header(key, value);
        }

        // Apply authentication (may depend on body bytes for HMAC).
        request = self.apply_auth(request, &body_bytes)?;

        // Send the request.
        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                warn!("webhook request timed out");
                WebhookError::Http(e)
            } else {
                WebhookError::Http(e)
            }
        })?;

        let status = response.status();
        let status_code = status.as_u16();

        // Collect response headers.
        let response_headers: std::collections::HashMap<String, String> = response
            .headers()
            .iter()
            .filter_map(|(k, v)| v.to_str().ok().map(|val| (k.to_string(), val.to_owned())))
            .collect();

        if status_code == 429 {
            warn!("webhook endpoint returned 429");
            return Err(WebhookError::RateLimited.into());
        }

        // Parse response body (best-effort JSON, fallback to text).
        let response_text = response.text().await.unwrap_or_default();
        let response_body: serde_json::Value =
            serde_json::from_str(&response_text).unwrap_or_else(|_| {
                serde_json::json!({
                    "status_code": status_code,
                    "body": response_text,
                })
            });

        if self.is_success_status(status_code) {
            let mut provider_response = ProviderResponse::success(response_body);
            provider_response.headers = response_headers;
            Ok(provider_response)
        } else {
            let mut provider_response = ProviderResponse::failure(response_body);
            provider_response.headers = response_headers;
            Ok(provider_response)
        }
    }

    #[instrument(skip(self), fields(provider = %self.provider_name))]
    async fn health_check(&self) -> Result<(), ProviderError> {
        debug!(url = %self.config.url, "performing webhook health check");

        let response = self
            .client
            .head(&self.config.url)
            .send()
            .await
            .map_err(|e| ProviderError::Connection(e.to_string()))?;

        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ProviderError::RateLimited);
        }

        if status.is_server_error() {
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::Connection(format!(
                "health check failed: HTTP {status}: {body}"
            )));
        }

        debug!("webhook health check passed");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use acteon_core::Action;
    use acteon_provider::{Provider, ProviderError};

    use super::*;
    use crate::config::{AuthMethod, HttpMethod, PayloadMode, WebhookConfig};

    /// A minimal mock HTTP server built on tokio that returns canned responses.
    struct MockWebhookServer {
        listener: tokio::net::TcpListener,
        base_url: String,
    }

    impl MockWebhookServer {
        async fn start() -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("failed to bind mock server");
            let port = listener.local_addr().unwrap().port();
            let base_url = format!("http://127.0.0.1:{port}");
            Self { listener, base_url }
        }

        /// Accept one connection and respond with the given status code and JSON
        /// body, then shut down. Returns the raw request bytes.
        async fn respond_once(self, status_code: u16, body: &str) -> Vec<u8> {
            let body = body.to_owned();
            let (mut stream, _) = self.listener.accept().await.unwrap();

            use tokio::io::{AsyncReadExt, AsyncWriteExt};

            let mut buf = vec![0u8; 16384];
            let n = stream.read(&mut buf).await.unwrap();
            buf.truncate(n);

            let response = format!(
                "HTTP/1.1 {status_code} OK\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\
                 X-Request-Id: test-123\r\n\
                 Connection: close\r\n\
                 \r\n\
                 {body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();

            buf
        }

        /// Accept one connection and respond with HTTP 429 (rate limited).
        async fn respond_rate_limited(self) {
            let body = r#"{"error":"rate_limited"}"#;
            self.respond_once(429, body).await;
        }
    }

    fn make_action(payload: serde_json::Value) -> Action {
        Action::new(
            "notifications",
            "tenant-1",
            "webhook",
            "send_event",
            payload,
        )
    }

    #[test]
    fn provider_name() {
        let config = WebhookConfig::new("https://example.com/hook");
        let provider = WebhookProvider::new("test-hook", config);
        assert_eq!(Provider::name(&provider), "test-hook");
    }

    #[tokio::test]
    async fn execute_success_post() {
        let server = MockWebhookServer::start().await;
        let config = WebhookConfig::new(&server.base_url);
        let provider = WebhookProvider::new("post-hook", config);

        let action = make_action(serde_json::json!({"event": "test"}));

        let response_body = r#"{"received": true}"#;
        let server_handle =
            tokio::spawn(async move { server.respond_once(200, response_body).await });

        let result = provider.execute(&action).await;
        let _request_bytes = server_handle.await.unwrap();

        let response = result.expect("execute should succeed");
        assert_eq!(response.status, acteon_core::ResponseStatus::Success);
        assert_eq!(response.body["received"], true);
    }

    #[tokio::test]
    async fn execute_success_put() {
        let server = MockWebhookServer::start().await;
        let config = WebhookConfig::new(&server.base_url).with_method(HttpMethod::Put);
        let provider = WebhookProvider::new("put-hook", config);

        let action = make_action(serde_json::json!({"event": "update"}));

        let response_body = r#"{"updated": true}"#;
        let server_handle = tokio::spawn(async move {
            let request = server.respond_once(200, response_body).await;
            let request_str = String::from_utf8_lossy(&request);
            assert!(request_str.starts_with("PUT "));
            request
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn execute_payload_only_mode() {
        let server = MockWebhookServer::start().await;
        let config =
            WebhookConfig::new(&server.base_url).with_payload_mode(PayloadMode::PayloadOnly);
        let provider = WebhookProvider::new("payload-hook", config);

        let action = make_action(serde_json::json!({"event": "minimal"}));

        let response_body = r#"{"ok": true}"#;
        let server_handle = tokio::spawn(async move {
            let request = server.respond_once(200, response_body).await;
            let request_str = String::from_utf8_lossy(&request);
            // In payload-only mode, the body should be just the payload, not the
            // full action envelope.
            assert!(
                !request_str.contains("namespace"),
                "payload-only mode should not include action envelope"
            );
            request
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn execute_with_bearer_auth() {
        let server = MockWebhookServer::start().await;
        let config = WebhookConfig::new(&server.base_url)
            .with_auth(AuthMethod::Bearer("my-secret-token".into()));
        let provider = WebhookProvider::new("bearer-hook", config);

        let action = make_action(serde_json::json!({"event": "secure"}));

        let response_body = r#"{"ok": true}"#;
        let server_handle = tokio::spawn(async move {
            let request = server.respond_once(200, response_body).await;
            let request_str = String::from_utf8_lossy(&request);
            assert!(
                request_str.contains("Bearer my-secret-token"),
                "request should contain Bearer token"
            );
            request
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn execute_with_api_key_auth() {
        let server = MockWebhookServer::start().await;
        let config = WebhookConfig::new(&server.base_url).with_auth(AuthMethod::ApiKey {
            header: "X-API-Key".into(),
            value: "key-12345".into(),
        });
        let provider = WebhookProvider::new("apikey-hook", config);

        let action = make_action(serde_json::json!({"event": "test"}));

        let response_body = r#"{"ok": true}"#;
        let server_handle = tokio::spawn(async move {
            let request = server.respond_once(200, response_body).await;
            let request_str = String::from_utf8_lossy(&request).to_lowercase();
            assert!(
                request_str.contains("x-api-key: key-12345"),
                "request should contain API key header"
            );
            request
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn execute_with_hmac_auth() {
        let server = MockWebhookServer::start().await;
        let config = WebhookConfig::new(&server.base_url).with_auth(AuthMethod::HmacSha256 {
            secret: "webhook-secret".into(),
            header: "X-Signature".into(),
        });
        let provider = WebhookProvider::new("hmac-hook", config);

        let action = make_action(serde_json::json!({"event": "signed"}));

        let response_body = r#"{"ok": true}"#;
        let server_handle = tokio::spawn(async move {
            let request = server.respond_once(200, response_body).await;
            let request_str = String::from_utf8_lossy(&request).to_lowercase();
            assert!(
                request_str.contains("x-signature: sha256="),
                "request should contain HMAC signature header"
            );
            request
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn execute_with_custom_headers() {
        let server = MockWebhookServer::start().await;
        let config = WebhookConfig::new(&server.base_url)
            .with_header("X-Custom-One", "value1")
            .with_header("X-Custom-Two", "value2");
        let provider = WebhookProvider::new("header-hook", config);

        let action = make_action(serde_json::json!({"event": "test"}));

        let response_body = r#"{"ok": true}"#;
        let server_handle = tokio::spawn(async move {
            let request = server.respond_once(200, response_body).await;
            let request_str = String::from_utf8_lossy(&request).to_lowercase();
            assert!(request_str.contains("x-custom-one: value1"));
            assert!(request_str.contains("x-custom-two: value2"));
            request
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn execute_rate_limited_is_retryable() {
        let server = MockWebhookServer::start().await;
        let config = WebhookConfig::new(&server.base_url);
        let provider = WebhookProvider::new("rate-hook", config);

        let action = make_action(serde_json::json!({"event": "test"}));

        let server_handle = tokio::spawn(async move {
            server.respond_rate_limited().await;
        });

        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();

        assert!(matches!(err, ProviderError::RateLimited));
        assert!(err.is_retryable());
    }

    #[tokio::test]
    async fn execute_server_error_returns_failure_response() {
        let server = MockWebhookServer::start().await;
        let config = WebhookConfig::new(&server.base_url);
        let provider = WebhookProvider::new("err-hook", config);

        let action = make_action(serde_json::json!({"event": "test"}));

        let response_body = r#"{"error":"internal server error"}"#;
        let server_handle =
            tokio::spawn(async move { server.respond_once(500, response_body).await });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        let response = result.expect("should return response, not error");
        assert_eq!(response.status, acteon_core::ResponseStatus::Failure);
    }

    #[tokio::test]
    async fn execute_custom_success_codes() {
        let server = MockWebhookServer::start().await;
        let config =
            WebhookConfig::new(&server.base_url).with_success_status_codes(vec![200, 201, 202]);
        let provider = WebhookProvider::new("custom-status", config);

        let action = make_action(serde_json::json!({"event": "test"}));

        let response_body = r#"{"queued": true}"#;
        let server_handle =
            tokio::spawn(async move { server.respond_once(202, response_body).await });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        let response = result.expect("execute should succeed");
        assert_eq!(response.status, acteon_core::ResponseStatus::Success);
    }

    #[tokio::test]
    async fn execute_captures_response_headers() {
        let server = MockWebhookServer::start().await;
        let config = WebhookConfig::new(&server.base_url);
        let provider = WebhookProvider::new("headers-hook", config);

        let action = make_action(serde_json::json!({"event": "test"}));

        let response_body = r#"{"ok": true}"#;
        let server_handle =
            tokio::spawn(async move { server.respond_once(200, response_body).await });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        let response = result.expect("execute should succeed");
        assert_eq!(response.headers.get("x-request-id").unwrap(), "test-123");
    }

    #[tokio::test]
    async fn health_check_success() {
        let server = MockWebhookServer::start().await;
        let config = WebhookConfig::new(&server.base_url);
        let provider = WebhookProvider::new("health-hook", config);

        let server_handle = tokio::spawn(async move { server.respond_once(200, "").await });

        let result = provider.health_check().await;
        server_handle.await.unwrap();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn health_check_rate_limited() {
        let server = MockWebhookServer::start().await;
        let config = WebhookConfig::new(&server.base_url);
        let provider = WebhookProvider::new("health-rate", config);

        let server_handle = tokio::spawn(async move {
            server.respond_rate_limited().await;
        });

        let err = provider.health_check().await.unwrap_err();
        server_handle.await.unwrap();

        assert!(matches!(err, ProviderError::RateLimited));
        assert!(err.is_retryable());
    }

    #[tokio::test]
    async fn health_check_server_error() {
        let server = MockWebhookServer::start().await;
        let config = WebhookConfig::new(&server.base_url);
        let provider = WebhookProvider::new("health-err", config);

        let server_handle =
            tokio::spawn(
                async move { server.respond_once(503, r#"{"error":"unavailable"}"#).await },
            );

        let err = provider.health_check().await.unwrap_err();
        server_handle.await.unwrap();

        assert!(matches!(err, ProviderError::Connection(_)));
        assert!(err.is_retryable());
    }

    #[test]
    fn compute_hmac_produces_hex_string() {
        let signature = WebhookProvider::compute_hmac("secret", b"hello world").unwrap();
        // Verify it's a valid hex string of the expected length (64 chars for SHA-256).
        assert_eq!(signature.len(), 64);
        assert!(signature.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn compute_hmac_deterministic() {
        let sig1 = WebhookProvider::compute_hmac("secret", b"data").unwrap();
        let sig2 = WebhookProvider::compute_hmac("secret", b"data").unwrap();
        assert_eq!(sig1, sig2);
    }

    #[test]
    fn compute_hmac_different_secrets() {
        let sig1 = WebhookProvider::compute_hmac("secret1", b"data").unwrap();
        let sig2 = WebhookProvider::compute_hmac("secret2", b"data").unwrap();
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn is_success_status_default() {
        let config = WebhookConfig::new("https://example.com");
        let provider = WebhookProvider::new("test", config);

        assert!(provider.is_success_status(200));
        assert!(provider.is_success_status(201));
        assert!(provider.is_success_status(204));
        assert!(!provider.is_success_status(301));
        assert!(!provider.is_success_status(400));
        assert!(!provider.is_success_status(500));
    }

    #[test]
    fn is_success_status_custom() {
        let config =
            WebhookConfig::new("https://example.com").with_success_status_codes(vec![200, 202]);
        let provider = WebhookProvider::new("test", config);

        assert!(provider.is_success_status(200));
        assert!(provider.is_success_status(202));
        assert!(!provider.is_success_status(201));
        assert!(!provider.is_success_status(204));
    }
}
