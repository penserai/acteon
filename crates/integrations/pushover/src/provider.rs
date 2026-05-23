use acteon_core::{Action, ProviderResponse};
use acteon_provider::{Provider, ProviderError, truncate_error_body};
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, instrument, warn};

use crate::config::PushoverConfig;
use crate::error::PushoverError;
use crate::types::{PushoverApiResponse, PushoverPriority, PushoverRequest};

/// Maximum length of the `message` field accepted by the Pushover
/// API as of the time this crate was written (1024 UTF-8 bytes).
/// Longer messages are truncated client-side — we'd rather ship
/// a slightly shortened notification than have the API reject the
/// whole call.
const MESSAGE_MAX_BYTES: usize = 1024;

/// Maximum length of the `title` field (250 UTF-8 bytes).
const TITLE_MAX_BYTES: usize = 250;

/// Maximum length of the `url` field (512 UTF-8 bytes).
const URL_MAX_BYTES: usize = 512;

/// Maximum length of the `url_title` field (100 UTF-8 bytes).
const URL_TITLE_MAX_BYTES: usize = 100;

/// Pushover provider that posts messages to the Messages API.
pub struct PushoverProvider {
    config: PushoverConfig,
    client: Client,
}

/// Fields extracted from an action payload.
#[derive(Debug, Deserialize)]
struct EventPayload {
    /// Optional — defaults to `"send"`. Present for symmetry with the
    /// other receivers that have a real lifecycle; Pushover itself has
    /// only one action (deliver a notification).
    #[serde(default)]
    event_action: Option<String>,
    #[serde(default)]
    user_key: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    priority: Option<i32>,
    #[serde(default)]
    retry: Option<u32>,
    #[serde(default)]
    expire: Option<u32>,
    #[serde(default)]
    sound: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    url_title: Option<String>,
    #[serde(default)]
    device: Option<String>,
    #[serde(default)]
    html: Option<bool>,
    #[serde(default)]
    monospace: Option<bool>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    ttl: Option<u32>,
}

