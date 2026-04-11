use acteon_core::{Action, ProviderResponse};
use acteon_provider::{Provider, ProviderError, truncate_error_body};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, instrument, warn};

use crate::config::TelegramConfig;
use crate::error::TelegramError;
use crate::types::{TelegramApiResponse, TelegramSendMessageRequest};

/// RFC 3986 path-segment encode set — matches the set used by the
/// `OpsGenie` and `VictorOps` providers for consistency.
const PATH_SEGMENT_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'/')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}')
    .add(b':')
    .add(b';')
    .add(b'&')
    .add(b'=')
    .add(b'+')
    .add(b'$')
    .add(b',');

/// Telegram Bot provider that posts messages via `sendMessage`.
pub struct TelegramProvider {
    config: TelegramConfig,
    client: Client,
}

/// Fields extracted from an action payload.
#[derive(Debug, Deserialize)]
struct EventPayload {
    /// Optional — defaults to `"send"`. Present for symmetry with
    /// the other receivers that have real lifecycles; Telegram has
    /// only one action.
    #[serde(default)]
    event_action: Option<String>,
    #[serde(default)]
    chat: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    parse_mode: Option<String>,
    #[serde(default)]
    disable_notification: Option<bool>,
    #[serde(default)]
    disable_web_page_preview: Option<bool>,
    #[serde(default)]
    protect_content: Option<bool>,
    #[serde(default)]
    reply_to_message_id: Option<i64>,
    #[serde(default)]
    message_thread_id: Option<i64>,
}

impl TelegramProvider {
    /// Create a new Telegram provider with a default HTTP client
    /// (30-second timeout).
    pub fn new(config: TelegramConfig) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");
        Self { config, client }
    }

    /// Create a new provider with a custom HTTP client.
    pub fn with_client(config: TelegramConfig, client: Client) -> Self {
        Self { config, client }
    }

    /// Truncate a string to `max_bytes` UTF-8 bytes without splitting
    /// a multi-byte character. Returns the input unchanged when it
    /// already fits.
    fn truncate_utf8(s: String, max_bytes: usize) -> String {
        if s.len() <= max_bytes {
            return s;
        }
        let mut cut = max_bytes;
        while cut > 0 && !s.is_char_boundary(cut) {
            cut -= 1;
        }
        let mut out = s;
        out.truncate(cut);
        out
    }

    /// Percent-encode a value for safe inclusion in an URL path
    /// segment. Used for the bot token in the request URL — the
    /// token has a mixed alphanumeric + `-` + `:` format so `:`
    /// specifically must be escaped.
    fn percent_encode_path_segment(raw: &str) -> String {
        utf8_percent_encode(raw, PATH_SEGMENT_ENCODE_SET).to_string()
    }

    /// Build the full `sendMessage` URL for this provider's bot.
    /// The bot token is embedded in the URL path and percent-encoded
    /// so any special characters (notably the `:` separator between
    /// the bot id and the secret) cannot collapse the path.
    fn send_message_url(&self) -> String {
        let token_seg = Self::percent_encode_path_segment(self.config.bot_token());
        format!("{}/bot{token_seg}/sendMessage", self.config.api_base_url())
    }

    /// Build a [`TelegramSendMessageRequest`] from the payload,
    /// applying config defaults and client-side validation.
    fn build_request(
        &self,
        payload: EventPayload,
    ) -> Result<TelegramSendMessageRequest, TelegramError> {
        // event_action defaults to "send"; any other value is an
        // explicit caller error.
        match payload.event_action.as_deref() {
            None | Some("send") => {}
            Some(other) => {
                return Err(TelegramError::InvalidPayload(format!(
                    "invalid event_action '{other}': Telegram only supports 'send' (or no event_action)"
                )));
            }
        }

        let text = payload
            .text
            .ok_or_else(|| TelegramError::InvalidPayload("missing 'text' field".into()))?;
        let text = Self::truncate_utf8(text, self.config.text_max_bytes);

        let chat_id = self
            .config
            .resolve_chat_id(payload.chat.as_deref())?
            .to_owned();

        let parse_mode = payload
            .parse_mode
            .or_else(|| self.config.default_parse_mode.clone());

        Ok(TelegramSendMessageRequest {
            chat_id,
            text,
            parse_mode,
            disable_notification: payload.disable_notification,
            disable_web_page_preview: payload.disable_web_page_preview,
            protect_content: payload.protect_content,
            reply_to_message_id: payload.reply_to_message_id,
            message_thread_id: payload.message_thread_id,
        })
    }

    /// POST a JSON body to the `sendMessage` endpoint and classify
    /// the response into a [`TelegramError`] or a success.
    async fn post_send(
        &self,
        request: &TelegramSendMessageRequest,
    ) -> Result<TelegramApiResponse, TelegramError> {
        // NOTE: do not include the URL in debug/error output — it
        // contains the bot token as a path segment.
        debug!("sending message to Telegram");
        let url = self.send_message_url();
        let builder = self.client.post(&url).json(request);
        let req = acteon_provider::inject_trace_context(builder);
        let response = req.send().await?;
        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            warn!("Telegram API rate limit hit");
            return Err(TelegramError::RateLimited);
        }
        // 401 Unauthorized or 404 Not Found on `/bot{token}/...`
        // both mean "the bot token is unrecognized" from the
        // server's perspective. Surface them as Configuration so
        // operators get pointed at the credential.
        if matches!(
            status,
            reqwest::StatusCode::UNAUTHORIZED
                | reqwest::StatusCode::FORBIDDEN
                | reqwest::StatusCode::NOT_FOUND
        ) {
            let body = response.text().await.unwrap_or_default();
            return Err(TelegramError::Unauthorized(format!(
                "HTTP {status}: {}",
                truncate_error_body(&body)
            )));
        }
        if status.is_server_error() || status == reqwest::StatusCode::REQUEST_TIMEOUT {
            let body = response.text().await.unwrap_or_default();
            warn!(%status, "Telegram transient error — will be retried by gateway");
            return Err(TelegramError::Transient(format!(
                "HTTP {status}: {}",
                truncate_error_body(&body)
            )));
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(TelegramError::Api(format!(
                "HTTP {status}: {}",
                truncate_error_body(&body)
            )));
        }

        let api_response: TelegramApiResponse = response
            .json()
            .await
            .map_err(|e| TelegramError::Api(format!("failed to parse Telegram response: {e}")))?;
        if !api_response.ok {
            return Err(TelegramError::Api(format!(
                "Telegram returned ok=false error_code={:?} description={:?}",
                api_response.error_code, api_response.description
            )));
        }
        Ok(api_response)
    }
}

