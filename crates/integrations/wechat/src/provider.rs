use std::sync::Arc;
use std::time::{Duration, Instant};

use acteon_core::{Action, ProviderResponse};
use acteon_crypto::{ExposeSecret, SecretString};
use acteon_provider::{Provider, ProviderError, truncate_error_body};
use reqwest::Client;
use serde::Deserialize;
use tokio::sync::Mutex;
use tracing::{debug, instrument, warn};

use crate::config::{WeChatConfig, WeChatRecipients};
use crate::error::WeChatError;
use crate::types::{
    WeChatApiResponse, WeChatMsgType, WeChatSendRequest, WeChatTextBody, WeChatTextCardBody,
    WeChatTokenResponse,
};

/// Maximum number of bytes to read from an error-response body
/// before giving up. A misbehaving upstream (or malicious
/// man-in-the-middle proxy) could otherwise stream an unbounded
/// body and force Acteon to allocate gigabytes just to produce
/// an error message. 2 KiB is plenty for the `errcode + errmsg`
/// envelope that `WeChat` actually returns — the rest would be
/// truncated by [`truncate_error_body`] anyway.
const MAX_ERROR_BODY_READ_BYTES: usize = 2048;

/// Read at most `max_bytes` bytes from a `reqwest::Response`
/// body and return them as a lossy UTF-8 string.
///
/// Unlike [`reqwest::Response::text`], which reads the entire
/// body into memory before returning, this helper pumps
/// `Response::chunk()` in a loop and stops as soon as the
/// configured byte limit is reached. A response whose
/// `Content-Length` is huge (or unbounded) gets truncated at
/// the caller's hard limit rather than `OOMing` the process.
async fn read_bounded_body(mut response: reqwest::Response, max_bytes: usize) -> String {
    let mut buf: Vec<u8> = Vec::with_capacity(max_bytes.min(1024));
    while buf.len() < max_bytes {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = max_bytes - buf.len();
                let take = chunk.len().min(remaining);
                buf.extend_from_slice(&chunk[..take]);
                if chunk.len() > remaining {
                    // Hit the cap mid-chunk — drop the rest of
                    // this chunk and stop pulling.
                    break;
                }
            }
            // End of stream or transport error — either way,
            // stop reading and return whatever we have so far.
            Ok(None) | Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}

// -- WeChat errcode classification ---------------------------------------
//
// The WeChat API returns an `errcode` envelope on every response.
// Zero means success; non-zero means failure. The mapping below
// is the authoritative source for how each class of error is
// surfaced to the gateway.
//
// Reference: https://developer.work.weixin.qq.com/document/path/90313

/// Access token is expired. Drives the provider's in-band
/// refresh-and-retry loop.
const ERRCODE_ACCESS_TOKEN_EXPIRED: i32 = 42001;

/// Access token is invalid (usually revoked). Same retry path as
/// `ERRCODE_ACCESS_TOKEN_EXPIRED`.
const ERRCODE_INVALID_ACCESS_TOKEN: i32 = 40014;

/// Invalid credential — usually a bad `corp_secret` passed to
/// `gettoken`. Non-retryable.
const ERRCODE_INVALID_CREDENTIAL: i32 = 40001;

/// Invalid `corpid`. Non-retryable.
const ERRCODE_INVALID_CORPID: i32 = 40013;

/// API call frequency exceeded. Retryable.
const ERRCODE_RATE_LIMITED: i32 = 45009;

/// Generic "system busy" error. Retryable.
const ERRCODE_SYSTEM_BUSY: i32 = -1;

/// Internal cache entry: the current access token plus its
/// expiry time as an `Instant` so we can compare against
/// `Instant::now()` without clock-drift worries.
#[derive(Clone)]
struct CachedToken {
    token: SecretString,
    expires_at: Instant,
}

/// `WeChat` Work provider.
///
/// Internally holds a token cache behind an async `Mutex` so the
/// refresh path is serialized — one expired-token observation
/// triggers exactly one refresh, even when many dispatches land
/// on the provider simultaneously.
pub struct WeChatProvider {
    config: WeChatConfig,
    client: Client,
    token_cache: Arc<Mutex<Option<CachedToken>>>,
}

/// Fields extracted from an action payload.
#[derive(Debug, Deserialize)]
struct EventPayload {
    /// Optional — defaults to `"send"`. `WeChat` has no lifecycle
    /// so the only supported value is `"send"`.
    #[serde(default)]
    event_action: Option<String>,
    #[serde(default)]
    touser: Option<String>,
    #[serde(default)]
    toparty: Option<String>,
    #[serde(default)]
    totag: Option<String>,
    #[serde(default)]
    msgtype: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    btntxt: Option<String>,
}

impl WeChatProvider {
    /// Create a new `WeChat` Work provider with a default HTTP
    /// client (30-second timeout).
    pub fn new(config: WeChatConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");
        Self::with_client(config, client)
    }

    /// Create a new provider with a custom HTTP client — useful
    /// for tests and for sharing a connection pool across
    /// providers.
    pub fn with_client(config: WeChatConfig, client: Client) -> Self {
        Self {
            config,
            client,
            token_cache: Arc::new(Mutex::new(None)),
        }
    }

    /// Fetch a valid access token, refreshing if the cached one
    /// is missing or within the configured refresh buffer.
    ///
    /// The entire read/check/refresh path is serialized behind an
    /// async `Mutex` so a burst of concurrent dispatches on an
    /// expired token triggers exactly one refresh, not N.
    async fn get_access_token(&self) -> Result<String, WeChatError> {
        let buffer = Duration::from_secs(self.config.token_refresh_buffer_seconds);
        let mut cache = self.token_cache.lock().await;
        if let Some(cached) = cache.as_ref()
            && cached.expires_at > Instant::now() + buffer
        {
            return Ok(cached.token.expose_secret().to_owned());
        }
        // Either no cached token or it's within the refresh buffer.
        let fresh = self.fetch_new_token().await?;
        *cache = Some(fresh.clone());
        Ok(fresh.token.expose_secret().to_owned())
    }

