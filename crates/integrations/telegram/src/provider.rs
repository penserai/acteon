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

    /// Truncate a string so it serializes to at most `max_units`
    /// **UTF-16 code units** — the units Telegram's API uses for
    /// its 4096 cap on the `text` field.
    ///
    /// Counting UTF-16 units matches the API exactly: one BMP
    /// character costs 1 unit, one non-BMP character (most emoji,
    /// some CJK supplementary ideographs) costs 2. This is less
    /// conservative than byte-based truncation for CJK and
    /// emoji-heavy traffic — a 4096-character CJK message can
    /// legitimately be ~12 KB of UTF-8, and a byte cap would
    /// truncate it at ~1365 characters even though the API
    /// would accept the full message.
    ///
    /// Returns the input unchanged when it already fits. The
    /// algorithm walks chars in order and stops as soon as
    /// adding the next char would exceed the cap — no
    /// intermediate allocation.
    fn truncate_to_utf16_units(s: String, max_units: usize) -> String {
        let mut units = 0usize;
        let mut byte_end = 0usize;
        for (i, ch) in s.char_indices() {
            let ch_units = ch.len_utf16();
            if units + ch_units > max_units {
                let mut out = s;
                out.truncate(byte_end);
                return out;
            }
            units += ch_units;
            byte_end = i + ch.len_utf8();
        }
        s
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
        let text = Self::truncate_to_utf16_units(text, self.config.text_max_utf16_units);

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
            // Telegram's 429 body includes `parameters.retry_after`
            // with the server-suggested wait in seconds. We still
            // return `RateLimited` (not a typed backoff) because
            // `ProviderError::RateLimited` does not currently carry
            // a duration, but surfacing the hint in the log lets
            // operators see how long Telegram wants the bot to
            // back off without digging into request traces.
            let body = response.text().await.unwrap_or_default();
            let retry_after = serde_json::from_str::<TelegramApiResponse>(&body)
                .ok()
                .and_then(|r| r.parameters)
                .and_then(|p| p.retry_after);
            if let Some(seconds) = retry_after {
                warn!(retry_after_seconds = seconds, "Telegram API rate limit hit");
            } else {
                warn!("Telegram API rate limit hit");
            }
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
        // Telegram's `getMe` endpoint is explicitly designed as a
        // credential-validity check: it's a free, idempotent GET
        // that returns the bot's identity on a valid token and
        // rejects bad tokens with a 401 / 404. That lets this
        // provider verify **both** connectivity *and* the bot
        // token on every health check — a stronger guarantee than
        // the other receiver crates in the repo, whose APIs lack
        // a comparable no-op endpoint.
        //
        // Failure classification:
        // - Transport error → `Connection` (retryable)
        // - 401 / 403 / 404 → `Configuration` (bad token)
        // - Other non-2xx → `Connection` (transient)
        // - 2xx but `ok: false` → `Configuration`
        // - 2xx with `ok: true` → health OK
        let token_seg = Self::percent_encode_path_segment(self.config.bot_token());
        let url = format!("{}/bot{token_seg}/getMe", self.config.api_base_url());
        debug!("performing Telegram health check");
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ProviderError::Connection(e.to_string()))?;
        let status = response.status();
        debug!(%status, "Telegram health check response");

        if matches!(
            status,
            reqwest::StatusCode::UNAUTHORIZED
                | reqwest::StatusCode::FORBIDDEN
                | reqwest::StatusCode::NOT_FOUND
        ) {
            return Err(ProviderError::Configuration(format!(
                "Telegram getMe rejected the bot token: HTTP {status}"
            )));
        }
        if !status.is_success() {
            // Transient non-2xx — surface as retryable Connection
            // so the gateway's circuit breaker treats it as a
            // reachability blip rather than a credential problem.
            return Err(ProviderError::Connection(format!(
                "Telegram getMe returned non-success: HTTP {status}"
            )));
        }

        let body: TelegramApiResponse = response.json().await.map_err(|e| {
            ProviderError::Connection(format!("failed to parse Telegram getMe response: {e}"))
        })?;
        if !body.ok {
            return Err(ProviderError::Configuration(format!(
                "Telegram getMe returned ok=false: {}",
                body.description.as_deref().unwrap_or("(no description)")
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use acteon_core::Action;
    use acteon_provider::{Provider, ProviderError};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::config::{DEFAULT_TEXT_MAX_UTF16_UNITS, TelegramConfig};

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
    fn truncate_to_utf16_units_passthrough() {
        assert_eq!(
            TelegramProvider::truncate_to_utf16_units("short".into(), 100),
            "short"
        );
    }

    #[test]
    fn truncate_to_utf16_units_ascii_cut() {
        assert_eq!(
            TelegramProvider::truncate_to_utf16_units("abcde".into(), 4),
            "abcd"
        );
    }

    #[test]
    fn truncate_to_utf16_units_char_boundaries() {
        // "café" → c, a, f, é. `é` is a single BMP char (1
        // UTF-16 unit, 2 UTF-8 bytes). A 4-unit cap fits the
        // whole string; a 3-unit cap stops at "caf".
        assert_eq!(
            TelegramProvider::truncate_to_utf16_units("café".into(), 4),
            "café"
        );
        assert_eq!(
            TelegramProvider::truncate_to_utf16_units("café".into(), 3),
            "caf"
        );
    }

    #[test]
    fn truncate_to_utf16_units_cjk_is_less_conservative_than_bytes() {
        // Four CJK characters. Each is 3 UTF-8 bytes but 1
        // UTF-16 unit (all in the BMP), so a 4-unit cap fits
        // the whole string — whereas a byte-based 4-cap would
        // fit only 1 character and 1 byte of the next (which we
        // would then have to back off to a char boundary). This
        // is the whole point of the refactor: CJK traffic was
        // being truncated at ~1365 characters under the old
        // byte-based 4096 cap even though the API accepts 4096
        // characters.
        let cjk = "字符串测".to_owned();
        assert_eq!(cjk.len(), 12, "four CJK chars = 12 UTF-8 bytes");
        assert_eq!(cjk.encode_utf16().count(), 4, "but 4 UTF-16 units");
        let truncated = TelegramProvider::truncate_to_utf16_units(cjk.clone(), 4);
        assert_eq!(truncated, cjk, "4-unit cap fits all 4 CJK chars");
    }

    #[test]
    fn truncate_to_utf16_units_handles_surrogate_pairs() {
        // Non-BMP emoji (U+1F680 ROCKET) costs 2 UTF-16 code
        // units (one surrogate pair). A 2-unit cap fits one
        // rocket; a 1-unit cap fits nothing because we cannot
        // split a surrogate pair.
        let two_rockets = "🚀🚀".to_owned();
        assert_eq!(
            two_rockets.encode_utf16().count(),
            4,
            "two rockets = 4 UTF-16 units"
        );
        assert_eq!(
            TelegramProvider::truncate_to_utf16_units(two_rockets.clone(), 2),
            "🚀"
        );
        assert_eq!(
            TelegramProvider::truncate_to_utf16_units(two_rockets.clone(), 1),
            "",
            "a 1-unit cap cannot fit a surrogate pair"
        );
        assert_eq!(
            TelegramProvider::truncate_to_utf16_units(two_rockets.clone(), 4),
            two_rockets
        );
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
        // ASCII `x` costs 1 UTF-16 unit per char, so sending
        // DEFAULT_TEXT_MAX_UTF16_UNITS + 1000 characters triggers
        // truncation down to the cap.
        let long = "x".repeat(DEFAULT_TEXT_MAX_UTF16_UNITS + 1000);
        let action = make_action(serde_json::json!({ "text": long }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(200, r#"{"ok":true,"result":{}}"#)
                .await
        });
        let _ = provider.execute(&action).await.unwrap();
        let request = server_handle.await.unwrap();
        let marker = format!("\"text\":\"{}\"", "x".repeat(DEFAULT_TEXT_MAX_UTF16_UNITS));
        assert!(
            request.contains(&marker),
            "text should be truncated to {DEFAULT_TEXT_MAX_UTF16_UNITS} UTF-16 code units"
        );
    }

    #[tokio::test]
    async fn execute_respects_configured_text_max_utf16_units() {
        let server = MockTelegramServer::start().await;
        let config = TelegramConfig::single_chat("t", "ops", "-1")
            .with_api_base_url(&server.base_url)
            .with_text_max_utf16_units(100);
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
    async fn execute_allows_full_utf16_runway_for_cjk() {
        // 4096 CJK characters is 12288 UTF-8 bytes — the old
        // byte-based cap would have truncated at ~1365 characters.
        // The UTF-16-unit cap lets us send the full 4096 chars
        // because each CJK character is a single UTF-16 unit.
        let server = MockTelegramServer::start().await;
        let config =
            TelegramConfig::single_chat("t", "ops", "-1").with_api_base_url(&server.base_url);
        let provider = TelegramProvider::new(config);
        let cjk_4096 = "字".repeat(DEFAULT_TEXT_MAX_UTF16_UNITS);
        let action = make_action(serde_json::json!({ "text": cjk_4096.clone() }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once_capturing(200, r#"{"ok":true,"result":{}}"#)
                .await
        });
        let _ = provider.execute(&action).await.unwrap();
        let request = server_handle.await.unwrap();
        // The full 4096 characters (12288 bytes) should appear in
        // the request body — proof that CJK traffic no longer hits
        // the conservative byte cap.
        assert!(
            request.contains(&cjk_4096),
            "full 4096-char CJK text should pass through unchanged"
        );
    }

    #[tokio::test]
    async fn execute_rate_limited_parses_retry_after_body() {
        // Telegram's 429 body carries `parameters.retry_after`
        // with the server-suggested backoff in seconds. The
        // provider logs it but still returns the unparameterized
        // `RateLimited` because `ProviderError` does not carry a
        // backoff duration. This test exercises the body-parsing
        // path — the retry_after value lands in the warn log
        // (not observable here without a tracing subscriber
        // capture), but the test does verify the error
        // classification is correct and that the body parse does
        // not break on the `parameters` sub-object.
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
    async fn execute_rate_limited_tolerates_missing_retry_after() {
        // If the 429 body has no `parameters.retry_after` (or is
        // completely unparseable), the provider still classifies
        // it as RateLimited — the parse is best-effort for the
        // log line, never a hard requirement.
        let server = MockTelegramServer::start().await;
        let config =
            TelegramConfig::single_chat("t", "ops", "-1").with_api_base_url(&server.base_url);
        let provider = TelegramProvider::new(config);
        let action = make_action(serde_json::json!({ "text": "hi" }));
        let server_handle = tokio::spawn(async move {
            // Empty body — parse returns None for retry_after
            // and the code falls through to the "hit, no hint"
            // log branch.
            server.respond_once(429, "").await;
        });
        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::RateLimited));
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
    async fn health_check_ok_true_succeeds() {
        // Happy path: getMe returns 200 with `ok: true` and a
        // non-empty bot identity result.
        let server = MockTelegramServer::start().await;
        let config =
            TelegramConfig::single_chat("t", "ops", "-1").with_api_base_url(&server.base_url);
        let provider = TelegramProvider::new(config);
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(
                    200,
                    r#"{"ok":true,"result":{"id":12345,"is_bot":true,"first_name":"acteon-bot","username":"acteon_bot"}}"#,
                )
                .await;
        });
        let result = provider.health_check().await;
        server_handle.await.unwrap();
        assert!(result.is_ok(), "getMe ok=true should succeed: {result:?}");
    }

    #[tokio::test]
    async fn health_check_401_is_configuration() {
        // Bad bot token: Telegram returns 401. This is the whole
        // point of verifying credentials in the health check — the
        // gateway's health dashboard should see it as
        // Configuration rather than a transient connection blip.
        let server = MockTelegramServer::start().await;
        let config =
            TelegramConfig::single_chat("t", "ops", "-1").with_api_base_url(&server.base_url);
        let provider = TelegramProvider::new(config);
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(
                    401,
                    r#"{"ok":false,"error_code":401,"description":"Unauthorized"}"#,
                )
                .await;
        });
        let err = provider.health_check().await.unwrap_err();
        server_handle.await.unwrap();
        assert!(
            matches!(err, ProviderError::Configuration(_)),
            "401 should be Configuration, got {err:?}"
        );
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn health_check_404_is_configuration() {
        // 404 on /bot{token}/getMe — unrecognized token.
        let server = MockTelegramServer::start().await;
        let config =
            TelegramConfig::single_chat("t", "ops", "-1").with_api_base_url(&server.base_url);
        let provider = TelegramProvider::new(config);
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(
                    404,
                    r#"{"ok":false,"error_code":404,"description":"Not Found"}"#,
                )
                .await;
        });
        let err = provider.health_check().await.unwrap_err();
        server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::Configuration(_)));
    }

    #[tokio::test]
    async fn health_check_200_ok_false_is_configuration() {
        // 200 envelope with `ok: false` (rare but possible) still
        // indicates a credential/config problem, not a transient
        // one.
        let server = MockTelegramServer::start().await;
        let config =
            TelegramConfig::single_chat("t", "ops", "-1").with_api_base_url(&server.base_url);
        let provider = TelegramProvider::new(config);
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(
                    200,
                    r#"{"ok":false,"error_code":401,"description":"bot token revoked"}"#,
                )
                .await;
        });
        let err = provider.health_check().await.unwrap_err();
        server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::Configuration(_)));
    }

    #[tokio::test]
    async fn health_check_500_is_connection() {
        // Transient non-2xx: the endpoint is reachable but the
        // server is unhappy. Classify as Connection so the
        // circuit breaker treats it as a reachability blip.
        let server = MockTelegramServer::start().await;
        let config =
            TelegramConfig::single_chat("t", "ops", "-1").with_api_base_url(&server.base_url);
        let provider = TelegramProvider::new(config);
        let server_handle = tokio::spawn(async move {
            server.respond_once(500, r#"{"ok":false}"#).await;
        });
        let err = provider.health_check().await.unwrap_err();
        server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::Connection(_)));
        assert!(err.is_retryable());
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