impl Provider for TelegramProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "telegram"
    }

    #[instrument(skip(self, action), fields(action_id = %action.id, provider = "telegram"))]
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        let payload: EventPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| TelegramError::InvalidPayload(format!("failed to parse payload: {e}")))?;
        let request = self.build_request(payload)?;
        let api_response = self.post_send(&request).await?;

        // Extract message_id from the result object if present;
        // surface it in the outcome body so chains and audit can
        // reference specific messages.
        let message_id = api_response
            .result
            .as_ref()
            .and_then(|v| v.get("message_id"))
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        let body = serde_json::json!({
            "ok": api_response.ok,
            "message_id": message_id,
        });
        Ok(ProviderResponse::success(body))
    }

    #[instrument(skip(self), fields(provider = "telegram"))]
    async fn health_check(&self) -> Result<(), ProviderError> {
        // Telegram exposes a `getMe` endpoint that returns the
        // bot's identity on any valid token. We only care that the
        // request round-trips at the HTTP layer; a 401/404 still
        // means the endpoint is reachable, and only a transport
        // failure is a hard health-check error.
        let token_seg = Self::percent_encode_path_segment(self.config.bot_token());
        let url = format!("{}/bot{token_seg}/getMe", self.config.api_base_url());
        debug!("performing Telegram health check");
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ProviderError::Connection(e.to_string()))?;
        debug!(status = %response.status(), "Telegram health check response");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use acteon_core::Action;
    use acteon_provider::{Provider, ProviderError};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::config::{DEFAULT_TEXT_MAX_BYTES, TelegramConfig};

    /// Tiny mock HTTP server for integration-style tests. Same
    /// pattern as the other integration crates — accept one
    /// connection, return a canned response, capture the request
    /// body for assertions.
    struct MockTelegramServer {
        listener: tokio::net::TcpListener,
        base_url: String,
    }

    impl MockTelegramServer {
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
        Action::new("notifications", "tenant-1", "telegram", "notify", payload)
    }

    #[test]
    fn provider_name() {
        let provider = TelegramProvider::new(TelegramConfig::single_chat("tok", "ops", "-1001234"));
        assert_eq!(provider.name(), "telegram");
    }

    #[test]
    fn truncate_utf8_passthrough() {
        assert_eq!(
            TelegramProvider::truncate_utf8("short".into(), 100),
            "short"
        );
    }

    #[test]
    fn truncate_utf8_ascii_cut() {
        assert_eq!(TelegramProvider::truncate_utf8("abcde".into(), 4), "abcd");
    }

    #[test]
    fn truncate_utf8_respects_char_boundaries() {
        // "café" = c, a, f, 0xC3, 0xA9 — naive 4-byte cut would
        // split the é. The helper must back up to the boundary.
        let out = TelegramProvider::truncate_utf8("café".into(), 4);
        assert_eq!(out, "caf");
    }

    #[test]
    fn percent_encode_bot_token_segment() {
        // Bot token format: "{bot_id}:{auth}" — the colon must
        // be percent-encoded so it does not collapse the URL
        // path.
        assert_eq!(
            TelegramProvider::percent_encode_path_segment("123456:ABC-DEF"),
            "123456%3AABC-DEF"
        );
    }

    #[test]
    fn percent_encode_utf8() {
        assert_eq!(
            TelegramProvider::percent_encode_path_segment("café"),
            "caf%C3%A9"
        );
    }

    #[tokio::test]
    async fn execute_send_success() {
        let server = MockTelegramServer::start().await;
        let config = TelegramConfig::single_chat("123:TOK", "ops", "-1001234")
            .with_api_base_url(&server.base_url);
        let provider = TelegramProvider::new(config);
        let action = make_action(serde_json::json!({
            "text": "Deploy complete",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(
                    200,
                    r#"{"ok":true,"result":{"message_id":42,"text":"Deploy complete"}}"#,
                )
                .await
        });
        let result = provider.execute(&action).await;
        let request = server_handle.await.unwrap();
        let response = result.expect("execute should succeed");
        assert_eq!(response.status, acteon_core::ResponseStatus::Success);
        assert_eq!(response.body["ok"], true);
        assert_eq!(response.body["message_id"], 42);

        // URL embeds the percent-encoded bot token.
        assert!(
            request.contains("POST /bot123%3ATOK/sendMessage"),
            "URL should embed percent-encoded bot token: {request}"
        );
        assert!(request.contains("content-type: application/json"));
        assert!(request.contains("\"chat_id\":\"-1001234\""));
        assert!(request.contains("\"text\":\"Deploy complete\""));
    }

    #[tokio::test]
    async fn execute_send_with_full_options() {
        let server = MockTelegramServer::start().await;
        let config = TelegramConfig::single_chat("t", "ops", "-1")
            .with_api_base_url(&server.base_url)
            .with_default_parse_mode("HTML");
        let provider = TelegramProvider::new(config);
        let action = make_action(serde_json::json!({
            "text": "<b>HIGH CPU</b>",
            "disable_notification": false,
            "disable_web_page_preview": true,
            "protect_content": true,
            "reply_to_message_id": 42,
            "message_thread_id": 7,
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(200, r#"{"ok":true,"result":{"message_id":100}}"#)
                .await
        });
        let _ = provider.execute(&action).await.unwrap();
        let request = server_handle.await.unwrap();

        assert!(request.contains("\"parse_mode\":\"HTML\""));
        assert!(request.contains("\"disable_web_page_preview\":true"));
        assert!(request.contains("\"protect_content\":true"));
        assert!(request.contains("\"reply_to_message_id\":42"));
        assert!(request.contains("\"message_thread_id\":7"));
    }

    #[tokio::test]
    async fn execute_payload_parse_mode_overrides_config_default() {
        let server = MockTelegramServer::start().await;
        let config = TelegramConfig::single_chat("t", "ops", "-1")
            .with_api_base_url(&server.base_url)
            .with_default_parse_mode("HTML");
        let provider = TelegramProvider::new(config);
        let action = make_action(serde_json::json!({
            "text": "*bold*",
            "parse_mode": "MarkdownV2",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(200, r#"{"ok":true,"result":{"message_id":1}}"#)
                .await
        });
        let _ = provider.execute(&action).await.unwrap();
        let request = server_handle.await.unwrap();
        // Payload override wins.
        assert!(request.contains("\"parse_mode\":\"MarkdownV2\""));
        assert!(!request.contains("\"parse_mode\":\"HTML\""));
    }

    #[tokio::test]
    async fn execute_send_event_action_default_and_explicit() {
        // event_action omitted.
        let server = MockTelegramServer::start().await;
        let config =
            TelegramConfig::single_chat("t", "ops", "-1").with_api_base_url(&server.base_url);
        let provider = TelegramProvider::new(config);
        let action = make_action(serde_json::json!({ "text": "hi" }));
        let server_handle = tokio::spawn(async move {
            server.respond_once(200, r#"{"ok":true,"result":{}}"#).await;
        });
        let r = provider.execute(&action).await;
        server_handle.await.unwrap();
        assert!(r.is_ok());

        // event_action = "send" explicitly.
        let server = MockTelegramServer::start().await;
        let config =
            TelegramConfig::single_chat("t", "ops", "-1").with_api_base_url(&server.base_url);
        let provider = TelegramProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "send",
            "text": "hi",
        }));
        let server_handle = tokio::spawn(async move {
            server.respond_once(200, r#"{"ok":true,"result":{}}"#).await;
        });
        let r = provider.execute(&action).await;
        server_handle.await.unwrap();
        assert!(r.is_ok());
    }

    #[tokio::test]
    async fn execute_invalid_event_action() {
        let config =
            TelegramConfig::single_chat("t", "ops", "-1").with_api_base_url("http://127.0.0.1:1");
        let provider = TelegramProvider::new(config);
        let action = make_action(serde_json::json!({
            "event_action": "acknowledge",
            "text": "hi",
        }));
        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
    }

    #[tokio::test]
    async fn execute_missing_text() {
        let config =
            TelegramConfig::single_chat("t", "ops", "-1").with_api_base_url("http://127.0.0.1:1");
        let provider = TelegramProvider::new(config);
        let action = make_action(serde_json::json!({ "chat": "ops" }));
        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
    }

    #[tokio::test]
    async fn execute_with_explicit_chat() {
        let server = MockTelegramServer::start().await;
        let config = TelegramConfig::new("t")
            .with_chat("ops", "-1001")
            .with_chat("dev", "@dev")
            .with_default_chat("ops")
            .with_api_base_url(&server.base_url);
        let provider = TelegramProvider::new(config);
        let action = make_action(serde_json::json!({
            "text": "hi",
            "chat": "dev",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(200, r#"{"ok":true,"result":{}}"#)
                .await
        });
        let _ = provider.execute(&action).await.unwrap();
        let request = server_handle.await.unwrap();
        assert!(
            request.contains("\"chat_id\":\"@dev\""),
            "explicit chat should pick @dev: {request}"
        );
    }

    #[tokio::test]
    async fn execute_unknown_chat_is_configuration() {
        let config =
            TelegramConfig::single_chat("t", "ops", "-1").with_api_base_url("http://127.0.0.1:1");
        let provider = TelegramProvider::new(config);
        let action = make_action(serde_json::json!({
            "text": "hi",
            "chat": "nope",
        }));
        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Configuration(_)));
    }

    #[tokio::test]
    async fn execute_truncates_long_text() {
        let server = MockTelegramServer::start().await;
        let config =
            TelegramConfig::single_chat("t", "ops", "-1").with_api_base_url(&server.base_url);
        let provider = TelegramProvider::new(config);
        let long = "x".repeat(DEFAULT_TEXT_MAX_BYTES + 1000);
        let action = make_action(serde_json::json!({ "text": long }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(200, r#"{"ok":true,"result":{}}"#)
                .await
        });
        let _ = provider.execute(&action).await.unwrap();
        let request = server_handle.await.unwrap();
        let marker = format!("\"text\":\"{}\"", "x".repeat(DEFAULT_TEXT_MAX_BYTES));
        assert!(
            request.contains(&marker),
            "text should be truncated to {DEFAULT_TEXT_MAX_BYTES} bytes"
        );
    }

    #[tokio::test]
    async fn execute_respects_configured_text_max_bytes() {
        let server = MockTelegramServer::start().await;
        let config = TelegramConfig::single_chat("t", "ops", "-1")
            .with_api_base_url(&server.base_url)
            .with_text_max_bytes(100);
        let provider = TelegramProvider::new(config);
        let long = "y".repeat(200);
        let action = make_action(serde_json::json!({ "text": long }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(200, r#"{"ok":true,"result":{}}"#)
                .await
        });
        let _ = provider.execute(&action).await.unwrap();
        let request = server_handle.await.unwrap();
        let marker = format!("\"text\":\"{}\"", "y".repeat(100));
        assert!(request.contains(&marker));
    }

    #[tokio::test]
    async fn execute_rate_limited() {
        let server = MockTelegramServer::start().await;
        let config =
            TelegramConfig::single_chat("t", "ops", "-1").with_api_base_url(&server.base_url);
        let provider = TelegramProvider::new(config);
        let action = make_action(serde_json::json!({ "text": "hi" }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(
                    429,
                    r#"{"ok":false,"error_code":429,"description":"retry after 5","parameters":{"retry_after":5}}"#,
                )
                .await;
        });
        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::RateLimited));
        assert!(err.is_retryable());
    }

    #[tokio::test]
    async fn execute_503_retryable_connection() {
        let server = MockTelegramServer::start().await;
        let config =
            TelegramConfig::single_chat("t", "ops", "-1").with_api_base_url(&server.base_url);
        let provider = TelegramProvider::new(config);
        let action = make_action(serde_json::json!({ "text": "hi" }));
        let server_handle = tokio::spawn(async move {
            server.respond_once(503, r#"{"ok":false}"#).await;
        });
        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::Connection(_)));
        assert!(err.is_retryable());
    }

    #[tokio::test]
    async fn execute_401_maps_to_configuration() {
        let server = MockTelegramServer::start().await;
        let config =
            TelegramConfig::single_chat("t", "ops", "-1").with_api_base_url(&server.base_url);
        let provider = TelegramProvider::new(config);
        let action = make_action(serde_json::json!({ "text": "hi" }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(
                    401,
                    r#"{"ok":false,"error_code":401,"description":"Unauthorized"}"#,
                )
                .await;
        });
        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::Configuration(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn execute_404_maps_to_configuration() {
        // 404 on /bot{token}/... means the token is unrecognized.
        let server = MockTelegramServer::start().await;
        let config =
            TelegramConfig::single_chat("t", "ops", "-1").with_api_base_url(&server.base_url);
        let provider = TelegramProvider::new(config);
        let action = make_action(serde_json::json!({ "text": "hi" }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(
                    404,
                    r#"{"ok":false,"error_code":404,"description":"Not Found"}"#,
                )
                .await;
        });
        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();
        assert!(
            matches!(err, ProviderError::Configuration(_)),
            "404 should surface as Configuration (bad bot token), got {err:?}"
        );
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn execute_400_non_retryable() {
        let server = MockTelegramServer::start().await;
        let config =
            TelegramConfig::single_chat("t", "ops", "-1").with_api_base_url(&server.base_url);
        let provider = TelegramProvider::new(config);
        let action = make_action(serde_json::json!({ "text": "hi" }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(
                    400,
                    r#"{"ok":false,"error_code":400,"description":"Bad Request: chat not found"}"#,
                )
                .await;
        });
        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::ExecutionFailed(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn execute_200_but_ok_false_is_api_error() {
        // Telegram sometimes returns 200 with ok=false (rare but
        // possible) — classify as permanent API error.
        let server = MockTelegramServer::start().await;
        let config =
            TelegramConfig::single_chat("t", "ops", "-1").with_api_base_url(&server.base_url);
        let provider = TelegramProvider::new(config);
        let action = make_action(serde_json::json!({ "text": "hi" }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(200, r#"{"ok":false,"error_code":400,"description":"nope"}"#)
                .await;
        });
        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::ExecutionFailed(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn health_check_reachable() {
        let server = MockTelegramServer::start().await;
        let config =
            TelegramConfig::single_chat("t", "ops", "-1").with_api_base_url(&server.base_url);
        let provider = TelegramProvider::new(config);
        // Even a 401 still means reachable.
        let server_handle =
            tokio::spawn(async move { server.respond_once(401, r#"{"ok":false}"#).await });
        let result = provider.health_check().await;
        server_handle.await.unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn health_check_connection_failure() {
        let config =
            TelegramConfig::single_chat("t", "ops", "-1").with_api_base_url("http://127.0.0.1:1");
        let provider = TelegramProvider::new(config);
        let err = provider.health_check().await.unwrap_err();
        assert!(matches!(err, ProviderError::Connection(_)));
        assert!(err.is_retryable());
    }
}