impl PushoverProvider {
    /// Create a new `Pushover` provider with a default HTTP client
    /// (30-second timeout).
    pub fn new(config: PushoverConfig) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");
        Self { config, client }
    }

    /// Create a new provider with a custom HTTP client — useful for
    /// tests and for sharing a connection pool across providers.
    pub fn with_client(config: PushoverConfig, client: Client) -> Self {
        Self { config, client }
    }

    fn messages_url(&self) -> String {
        format!("{}/1/messages.json", self.config.api_base_url())
    }

    /// Truncate a string to `max_bytes` UTF-8 bytes without splitting
    /// a multi-byte character. Returns the input unchanged when it
    /// already fits.
    fn truncate_utf8(s: String, max_bytes: usize) -> String {
        if s.len() <= max_bytes {
            return s;
        }
        let mut cut = max_bytes;
        // Walk backwards to the nearest UTF-8 char boundary so we
        // never hand the API a partially-cut multi-byte character.
        while cut > 0 && !s.is_char_boundary(cut) {
            cut -= 1;
        }
        let mut out = s;
        out.truncate(cut);
        out
    }

    /// Build a [`PushoverRequest`] from the payload, applying
    /// config defaults and client-side validation.
    fn build_request(&self, payload: EventPayload) -> Result<PushoverRequest, PushoverError> {
        // event_action is optional and defaults to "send"; any
        // other value is an explicit caller error so operators
        // notice quickly when they've mistyped a rule.
        match payload.event_action.as_deref() {
            None | Some("send") => {}
            Some(other) => {
                return Err(PushoverError::InvalidPayload(format!(
                    "invalid event_action '{other}': Pushover only supports 'send' (or no event_action)"
                )));
            }
        }

        let message = payload
            .message
            .ok_or_else(|| PushoverError::InvalidPayload("missing 'message' field".into()))?;
        let message = Self::truncate_utf8(message, MESSAGE_MAX_BYTES);

        let user = self
            .config
            .resolve_user_key(payload.user_key.as_deref())?
            .to_owned();

        let priority = if let Some(raw) = payload.priority {
            Some(
                PushoverPriority::from_i32(raw)
                    .map_err(PushoverError::InvalidPayload)?
                    .as_i32(),
            )
        } else {
            None
        };

        // Emergency priority (2) requires retry + expire. Reject at
        // build time so we never send the API a request that is
        // guaranteed to fail.
        if priority == Some(2) && (payload.retry.is_none() || payload.expire.is_none()) {
            return Err(PushoverError::InvalidPayload(
                "priority=2 (emergency) requires both 'retry' and 'expire' fields".into(),
            ));
        }
        // Validate retry/expire bounds per the Pushover API docs.
        if let Some(retry) = payload.retry
            && retry < 30
        {
            return Err(PushoverError::InvalidPayload(format!(
                "retry must be >= 30 seconds (got {retry})"
            )));
        }
        if let Some(expire) = payload.expire
            && expire > 10_800
        {
            return Err(PushoverError::InvalidPayload(format!(
                "expire must be <= 10800 seconds (got {expire})"
            )));
        }

        // html and monospace are mutually exclusive.
        if payload.html == Some(true) && payload.monospace == Some(true) {
            return Err(PushoverError::InvalidPayload(
                "html and monospace are mutually exclusive".into(),
            ));
        }

        let title = payload
            .title
            .map(|t| Self::truncate_utf8(t, TITLE_MAX_BYTES));
        let url = payload.url.map(|u| Self::truncate_utf8(u, URL_MAX_BYTES));
        let url_title = payload
            .url_title
            .map(|t| Self::truncate_utf8(t, URL_TITLE_MAX_BYTES));

        Ok(PushoverRequest {
            token: self.config.app_token().to_owned(),
            user,
            message,
            title,
            priority,
            retry: payload.retry,
            expire: payload.expire,
            sound: payload.sound,
            url,
            url_title,
            device: payload.device,
            html: payload.html.map(u8::from),
            monospace: payload.monospace.map(u8::from),
            timestamp: payload.timestamp,
            ttl: payload.ttl,
        })
    }

    /// POST a form body to the Pushover Messages API and classify
    /// the response into a [`PushoverError`] or a success.
    async fn send_message(
        &self,
        request: &PushoverRequest,
    ) -> Result<PushoverApiResponse, PushoverError> {
        let url = self.messages_url();
        // NOTE: do not include the form body in debug/error output;
        // it contains both secrets (token and user key).
        debug!("sending message to Pushover");
        let builder = self.client.post(&url).form(request);
        let request = acteon_provider::inject_trace_context(builder);
        let response = request.send().await?;
        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            warn!("Pushover API rate limit hit");
            return Err(PushoverError::RateLimited);
        }
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            let body = response.text().await.unwrap_or_default();
            return Err(PushoverError::Unauthorized(format!(
                "HTTP {status}: {}",
                truncate_error_body(&body)
            )));
        }
        if status.is_server_error() || status == reqwest::StatusCode::REQUEST_TIMEOUT {
            let body = response.text().await.unwrap_or_default();
            warn!(%status, "Pushover transient error — will be retried by gateway");
            return Err(PushoverError::Transient(format!(
                "HTTP {status}: {}",
                truncate_error_body(&body)
            )));
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(PushoverError::Api(format!(
                "HTTP {status}: {}",
                truncate_error_body(&body)
            )));
        }

        // 2xx — parse the body and respect the Pushover-layer
        // `status` field. Most 2xx responses include `status: 1`,
        // but if for some reason the server returns 200 with
        // `status: 0` and an errors array we still surface it as
        // a permanent API error rather than silently passing.
        let api_response: PushoverApiResponse = response
            .json()
            .await
            .map_err(|e| PushoverError::Api(format!("failed to parse Pushover response: {e}")))?;
        if api_response.status != 1 {
            return Err(PushoverError::Api(format!(
                "Pushover returned status={} errors={:?}",
                api_response.status, api_response.errors
            )));
        }
        Ok(api_response)
    }
}