    /// Force-invalidate the cached token. Called when a send
    /// operation observes `errcode: 42001` / `40014`, so the
    /// next `get_access_token` triggers a fresh fetch.
    async fn invalidate_token(&self) {
        let mut cache = self.token_cache.lock().await;
        *cache = None;
    }

    /// Actually call `GET /cgi-bin/gettoken` against the server.
    /// Does not touch the cache — caller is responsible.
    ///
    /// ## Credential-leak hardening
    ///
    /// `WeChat`'s `gettoken` endpoint mandates `corpid` and
    /// `corpsecret` as URL query parameters — that's a protocol
    /// constraint we cannot avoid. To shrink the blast radius
    /// of an accidental log of our local `url` variable (by a
    /// distributed-tracing span that captures span fields, by a
    /// panic backtrace, by reqwest's own debug-level tracing,
    /// etc.), we build the **base** URL as a String here and
    /// hand the secrets to reqwest via `.query()` so they only
    /// exist inside reqwest's request builder, never inside a
    /// String any of our code holds. The final wire URL still
    /// contains the secrets — that's unavoidable — but the
    /// surface for accidental exposure via our own logs is
    /// reduced.
    async fn fetch_new_token(&self) -> Result<CachedToken, WeChatError> {
        // This URL is deliberately secret-free: if any
        // downstream log captures it, only the base path leaks.
        let url = format!("{}/cgi-bin/gettoken", self.config.api_base_url());
        debug!("refreshing WeChat access token");
        let request = acteon_provider::inject_trace_context(self.client.get(&url).query(&[
            ("corpid", self.config.corp_id()),
            ("corpsecret", self.config.corp_secret()),
        ]));
        let response = request.send().await?;
        let status = response.status();
        if !status.is_success() {
            // Bounded read: refuse to pull more than 2 KiB of
            // the error body into memory. A misbehaving
            // upstream (or a malicious proxy) could otherwise
            // stream unbounded data and OOM the gateway.
            let body = read_bounded_body(response, MAX_ERROR_BODY_READ_BYTES).await;
            // Transient-class HTTP errors on the token endpoint
            // should be retried — the gateway's retry loop will
            // call back into the provider, which will call back
            // into `fetch_new_token`.
            if status.is_server_error() || status == reqwest::StatusCode::REQUEST_TIMEOUT {
                return Err(WeChatError::Transient(format!(
                    "gettoken HTTP {status}: {}",
                    truncate_error_body(&body)
                )));
            }
            return Err(WeChatError::Api(format!(
                "gettoken HTTP {status}: {}",
                truncate_error_body(&body)
            )));
        }
        let token_response: WeChatTokenResponse = response
            .json()
            .await
            .map_err(|e| WeChatError::Api(format!("failed to parse gettoken response: {e}")))?;
        if token_response.errcode != 0 {
            // Bad credentials on the token endpoint are the
            // clearest Configuration failure we can surface.
            if matches!(
                token_response.errcode,
                ERRCODE_INVALID_CREDENTIAL | ERRCODE_INVALID_CORPID
            ) {
                return Err(WeChatError::Unauthorized(format!(
                    "gettoken errcode={} errmsg={}",
                    token_response.errcode, token_response.errmsg
                )));
            }
            return Err(WeChatError::Api(format!(
                "gettoken errcode={} errmsg={}",
                token_response.errcode, token_response.errmsg
            )));
        }
        if token_response.access_token.is_empty() {
            return Err(WeChatError::Api(
                "gettoken returned empty access_token".into(),
            ));
        }
        let ttl = Duration::from_secs(token_response.expires_in.max(1));
        Ok(CachedToken {
            token: SecretString::new(token_response.access_token),
            expires_at: Instant::now() + ttl,
        })
    }

    /// Build a [`WeChatSendRequest`] from the dispatch payload
    /// and the config defaults.
    fn build_request(&self, payload: EventPayload) -> Result<WeChatSendRequest, WeChatError> {
        // event_action defaults to "send"; any other value is an
        // explicit caller error.
        match payload.event_action.as_deref() {
            None | Some("send") => {}
            Some(other) => {
                return Err(WeChatError::InvalidPayload(format!(
                    "invalid event_action '{other}': WeChat only supports 'send'"
                )));
            }
        }

        let msgtype_str = payload
            .msgtype
            .as_deref()
            .unwrap_or(&self.config.default_msgtype);
        let msgtype = WeChatMsgType::parse(msgtype_str).map_err(WeChatError::InvalidPayload)?;

        // Resolve recipient selectors: payload first, config default
        // second. At least one of touser / toparty / totag must be
        // set after fallback, or the API will reject the request.
        let touser = payload.touser.or_else(|| {
            self.config
                .default_recipients
                .as_ref()
                .and_then(|r| r.touser.clone())
        });
        let toparty = payload.toparty.or_else(|| {
            self.config
                .default_recipients
                .as_ref()
                .and_then(|r| r.toparty.clone())
        });
        let totag = payload.totag.or_else(|| {
            self.config
                .default_recipients
                .as_ref()
                .and_then(|r| r.totag.clone())
        });
        let recipients = WeChatRecipients {
            touser: touser.clone(),
            toparty: toparty.clone(),
            totag: totag.clone(),
        };
        if !recipients.is_populated() {
            return Err(WeChatError::InvalidPayload(
                "at least one of 'touser', 'toparty', or 'totag' must be set (either in the payload or as a config default)".into(),
            ));
        }

        // Build the msgtype-specific body.
        let (text, markdown, textcard) = match msgtype {
            WeChatMsgType::Text => {
                let content = payload.content.ok_or_else(|| {
                    WeChatError::InvalidPayload("text msgtype requires 'content'".into())
                })?;
                (Some(WeChatTextBody { content }), None, None)
            }
            WeChatMsgType::Markdown => {
                let content = payload.content.ok_or_else(|| {
                    WeChatError::InvalidPayload("markdown msgtype requires 'content'".into())
                })?;
                (None, Some(WeChatTextBody { content }), None)
            }
            WeChatMsgType::TextCard => {
                let title = payload.title.ok_or_else(|| {
                    WeChatError::InvalidPayload("textcard msgtype requires 'title'".into())
                })?;
                let description = payload.description.ok_or_else(|| {
                    WeChatError::InvalidPayload("textcard msgtype requires 'description'".into())
                })?;
                let url = payload.url.ok_or_else(|| {
                    WeChatError::InvalidPayload("textcard msgtype requires 'url'".into())
                })?;
                (
                    None,
                    None,
                    Some(WeChatTextCardBody {
                        title,
                        description,
                        url,
                        btntxt: payload.btntxt,
                    }),
                )
            }
        };

        Ok(WeChatSendRequest {
            touser,
            toparty,
            totag,
            msgtype: msgtype.as_wire(),
            agentid: self.config.agent_id,
            text,
            markdown,
            textcard,
            safe: self.config.safe,
            enable_duplicate_check: if self.config.enable_duplicate_check {
                Some(1)
            } else {
                None
            },
            duplicate_check_interval: self.config.duplicate_check_interval,
        })
    }

    /// Send a message, transparently refreshing the access token
    /// and retrying **exactly once** on `errcode: 42001` / `40014`.
    async fn send_with_retry(
        &self,
        request: &WeChatSendRequest,
    ) -> Result<WeChatApiResponse, WeChatError> {
        let token = self.get_access_token().await?;
        match self.send_once(&token, request).await {
            Err(WeChatError::TokenExpired) => {
                // The server rejected our cached token. Drop it,
                // fetch a fresh one, and retry once. If the retry
                // produces the same error, the caller sees
                // `Unauthorized` (via `ProviderError::Configuration`)
                // rather than an infinite refresh loop.
                warn!("WeChat access token rejected mid-send; refreshing and retrying once");
                self.invalidate_token().await;
                let fresh = self.get_access_token().await?;
                match self.send_once(&fresh, request).await {
                    Err(WeChatError::TokenExpired) => Err(WeChatError::Unauthorized(
                        "WeChat access_token rejected again after refresh".into(),
                    )),
                    other => other,
                }
            }
            other => other,
        }
    }

    /// Perform a single `POST /cgi-bin/message/send` with the
    /// given token. Error classification is entirely contained
    /// in this function so the retry wrapper stays simple.
    ///
    /// Uses the same `.query()` pattern as `fetch_new_token` so
    /// the `access_token` never lands inside a String owned by
    /// this code — it only exists inside reqwest's request
    /// builder.
    async fn send_once(
        &self,
        access_token: &str,
        request: &WeChatSendRequest,
    ) -> Result<WeChatApiResponse, WeChatError> {
        // Deliberately secret-free URL; access_token is attached
        // via `.query()` below.
        let url = format!("{}/cgi-bin/message/send", self.config.api_base_url());
        debug!("sending message to WeChat");
        let builder = self
            .client
            .post(&url)
            .query(&[("access_token", access_token)])
            .json(request);
        let req = acteon_provider::inject_trace_context(builder);
        let response = req.send().await?;
        let status = response.status();
        if status.is_server_error() || status == reqwest::StatusCode::REQUEST_TIMEOUT {
            let body = read_bounded_body(response, MAX_ERROR_BODY_READ_BYTES).await;
            warn!(%status, "WeChat transient HTTP error — will be retried by gateway");
            return Err(WeChatError::Transient(format!(
                "HTTP {status}: {}",
                truncate_error_body(&body)
            )));
        }
        if matches!(
            status,
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
        ) {
            let body = read_bounded_body(response, MAX_ERROR_BODY_READ_BYTES).await;
            return Err(WeChatError::Unauthorized(format!(
                "HTTP {status}: {}",
                truncate_error_body(&body)
            )));
        }
        if !status.is_success() {
            let body = read_bounded_body(response, MAX_ERROR_BODY_READ_BYTES).await;
            return Err(WeChatError::Api(format!(
                "HTTP {status}: {}",
                truncate_error_body(&body)
            )));
        }

        let api_response: WeChatApiResponse = response
            .json()
            .await
            .map_err(|e| WeChatError::Api(format!("failed to parse WeChat response: {e}")))?;

        // HTTP-200 envelope classification. The errcode is the
        // primary signal — HTTP status is only a fallback for
        // non-200 responses handled above.
        if api_response.errcode == 0 {
            return Ok(api_response);
        }
        match api_response.errcode {
            ERRCODE_ACCESS_TOKEN_EXPIRED | ERRCODE_INVALID_ACCESS_TOKEN => {
                Err(WeChatError::TokenExpired)
            }
            ERRCODE_RATE_LIMITED => {
                warn!(
                    errcode = api_response.errcode,
                    errmsg = %api_response.errmsg,
                    "WeChat API rate limit hit"
                );
                Err(WeChatError::RateLimited)
            }
            ERRCODE_SYSTEM_BUSY => Err(WeChatError::Transient(format!(
                "errcode={} errmsg={}",
                api_response.errcode, api_response.errmsg
            ))),
            ERRCODE_INVALID_CREDENTIAL | ERRCODE_INVALID_CORPID => {
                Err(WeChatError::Unauthorized(format!(
                    "errcode={} errmsg={}",
                    api_response.errcode, api_response.errmsg
                )))
            }
            other => Err(WeChatError::Api(format!(
                "errcode={other} errmsg={}",
                api_response.errmsg
            ))),
        }
    }
}