impl Provider for PushoverProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "pushover"
    }

    #[instrument(skip(self, action), fields(action_id = %action.id, provider = "pushover"))]
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        let payload: EventPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| PushoverError::InvalidPayload(format!("failed to parse payload: {e}")))?;
        let request = self.build_request(payload)?;
        let api_response = self.send_message(&request).await?;
        let body = serde_json::json!({
            "status": api_response.status,
            "request": api_response.request,
            "receipt": api_response.receipt,
        });
        Ok(ProviderResponse::success(body))
    }

    #[instrument(skip(self), fields(provider = "pushover"))]
    async fn health_check(&self) -> Result<(), ProviderError> {
        // Pushover doesn't have a public ping endpoint, so we issue
        // a GET against the Messages URL. A 405 Method Not Allowed
        // proves the endpoint is reachable; only a transport
        // failure counts as a hard health-check error.
        let url = self.messages_url();
        debug!("performing Pushover health check");
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ProviderError::Connection(e.to_string()))?;
        debug!(status = %response.status(), "Pushover health check response");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use acteon_core::Action;
    use acteon_provider::{Provider, ProviderError};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::config::PushoverConfig;

    /// Tiny mock HTTP server that accepts one connection, returns
    /// a canned response, and captures the request body for
    /// assertions.
    struct MockPushoverServer {
        listener: tokio::net::TcpListener,
        base_url: String,
    }

    impl MockPushoverServer {
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
            let mut buf = vec![0u8; 16384];
            let _ = stream.read(&mut buf).await.unwrap();
            let response = format!(
                "HTTP/1.1 {status_code} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
        }

        async fn respond_once_capturing(self, status_code: u16, body: &str) -> String {
            let body = body.to_owned();
            let (mut stream, _) = self.listener.accept().await.unwrap();
            let mut buf = vec![0u8; 16384];
            let n = stream.read(&mut buf).await.unwrap();
            let raw = String::from_utf8_lossy(&buf[..n]).to_string();
            let response = format!(
                "HTTP/1.1 {status_code} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
            raw
        }
    }

    fn make_action(payload: serde_json::Value) -> Action {
        Action::new("notifications", "tenant-1", "pushover", "notify", payload)
    }

    #[test]
    fn provider_name() {
        let provider =
            PushoverProvider::new(PushoverConfig::single_recipient("app", "ops", "u-ops"));
        assert_eq!(provider.name(), "pushover");
    }

    #[test]
    fn truncate_utf8_passthrough() {
        assert_eq!(
            PushoverProvider::truncate_utf8("short".into(), 100),
            "short"
        );
    }

    #[test]
    fn truncate_utf8_cuts_on_boundary() {
        // 5 bytes, we only allow 4 — the cut should land between
        // whole ASCII characters.
        assert_eq!(PushoverProvider::truncate_utf8("abcde".into(), 4), "abcd");
    }

    #[test]
    fn truncate_utf8_respects_char_boundaries() {
        // "café" is 5 bytes (`c`, `a`, `f`, 0xC3, 0xA9). A naive
        // truncate to 4 bytes would split the `é` in half; our
        // helper must back up to the last UTF-8 boundary.
        let out = PushoverProvider::truncate_utf8("café".into(), 4);
        // Cut should land between `f` and `é`, so we get "caf".
        assert_eq!(out, "caf");
    }

    #[tokio::test]
    async fn execute_send_success() {
        let server = MockPushoverServer::start().await;
        let config = PushoverConfig::single_recipient("test-app", "ops", "u-ops")
            .with_api_base_url(&server.base_url);
        let provider = PushoverProvider::new(config);
        let action = make_action(serde_json::json!({
            "message": "Deploy complete",
            "title": "CI/CD",
            "priority": 0,
            "url": "https://ci.example.com/build/1234",
            "url_title": "View build",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(200, r#"{"status":1,"request":"req-xyz"}"#)
                .await
        });
        let result = provider.execute(&action).await;
        let request = server_handle.await.unwrap();
        let response = result.expect("execute should succeed");
        assert_eq!(response.status, acteon_core::ResponseStatus::Success);
        assert_eq!(response.body["status"], 1);
        assert_eq!(response.body["request"], "req-xyz");

        // Form-encoded POST against /1/messages.json.
        assert!(request.contains("POST /1/messages.json"));
        assert!(request.contains("content-type: application/x-www-form-urlencoded"));
        // Body carries the form-encoded fields.
        assert!(request.contains("token=test-app"));
        assert!(request.contains("user=u-ops"));
        assert!(request.contains("message=Deploy+complete"));
        assert!(request.contains("title=CI%2FCD"));
        assert!(request.contains("priority=0"));
        assert!(request.contains("url_title=View+build"));
    }

    #[tokio::test]
    async fn execute_send_event_action_default() {
        // event_action omitted — should default to "send".
        let server = MockPushoverServer::start().await;
        let config =
            PushoverConfig::single_recipient("t", "ops", "u").with_api_base_url(&server.base_url);
        let provider = PushoverProvider::new(config);
        let action = make_action(serde_json::json!({ "message": "hi" }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(200, r#"{"status":1,"request":"r"}"#)
                .await;
        });
        let result = provider.execute(&action).await;
        server_handle.await.unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn execute_send_event_action_explicit() {
        let server = MockPushoverServer::start().await;
        let config =
            PushoverConfig::single_recipient("t", "ops", "u").with_api_base_url(&server.base_url);
        let provider = PushoverProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "send",
            "message": "hi",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(200, r#"{"status":1,"request":"r"}"#)
                .await;
        });
        let result = provider.execute(&action).await;
        server_handle.await.unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn execute_invalid_event_action() {
        let config = PushoverConfig::single_recipient("t", "ops", "u")
            .with_api_base_url("http://127.0.0.1:1");
        let provider = PushoverProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "acknowledge",
            "message": "hi",
        }));
        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
    }

    #[tokio::test]
    async fn execute_missing_message() {
        let config = PushoverConfig::single_recipient("t", "ops", "u")
            .with_api_base_url("http://127.0.0.1:1");
        let provider = PushoverProvider::new(config);
        let action = make_action(serde_json::json!({ "title": "oops" }));
        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
    }

    #[tokio::test]
    async fn execute_truncates_long_message() {
        let server = MockPushoverServer::start().await;
        let config =
            PushoverConfig::single_recipient("t", "ops", "u").with_api_base_url(&server.base_url);
        let provider = PushoverProvider::new(config);
        let long = "x".repeat(2000);
        let action = make_action(serde_json::json!({ "message": long }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(200, r#"{"status":1,"request":"r"}"#)
                .await
        });
        let _ = provider.execute(&action).await.unwrap();
        let request = server_handle.await.unwrap();
        // The body should contain exactly MESSAGE_MAX_BYTES x's,
        // not 2000.
        let marker = format!("message={}", "x".repeat(MESSAGE_MAX_BYTES));
        assert!(
            request.contains(&marker),
            "message should be truncated to {MESSAGE_MAX_BYTES} bytes"
        );
    }

    #[tokio::test]
    async fn execute_priority_out_of_range() {
        let config = PushoverConfig::single_recipient("t", "ops", "u")
            .with_api_base_url("http://127.0.0.1:1");
        let provider = PushoverProvider::new(config);
        let action = make_action(serde_json::json!({
            "message": "hi",
            "priority": 5,
        }));
        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
    }

    #[tokio::test]
    async fn execute_emergency_priority_requires_retry_and_expire() {
        let config = PushoverConfig::single_recipient("t", "ops", "u")
            .with_api_base_url("http://127.0.0.1:1");
        let provider = PushoverProvider::new(config);
        let action = make_action(serde_json::json!({
            "message": "pageme",
            "priority": 2,
        }));
        let err = provider.execute(&action).await.unwrap_err();
        let msg = match err {
            ProviderError::Serialization(m) => m,
            other => panic!("expected Serialization, got {other:?}"),
        };
        assert!(msg.contains("retry") && msg.contains("expire"));
    }

    #[tokio::test]
    async fn execute_emergency_priority_with_retry_and_expire() {
        let server = MockPushoverServer::start().await;
        let config =
            PushoverConfig::single_recipient("t", "ops", "u").with_api_base_url(&server.base_url);
        let provider = PushoverProvider::new(config);
        let action = make_action(serde_json::json!({
            "message": "pageme",
            "priority": 2,
            "retry": 60,
            "expire": 3600,
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(
                    200,
                    r#"{"status":1,"request":"r","receipt":"rec-emergency"}"#,
                )
                .await
        });
        let result = provider.execute(&action).await;
        let request = server_handle.await.unwrap();
        let response = result.expect("execute should succeed");
        assert!(request.contains("priority=2"));
        assert!(request.contains("retry=60"));
        assert!(request.contains("expire=3600"));
        // Receipt makes it through to the outcome body.
        assert_eq!(response.body["receipt"], "rec-emergency");
    }

    #[tokio::test]
    async fn execute_retry_below_minimum() {
        let config = PushoverConfig::single_recipient("t", "ops", "u")
            .with_api_base_url("http://127.0.0.1:1");
        let provider = PushoverProvider::new(config);
        let action = make_action(serde_json::json!({
            "message": "pageme",
            "priority": 2,
            "retry": 15,     // < 30
            "expire": 600,
        }));
        let err = provider.execute(&action).await.unwrap_err();
        let msg = match err {
            ProviderError::Serialization(m) => m,
            other => panic!("expected Serialization, got {other:?}"),
        };
        assert!(msg.contains("retry"));
    }

    #[tokio::test]
    async fn execute_expire_above_maximum() {
        let config = PushoverConfig::single_recipient("t", "ops", "u")
            .with_api_base_url("http://127.0.0.1:1");
        let provider = PushoverProvider::new(config);
        let action = make_action(serde_json::json!({
            "message": "pageme",
            "priority": 2,
            "retry": 60,
            "expire": 99_999, // > 10800
        }));
        let err = provider.execute(&action).await.unwrap_err();
        let msg = match err {
            ProviderError::Serialization(m) => m,
            other => panic!("expected Serialization, got {other:?}"),
        };
        assert!(msg.contains("expire"));
    }

    #[tokio::test]
    async fn execute_html_and_monospace_mutually_exclusive() {
        let config = PushoverConfig::single_recipient("t", "ops", "u")
            .with_api_base_url("http://127.0.0.1:1");
        let provider = PushoverProvider::new(config);
        let action = make_action(serde_json::json!({
            "message": "hi",
            "html": true,
            "monospace": true,
        }));
        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
    }

    #[tokio::test]
    async fn execute_with_explicit_user_key() {
        let server = MockPushoverServer::start().await;
        let config = PushoverConfig::new("t")
            .with_recipient("ops", "u-ops")
            .with_recipient("dev", "u-dev")
            .with_default_recipient("ops")
            .with_api_base_url(&server.base_url);
        let provider = PushoverProvider::new(config);
        let action = make_action(serde_json::json!({
            "message": "hi",
            "user_key": "dev",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(200, r#"{"status":1,"request":"r"}"#)
                .await
        });
        let _ = provider.execute(&action).await.unwrap();
        let request = server_handle.await.unwrap();
        assert!(
            request.contains("user=u-dev"),
            "explicit user_key should pick u-dev: {request}"
        );
    }

    #[tokio::test]
    async fn execute_unknown_user_key_is_configuration() {
        let config = PushoverConfig::single_recipient("t", "ops", "u-ops")
            .with_api_base_url("http://127.0.0.1:1");
        let provider = PushoverProvider::new(config);
        let action = make_action(serde_json::json!({
            "message": "hi",
            "user_key": "nope",
        }));
        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Configuration(_)));
    }

    #[tokio::test]
    async fn execute_rate_limited() {
        let server = MockPushoverServer::start().await;
        let config =
            PushoverConfig::single_recipient("t", "ops", "u").with_api_base_url(&server.base_url);
        let provider = PushoverProvider::new(config);
        let action = make_action(serde_json::json!({ "message": "hi" }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(429, r#"{"status":0,"errors":["rate limited"]}"#)
                .await;
        });
        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::RateLimited));
        assert!(err.is_retryable());
    }

    #[tokio::test]
    async fn execute_503_retryable_connection() {
        let server = MockPushoverServer::start().await;
        let config =
            PushoverConfig::single_recipient("t", "ops", "u").with_api_base_url(&server.base_url);
        let provider = PushoverProvider::new(config);
        let action = make_action(serde_json::json!({ "message": "hi" }));
        let server_handle = tokio::spawn(async move {
            server.respond_once(503, r#"{"status":0}"#).await;
        });
        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::Connection(_)));
        assert!(err.is_retryable());
    }

    #[tokio::test]
    async fn execute_401_maps_to_configuration() {
        let server = MockPushoverServer::start().await;
        let config =
            PushoverConfig::single_recipient("t", "ops", "u").with_api_base_url(&server.base_url);
        let provider = PushoverProvider::new(config);
        let action = make_action(serde_json::json!({ "message": "hi" }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(401, r#"{"status":0,"errors":["bad token"]}"#)
                .await;
        });
        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::Configuration(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn execute_400_non_retryable() {
        let server = MockPushoverServer::start().await;
        let config =
            PushoverConfig::single_recipient("t", "ops", "u").with_api_base_url(&server.base_url);
        let provider = PushoverProvider::new(config);
        let action = make_action(serde_json::json!({ "message": "hi" }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(
                    400,
                    r#"{"status":0,"errors":["user identifier is invalid"]}"#,
                )
                .await;
        });
        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::ExecutionFailed(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn execute_200_but_status_zero_is_api_error() {
        // Pushover sometimes returns HTTP 200 with a body that
        // contains `status: 0`. The provider must still classify
        // this as a permanent API error rather than silently
        // succeeding.
        let server = MockPushoverServer::start().await;
        let config =
            PushoverConfig::single_recipient("t", "ops", "u").with_api_base_url(&server.base_url);
        let provider = PushoverProvider::new(config);
        let action = make_action(serde_json::json!({ "message": "hi" }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(200, r#"{"status":0,"request":"r","errors":["nope"]}"#)
                .await;
        });
        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::ExecutionFailed(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn health_check_reachable_endpoint() {
        let server = MockPushoverServer::start().await;
        let config =
            PushoverConfig::single_recipient("t", "ops", "u").with_api_base_url(&server.base_url);
        let provider = PushoverProvider::new(config);
        // A 405 Method Not Allowed still means reachable.
        let server_handle = tokio::spawn(async move { server.respond_once(405, "{}").await });
        let result = provider.health_check().await;
        server_handle.await.unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn health_check_connection_failure() {
        let config = PushoverConfig::single_recipient("t", "ops", "u")
            .with_api_base_url("http://127.0.0.1:1");
        let provider = PushoverProvider::new(config);
        let err = provider.health_check().await.unwrap_err();
        assert!(matches!(err, ProviderError::Connection(_)));
        assert!(err.is_retryable());
    }
}