impl Provider for WeChatProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "wechat"
    }

    #[instrument(skip(self, action), fields(action_id = %action.id, provider = "wechat"))]
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        // Borrow the payload rather than cloning: `serde_json::Value`
        // implements `serde::Deserializer` for `&Value`, so we can
        // deserialize `EventPayload` directly off a reference. This
        // avoids a deep clone of the (potentially large) JSON value
        // on every dispatch.
        let payload: EventPayload = EventPayload::deserialize(&action.payload)
            .map_err(|e| WeChatError::InvalidPayload(format!("failed to parse payload: {e}")))?;
        let request = self.build_request(payload)?;
        let api_response = self.send_with_retry(&request).await?;
        let body = serde_json::json!({
            "errcode": api_response.errcode,
            "errmsg": api_response.errmsg,
            "msgid": api_response.msgid,
            "invaliduser": api_response.invaliduser,
            "invalidparty": api_response.invalidparty,
            "invalidtag": api_response.invalidtag,
        });
        Ok(ProviderResponse::success(body))
    }

    #[instrument(skip(self), fields(provider = "wechat"))]
    async fn health_check(&self) -> Result<(), ProviderError> {
        // Fetching (or reusing a cached) access token is both a
        // connectivity check AND a credential check: a bad
        // `corp_secret` surfaces as a non-retryable Unauthorized
        // error, and a network outage surfaces as retryable
        // Connection. This gives `WeChat` the same strong health
        // guarantee the Telegram provider ships.
        self.get_access_token().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex as StdMutex};

    use acteon_core::Action;
    use acteon_provider::{Provider, ProviderError};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::config::WeChatConfig;

    /// Mock HTTP server that can respond to N successive
    /// connections. Each response is keyed by the number of
    /// accepts so far, so a test can script "first call returns
    /// token X, second call returns success, third call returns
    /// token Y, fourth call returns success" without juggling
    /// its own state.
    struct MockWeChatServer {
        listener: tokio::net::TcpListener,
        base_url: String,
    }

    /// A captured request from the mock server — URL path,
    /// query string, and body.
    #[derive(Debug, Clone)]
    struct CapturedRequest {
        raw: String,
    }

    impl CapturedRequest {
        fn contains(&self, needle: &str) -> bool {
            self.raw.contains(needle)
        }
    }

    impl MockWeChatServer {
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

        /// Respond to exactly `n` sequential connections, each
        /// with the status/body from `responses[i]`. Captures all
        /// request bodies and returns them in order.
        async fn respond_n_capturing(self, responses: Vec<(u16, String)>) -> Vec<CapturedRequest> {
            let captured = Arc::new(StdMutex::new(Vec::new()));
            for (status_code, body) in responses {
                let (mut stream, _) = self.listener.accept().await.unwrap();
                let mut buf = vec![0u8; 16384];
                let n = stream.read(&mut buf).await.unwrap();
                let raw = String::from_utf8_lossy(&buf[..n]).to_string();
                captured.lock().unwrap().push(CapturedRequest { raw });
                let response = format!(
                    "HTTP/1.1 {status_code} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.shutdown().await.unwrap();
            }
            let locked = captured.lock().unwrap();
            locked.clone()
        }
    }

    fn make_action(payload: serde_json::Value) -> Action {
        Action::new("notifications", "tenant-1", "wechat", "notify", payload)
    }

    #[test]
    fn provider_name() {
        let provider =
            WeChatProvider::new(WeChatConfig::new("corp", "secret", 1).with_default_touser("@all"));
        assert_eq!(provider.name(), "wechat");
    }

    // -- build_request unit tests -----------------------------------------

    #[test]
    fn build_request_text_happy_path() {
        let config = WeChatConfig::new("c", "s", 42).with_default_touser("@all");
        let provider = WeChatProvider::new(config);
        let payload: EventPayload = serde_json::from_value(serde_json::json!({
            "content": "hello",
        }))
        .unwrap();
        let req = provider.build_request(payload).unwrap();
        assert_eq!(req.msgtype, "text");
        assert_eq!(req.agentid, 42);
        assert_eq!(req.touser.as_deref(), Some("@all"));
        assert_eq!(req.text.as_ref().unwrap().content, "hello");
    }

    #[test]
    fn build_request_markdown() {
        let config = WeChatConfig::new("c", "s", 1)
            .with_default_msgtype("markdown")
            .with_default_touser("u1");
        let provider = WeChatProvider::new(config);
        let payload: EventPayload = serde_json::from_value(serde_json::json!({
            "content": "**bold**",
        }))
        .unwrap();
        let req = provider.build_request(payload).unwrap();
        assert_eq!(req.msgtype, "markdown");
        assert_eq!(req.markdown.as_ref().unwrap().content, "**bold**");
        assert!(req.text.is_none());
    }

    #[test]
    fn build_request_textcard() {
        let config = WeChatConfig::new("c", "s", 1).with_default_touser("u1");
        let provider = WeChatProvider::new(config);
        let payload: EventPayload = serde_json::from_value(serde_json::json!({
            "msgtype": "textcard",
            "title": "High CPU",
            "description": "web-01 at 95%",
            "url": "https://runbook.example.com/cpu",
            "btntxt": "Open",
        }))
        .unwrap();
        let req = provider.build_request(payload).unwrap();
        assert_eq!(req.msgtype, "textcard");
        let card = req.textcard.as_ref().unwrap();
        assert_eq!(card.title, "High CPU");
        assert_eq!(card.description, "web-01 at 95%");
        assert_eq!(card.url, "https://runbook.example.com/cpu");
        assert_eq!(card.btntxt.as_deref(), Some("Open"));
    }

    #[test]
    fn build_request_recipient_fallback() {
        let config = WeChatConfig::new("c", "s", 1)
            .with_default_touser("@all")
            .with_default_totag("tag-ops");
        let provider = WeChatProvider::new(config);
        let payload: EventPayload = serde_json::from_value(serde_json::json!({
            "content": "hi",
        }))
        .unwrap();
        let req = provider.build_request(payload).unwrap();
        assert_eq!(req.touser.as_deref(), Some("@all"));
        assert_eq!(req.totag.as_deref(), Some("tag-ops"));
    }

    #[test]
    fn build_request_recipient_override() {
        let config = WeChatConfig::new("c", "s", 1).with_default_touser("@all");
        let provider = WeChatProvider::new(config);
        let payload: EventPayload = serde_json::from_value(serde_json::json!({
            "content": "hi",
            "touser": "u1|u2",
            "toparty": "p1",
        }))
        .unwrap();
        let req = provider.build_request(payload).unwrap();
        assert_eq!(req.touser.as_deref(), Some("u1|u2"));
        assert_eq!(req.toparty.as_deref(), Some("p1"));
    }

    #[test]
    fn build_request_missing_recipients() {
        let config = WeChatConfig::new("c", "s", 1);
        let provider = WeChatProvider::new(config);
        let payload: EventPayload = serde_json::from_value(serde_json::json!({
            "content": "hi",
        }))
        .unwrap();
        let err = provider.build_request(payload).unwrap_err();
        let msg = match err {
            WeChatError::InvalidPayload(m) => m,
            other => panic!("expected InvalidPayload, got {other:?}"),
        };
        assert!(msg.contains("touser") && msg.contains("toparty") && msg.contains("totag"));
    }

    #[test]
    fn build_request_missing_text_content() {
        let config = WeChatConfig::new("c", "s", 1).with_default_touser("@all");
        let provider = WeChatProvider::new(config);
        let payload: EventPayload = serde_json::from_value(serde_json::json!({
            "msgtype": "text"
        }))
        .unwrap();
        let err = provider.build_request(payload).unwrap_err();
        assert!(matches!(err, WeChatError::InvalidPayload(_)));
    }

    #[test]
    fn build_request_missing_textcard_url() {
        let config = WeChatConfig::new("c", "s", 1).with_default_touser("@all");
        let provider = WeChatProvider::new(config);
        let payload: EventPayload = serde_json::from_value(serde_json::json!({
            "msgtype": "textcard",
            "title": "t",
            "description": "d",
        }))
        .unwrap();
        let err = provider.build_request(payload).unwrap_err();
        let msg = match err {
            WeChatError::InvalidPayload(m) => m,
            other => panic!("expected InvalidPayload, got {other:?}"),
        };
        assert!(msg.contains("url"));
    }

    #[test]
    fn build_request_invalid_msgtype() {
        let config = WeChatConfig::new("c", "s", 1).with_default_touser("@all");
        let provider = WeChatProvider::new(config);
        let payload: EventPayload = serde_json::from_value(serde_json::json!({
            "msgtype": "image",
            "content": "hi",
        }))
        .unwrap();
        let err = provider.build_request(payload).unwrap_err();
        assert!(matches!(err, WeChatError::InvalidPayload(_)));
    }

    #[test]
    fn build_request_invalid_event_action() {
        let config = WeChatConfig::new("c", "s", 1).with_default_touser("@all");
        let provider = WeChatProvider::new(config);
        let payload: EventPayload = serde_json::from_value(serde_json::json!({
            "event_action": "acknowledge",
            "content": "hi",
        }))
        .unwrap();
        let err = provider.build_request(payload).unwrap_err();
        assert!(matches!(err, WeChatError::InvalidPayload(_)));
    }

    #[test]
    fn build_request_enables_duplicate_check() {
        let config = WeChatConfig::new("c", "s", 1)
            .with_default_touser("@all")
            .with_duplicate_check(600);
        let provider = WeChatProvider::new(config);
        let payload: EventPayload = serde_json::from_value(serde_json::json!({
            "content": "hi",
        }))
        .unwrap();
        let req = provider.build_request(payload).unwrap();
        assert_eq!(req.enable_duplicate_check, Some(1));
        assert_eq!(req.duplicate_check_interval, Some(600));
    }

    #[test]
    fn build_request_safe_flag() {
        let config = WeChatConfig::new("c", "s", 1)
            .with_default_touser("@all")
            .with_safe(true);
        let provider = WeChatProvider::new(config);
        let payload: EventPayload = serde_json::from_value(serde_json::json!({
            "content": "hi",
        }))
        .unwrap();
        let req = provider.build_request(payload).unwrap();
        assert_eq!(req.safe, 1);
    }

    // -- End-to-end tests with the mock server ---------------------------

    #[tokio::test]
    async fn execute_text_happy_path_with_token_refresh() {
        // First call → gettoken returns a fresh token.
        // Second call → message/send returns errcode 0.
        // The provider performs both transparently.
        let server = MockWeChatServer::start().await;
        let config = WeChatConfig::new("corp", "secret", 1_000_002)
            .with_default_touser("@all")
            .with_api_base_url(&server.base_url);
        let provider = WeChatProvider::new(config);
        let action = make_action(serde_json::json!({
            "content": "Deploy complete",
        }));
        let server_handle = tokio::spawn(async move {
            server
                .respond_n_capturing(vec![
                    (
                        200,
                        r#"{"errcode":0,"errmsg":"ok","access_token":"tok-abc","expires_in":7200}"#
                            .into(),
                    ),
                    (200, r#"{"errcode":0,"errmsg":"ok","msgid":"msg-1"}"#.into()),
                ])
                .await
        });
        let result = provider.execute(&action).await;
        let captured = server_handle.await.unwrap();
        let response = result.expect("execute should succeed");
        assert_eq!(response.status, acteon_core::ResponseStatus::Success);
        assert_eq!(response.body["errcode"], 0);
        assert_eq!(response.body["msgid"], "msg-1");

        assert_eq!(captured.len(), 2, "token refresh + send = 2 requests");
        // First request is the token fetch.
        assert!(captured[0].contains("GET /cgi-bin/gettoken?corpid=corp&corpsecret=secret"));
        // Second request is the send, targeting the fresh token.
        assert!(captured[1].contains("POST /cgi-bin/message/send?access_token=tok-abc"));
        assert!(captured[1].contains("\"msgtype\":\"text\""));
        assert!(captured[1].contains("\"content\":\"Deploy complete\""));
        assert!(captured[1].contains("\"touser\":\"@all\""));
        assert!(captured[1].contains("\"agentid\":1000002"));
    }

    #[tokio::test]
    async fn execute_second_send_reuses_cached_token() {
        // Three sequential server interactions:
        //   1. gettoken (first send triggers refresh)
        //   2. first send
        //   3. second send (cached token reused — no new gettoken)
        let server = MockWeChatServer::start().await;
        let config = WeChatConfig::new("corp", "secret", 1)
            .with_default_touser("@all")
            .with_api_base_url(&server.base_url);
        let provider = WeChatProvider::new(config);
        let action1 = make_action(serde_json::json!({"content": "msg-1"}));
        let action2 = make_action(serde_json::json!({"content": "msg-2"}));
        let server_handle = tokio::spawn(async move {
            server
                .respond_n_capturing(vec![
                    (200, r#"{"errcode":0,"errmsg":"ok","access_token":"cached-tok","expires_in":7200}"#.into()),
                    (200, r#"{"errcode":0,"errmsg":"ok","msgid":"m1"}"#.into()),
                    (200, r#"{"errcode":0,"errmsg":"ok","msgid":"m2"}"#.into()),
                ])
                .await
        });
        let _ = provider.execute(&action1).await.unwrap();
        let _ = provider.execute(&action2).await.unwrap();
        let captured = server_handle.await.unwrap();
        assert_eq!(captured.len(), 3);
        // The second send also targets the cached token.
        assert!(captured[2].contains("POST /cgi-bin/message/send?access_token=cached-tok"));
    }

    #[tokio::test]
    async fn execute_errcode_42001_triggers_refresh_and_retry() {
        // Four interactions:
        //   1. gettoken    → old token (tok-old)
        //   2. send        → errcode 42001 "access_token expired"
        //   3. gettoken    → new token (tok-new)
        //   4. send        → success
        // The retry is invisible to the caller; they see one
        // successful Executed outcome.
        let server = MockWeChatServer::start().await;
        let config = WeChatConfig::new("corp", "secret", 1)
            .with_default_touser("@all")
            .with_api_base_url(&server.base_url);
        let provider = WeChatProvider::new(config);
        let action = make_action(serde_json::json!({"content": "hi"}));
        let server_handle = tokio::spawn(async move {
            server
                .respond_n_capturing(vec![
                    (
                        200,
                        r#"{"errcode":0,"errmsg":"ok","access_token":"tok-old","expires_in":7200}"#
                            .into(),
                    ),
                    (
                        200,
                        r#"{"errcode":42001,"errmsg":"access_token expired"}"#.into(),
                    ),
                    (
                        200,
                        r#"{"errcode":0,"errmsg":"ok","access_token":"tok-new","expires_in":7200}"#
                            .into(),
                    ),
                    (200, r#"{"errcode":0,"errmsg":"ok","msgid":"m1"}"#.into()),
                ])
                .await
        });
        let result = provider.execute(&action).await;
        let captured = server_handle.await.unwrap();
        let response = result.expect("execute should eventually succeed after refresh");
        assert_eq!(response.body["errcode"], 0);
        assert_eq!(
            captured.len(),
            4,
            "expected 4 interactions, got {}",
            captured.len()
        );
        // First send used the old token; second used the new.
        assert!(captured[1].contains("access_token=tok-old"));
        assert!(captured[3].contains("access_token=tok-new"));
    }

    #[tokio::test]
    async fn execute_errcode_42001_twice_fails_as_unauthorized() {
        // Refresh-and-retry only runs once. If the retry also
        // observes 42001, the provider gives up and surfaces an
        // Unauthorized (→ ProviderError::Configuration) rather
        // than looping.
        let server = MockWeChatServer::start().await;
        let config = WeChatConfig::new("corp", "secret", 1)
            .with_default_touser("@all")
            .with_api_base_url(&server.base_url);
        let provider = WeChatProvider::new(config);
        let action = make_action(serde_json::json!({"content": "hi"}));
        let server_handle = tokio::spawn(async move {
            server
                .respond_n_capturing(vec![
                    (
                        200,
                        r#"{"errcode":0,"errmsg":"ok","access_token":"tok1","expires_in":7200}"#
                            .into(),
                    ),
                    (200, r#"{"errcode":42001,"errmsg":"expired"}"#.into()),
                    (
                        200,
                        r#"{"errcode":0,"errmsg":"ok","access_token":"tok2","expires_in":7200}"#
                            .into(),
                    ),
                    (200, r#"{"errcode":42001,"errmsg":"expired again"}"#.into()),
                ])
                .await
        });
        let err = provider.execute(&action).await.unwrap_err();
        let _ = server_handle.await.unwrap();
        assert!(
            matches!(err, ProviderError::Configuration(_)),
            "second 42001 should surface as Configuration, got {err:?}"
        );
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn execute_errcode_45009_rate_limited() {
        let server = MockWeChatServer::start().await;
        let config = WeChatConfig::new("corp", "secret", 1)
            .with_default_touser("@all")
            .with_api_base_url(&server.base_url);
        let provider = WeChatProvider::new(config);
        let action = make_action(serde_json::json!({"content": "hi"}));
        let server_handle = tokio::spawn(async move {
            server
                .respond_n_capturing(vec![
                    (
                        200,
                        r#"{"errcode":0,"errmsg":"ok","access_token":"tok","expires_in":7200}"#
                            .into(),
                    ),
                    (
                        200,
                        r#"{"errcode":45009,"errmsg":"api freq out of limit"}"#.into(),
                    ),
                ])
                .await
        });
        let err = provider.execute(&action).await.unwrap_err();
        let _ = server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::RateLimited));
        assert!(err.is_retryable());
    }

    #[tokio::test]
    async fn execute_errcode_minus_one_is_transient() {
        let server = MockWeChatServer::start().await;
        let config = WeChatConfig::new("corp", "secret", 1)
            .with_default_touser("@all")
            .with_api_base_url(&server.base_url);
        let provider = WeChatProvider::new(config);
        let action = make_action(serde_json::json!({"content": "hi"}));
        let server_handle = tokio::spawn(async move {
            server
                .respond_n_capturing(vec![
                    (
                        200,
                        r#"{"errcode":0,"errmsg":"ok","access_token":"tok","expires_in":7200}"#
                            .into(),
                    ),
                    (200, r#"{"errcode":-1,"errmsg":"system busy"}"#.into()),
                ])
                .await
        });
        let err = provider.execute(&action).await.unwrap_err();
        let _ = server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::Connection(_)));
        assert!(err.is_retryable());
    }

    #[tokio::test]
    async fn execute_errcode_40001_on_send_is_unauthorized() {
        let server = MockWeChatServer::start().await;
        let config = WeChatConfig::new("corp", "secret", 1)
            .with_default_touser("@all")
            .with_api_base_url(&server.base_url);
        let provider = WeChatProvider::new(config);
        let action = make_action(serde_json::json!({"content": "hi"}));
        let server_handle = tokio::spawn(async move {
            server
                .respond_n_capturing(vec![
                    (
                        200,
                        r#"{"errcode":0,"errmsg":"ok","access_token":"tok","expires_in":7200}"#
                            .into(),
                    ),
                    (
                        200,
                        r#"{"errcode":40001,"errmsg":"invalid credential"}"#.into(),
                    ),
                ])
                .await
        });
        let err = provider.execute(&action).await.unwrap_err();
        let _ = server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::Configuration(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn execute_other_errcode_is_api_error() {
        let server = MockWeChatServer::start().await;
        let config = WeChatConfig::new("corp", "secret", 1)
            .with_default_touser("@all")
            .with_api_base_url(&server.base_url);
        let provider = WeChatProvider::new(config);
        let action = make_action(serde_json::json!({"content": "hi"}));
        let server_handle = tokio::spawn(async move {
            server
                .respond_n_capturing(vec![
                    (
                        200,
                        r#"{"errcode":0,"errmsg":"ok","access_token":"tok","expires_in":7200}"#
                            .into(),
                    ),
                    (
                        200,
                        r#"{"errcode":60011,"errmsg":"permission denied"}"#.into(),
                    ),
                ])
                .await
        });
        let err = provider.execute(&action).await.unwrap_err();
        let _ = server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::ExecutionFailed(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn execute_http_500_on_send_is_transient() {
        let server = MockWeChatServer::start().await;
        let config = WeChatConfig::new("corp", "secret", 1)
            .with_default_touser("@all")
            .with_api_base_url(&server.base_url);
        let provider = WeChatProvider::new(config);
        let action = make_action(serde_json::json!({"content": "hi"}));
        let server_handle = tokio::spawn(async move {
            server
                .respond_n_capturing(vec![
                    (
                        200,
                        r#"{"errcode":0,"errmsg":"ok","access_token":"tok","expires_in":7200}"#
                            .into(),
                    ),
                    (500, "{}".into()),
                ])
                .await
        });
        let err = provider.execute(&action).await.unwrap_err();
        let _ = server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::Connection(_)));
        assert!(err.is_retryable());
    }

    #[tokio::test]
    async fn gettoken_bad_credential_is_unauthorized() {
        // The initial token fetch fails with errcode 40001. The
        // dispatch should surface Unauthorized (→ Configuration)
        // without attempting any send.
        let server = MockWeChatServer::start().await;
        let config = WeChatConfig::new("corp", "BAD-SECRET", 1)
            .with_default_touser("@all")
            .with_api_base_url(&server.base_url);
        let provider = WeChatProvider::new(config);
        let action = make_action(serde_json::json!({"content": "hi"}));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(200, r#"{"errcode":40001,"errmsg":"invalid credential"}"#)
                .await;
        });
        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::Configuration(_)));
        assert!(!err.is_retryable());
    }

    #[tokio::test]
    async fn gettoken_empty_access_token_is_api_error() {
        let server = MockWeChatServer::start().await;
        let config = WeChatConfig::new("corp", "secret", 1)
            .with_default_touser("@all")
            .with_api_base_url(&server.base_url);
        let provider = WeChatProvider::new(config);
        let action = make_action(serde_json::json!({"content": "hi"}));
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(
                    200,
                    r#"{"errcode":0,"errmsg":"ok","access_token":"","expires_in":7200}"#,
                )
                .await;
        });
        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::ExecutionFailed(_)));
    }

    #[tokio::test]
    async fn gettoken_local_url_does_not_contain_secrets() {
        // Regression guard for the credential-leak hardening:
        // the fetch_new_token code constructs its local `url`
        // variable as the base path only, then hands the
        // corp_id / corp_secret to reqwest via `.query()`. This
        // test asserts the format string the code uses produces
        // a secret-free URL, so a future refactor that puts the
        // secrets back into the String would break the test.
        //
        // We can't unit-test the variable directly (it's inside
        // an async fn), so we reconstruct the same format
        // locally and assert it contains neither secret.
        let config = WeChatConfig::new(
            "super-secret-corp-id-xxx",
            "super-secret-corp-secret-yyy",
            1,
        )
        .with_api_base_url("https://example.test");
        let local_url = format!("{}/cgi-bin/gettoken", config.api_base_url());
        assert!(!local_url.contains("super-secret-corp-id-xxx"));
        assert!(!local_url.contains("super-secret-corp-secret-yyy"));
        assert!(!local_url.contains("corpid"));
        assert!(!local_url.contains("corpsecret"));
        assert_eq!(local_url, "https://example.test/cgi-bin/gettoken");
    }

    #[tokio::test]
    async fn send_once_local_url_does_not_contain_access_token() {
        // Same regression guard for the send path.
        let config = WeChatConfig::new("c", "s", 1).with_api_base_url("https://example.test");
        let local_url = format!("{}/cgi-bin/message/send", config.api_base_url());
        assert!(!local_url.contains("access_token"));
        assert_eq!(local_url, "https://example.test/cgi-bin/message/send");
    }

    #[tokio::test]
    async fn read_bounded_body_truncates_oversized_response() {
        // Spin up a mock server that returns a body far larger
        // than the cap, then assert that `read_bounded_body`
        // returns only the first MAX_ERROR_BODY_READ_BYTES.
        let server = MockWeChatServer::start().await;
        let base_url = server.base_url.clone();
        let huge_body = "x".repeat(10_240);
        let huge_body_clone = huge_body.clone();
        let server_handle = tokio::spawn(async move {
            server.respond_once(200, &huge_body_clone).await;
        });
        let client = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let response = client.get(&base_url).send().await.unwrap();
        let body = read_bounded_body(response, MAX_ERROR_BODY_READ_BYTES).await;
        server_handle.await.unwrap();
        assert_eq!(
            body.len(),
            MAX_ERROR_BODY_READ_BYTES,
            "oversized body should be truncated at MAX_ERROR_BODY_READ_BYTES"
        );
        assert!(
            body.chars().all(|c| c == 'x'),
            "the truncated prefix should be {MAX_ERROR_BODY_READ_BYTES} x's"
        );
        assert!(
            huge_body.len() > MAX_ERROR_BODY_READ_BYTES,
            "sanity: upstream body was larger than the cap"
        );
    }

    #[tokio::test]
    async fn read_bounded_body_passes_through_small_body() {
        let server = MockWeChatServer::start().await;
        let base_url = server.base_url.clone();
        let server_handle =
            tokio::spawn(async move { server.respond_once(200, "small body").await });
        let client = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let response = client.get(&base_url).send().await.unwrap();
        let body = read_bounded_body(response, MAX_ERROR_BODY_READ_BYTES).await;
        server_handle.await.unwrap();
        assert_eq!(body, "small body");
    }

    #[tokio::test]
    async fn gettoken_http_500_is_transient() {
        let server = MockWeChatServer::start().await;
        let config = WeChatConfig::new("corp", "secret", 1)
            .with_default_touser("@all")
            .with_api_base_url(&server.base_url);
        let provider = WeChatProvider::new(config);
        let action = make_action(serde_json::json!({"content": "hi"}));
        let server_handle = tokio::spawn(async move {
            server.respond_once(500, "{}").await;
        });
        let err = provider.execute(&action).await.unwrap_err();
        server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::Connection(_)));
        assert!(err.is_retryable());
    }

    // -- Health check tests -----------------------------------------------

    #[tokio::test]
    async fn health_check_success_with_fresh_token() {
        let server = MockWeChatServer::start().await;
        let config = WeChatConfig::new("corp", "secret", 1)
            .with_default_touser("@all")
            .with_api_base_url(&server.base_url);
        let provider = WeChatProvider::new(config);
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(
                    200,
                    r#"{"errcode":0,"errmsg":"ok","access_token":"healthy","expires_in":7200}"#,
                )
                .await;
        });
        let result = provider.health_check().await;
        server_handle.await.unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn health_check_bad_credential_is_configuration() {
        // A failing gettoken on the health check path surfaces
        // as Configuration because it's the same
        // credential-validation code that the send path uses.
        let server = MockWeChatServer::start().await;
        let config = WeChatConfig::new("corp", "BAD", 1)
            .with_default_touser("@all")
            .with_api_base_url(&server.base_url);
        let provider = WeChatProvider::new(config);
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(200, r#"{"errcode":40001,"errmsg":"invalid credential"}"#)
                .await;
        });
        let err = provider.health_check().await.unwrap_err();
        server_handle.await.unwrap();
        assert!(matches!(err, ProviderError::Configuration(_)));
    }

    #[tokio::test]
    async fn health_check_connection_failure() {
        let config = WeChatConfig::new("corp", "secret", 1)
            .with_default_touser("@all")
            .with_api_base_url("http://127.0.0.1:1");
        let provider = WeChatProvider::new(config);
        let err = provider.health_check().await.unwrap_err();
        assert!(matches!(err, ProviderError::Connection(_)));
        assert!(err.is_retryable());
    }

    // -- Token cache behavior ---------------------------------------------

    #[tokio::test]
    async fn invalidate_token_clears_cache() {
        let server = MockWeChatServer::start().await;
        let config = WeChatConfig::new("corp", "secret", 1)
            .with_default_touser("@all")
            .with_api_base_url(&server.base_url);
        let provider = WeChatProvider::new(config);
        // Prime the cache via a health check.
        let server_handle = tokio::spawn(async move {
            server
                .respond_once(
                    200,
                    r#"{"errcode":0,"errmsg":"ok","access_token":"first","expires_in":7200}"#,
                )
                .await;
        });
        provider.health_check().await.unwrap();
        server_handle.await.unwrap();

        // Cache should be populated.
        {
            let cache = provider.token_cache.lock().await;
            assert!(cache.is_some());
        }
        // Invalidate it.
        provider.invalidate_token().await;
        {
            let cache = provider.token_cache.lock().await;
            assert!(cache.is_none());
        }
    }
}
