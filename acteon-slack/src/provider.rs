use acteon_core::{Action, ProviderResponse};
use acteon_provider::{Provider, ProviderError};
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, instrument, warn};

use crate::config::SlackConfig;
use crate::error::SlackError;
use crate::image_gen::{self, ImageSpec};
use crate::types::{
    SlackApiResponse, SlackAuthTestResponse, SlackCompleteUploadRequest,
    SlackCompleteUploadResponse, SlackFileReference, SlackGetUploadUrlResponse,
    SlackPostMessageRequest,
};

/// Slack provider that sends messages and uploads files via the Slack Web API.
///
/// Implements the [`Provider`] trait so it can be registered in the provider
/// registry and used by the action executor.
///
/// # Supported action types
///
/// - `send_message` — posts a message via `chat.postMessage`
/// - `upload_image` — generates (or accepts) an image and uploads it via
///   the Slack files.uploadV2 flow
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

/// Fields extracted from an action payload for image upload.
#[derive(Debug, Deserialize)]
struct ImageUploadPayload {
    /// Target channel to share the uploaded file to.
    channel: Option<String>,
    /// Filename for the upload (defaults to "image.png").
    filename: Option<String>,
    /// Optional title for the file in Slack.
    title: Option<String>,
    /// Optional initial comment when sharing.
    initial_comment: Option<String>,
    /// Base64-encoded PNG image data. If provided, `generate` is ignored.
    image_base64: Option<String>,
    /// Image generation specification. Used when `image_base64` is absent.
    generate: Option<ImageSpec>,
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

    /// Execute the `send_message` action type.
    async fn execute_send_message(
        &self,
        action: &Action,
    ) -> Result<ProviderResponse, ProviderError> {
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

    /// Execute the `upload_image` action type.
    ///
    /// Uses the Slack files.uploadV2 three-step flow:
    /// 1. `files.getUploadURLExternal` — obtain a presigned upload URL
    /// 2. POST file bytes to the presigned URL
    /// 3. `files.completeUploadExternal` — finalize and share the file
    async fn execute_upload_image(
        &self,
        action: &Action,
    ) -> Result<ProviderResponse, ProviderError> {
        let payload: ImageUploadPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| SlackError::InvalidPayload(format!("failed to parse payload: {e}")))?;

        let channel = self.resolve_channel(payload.channel.as_deref())?;
        let filename = payload
            .filename
            .unwrap_or_else(|| "image.png".to_owned());

        // Obtain image bytes: either from base64 or by generating.
        let image_bytes = if let Some(b64) = &payload.image_base64 {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| {
                    SlackError::InvalidPayload(format!("invalid base64 image data: {e}"))
                })?
        } else if let Some(spec) = &payload.generate {
            image_gen::generate_image(spec)?
        } else {
            return Err(SlackError::InvalidPayload(
                "payload must contain either 'image_base64' or 'generate' specification".into(),
            )
            .into());
        };

        debug!(
            filename = %filename,
            bytes = image_bytes.len(),
            channel = %channel,
            "uploading image to Slack"
        );

        // Step 1: Get upload URL.
        let upload_url_resp = self.get_upload_url(&filename, image_bytes.len() as u64).await?;

        let presigned_url = upload_url_resp.upload_url.ok_or_else(|| {
            SlackError::Api("files.getUploadURLExternal returned no upload_url".into())
        })?;
        let file_id = upload_url_resp.file_id.ok_or_else(|| {
            SlackError::Api("files.getUploadURLExternal returned no file_id".into())
        })?;

        // Step 2: Upload file bytes to the presigned URL.
        self.upload_to_presigned_url(&presigned_url, &image_bytes, &filename)
            .await?;

        // Step 3: Complete the upload and share to channel.
        let complete_resp = self
            .complete_upload(
                &file_id,
                payload.title.as_deref().or(Some(&filename)),
                Some(&channel),
                payload.initial_comment.as_deref(),
            )
            .await?;

        let file_ids: Vec<String> = complete_resp
            .files
            .unwrap_or_default()
            .iter()
            .filter_map(|f| f.id.clone())
            .collect();

        let body = serde_json::json!({
            "ok": true,
            "file_id": file_id,
            "file_ids": file_ids,
            "channel": channel,
        });

        Ok(ProviderResponse::success(body))
    }

    /// Step 1: Call `files.getUploadURLExternal` to obtain a presigned upload URL.
    async fn get_upload_url(
        &self,
        filename: &str,
        length: u64,
    ) -> Result<SlackGetUploadUrlResponse, SlackError> {
        let url = self.api_url("files.getUploadURLExternal");

        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.config.token)
            .form(&[
                ("filename", filename.to_owned()),
                ("length", length.to_string()),
            ])
            .send()
            .await?;

        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(SlackError::RateLimited);
        }

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(SlackError::Api(format!(
                "files.getUploadURLExternal HTTP {status}: {body}"
            )));
        }

        let resp: SlackGetUploadUrlResponse = response.json().await?;

        if !resp.ok {
            let err = resp.error.unwrap_or_else(|| "unknown_error".into());
            return Err(SlackError::Api(format!(
                "files.getUploadURLExternal failed: {err}"
            )));
        }

        Ok(resp)
    }

    /// Step 2: Upload file bytes to the presigned URL via multipart form.
    async fn upload_to_presigned_url(
        &self,
        presigned_url: &str,
        data: &[u8],
        filename: &str,
    ) -> Result<(), SlackError> {
        let part = reqwest::multipart::Part::bytes(data.to_vec())
            .file_name(filename.to_owned())
            .mime_str("image/png")
            .map_err(|e| SlackError::InvalidPayload(format!("invalid mime type: {e}")))?;

        let form = reqwest::multipart::Form::new().part("file", part);

        let response = self
            .client
            .post(presigned_url)
            .multipart(form)
            .send()
            .await?;

        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(SlackError::RateLimited);
        }

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(SlackError::Api(format!(
                "file upload to presigned URL failed HTTP {status}: {body}"
            )));
        }

        debug!("file bytes uploaded to presigned URL successfully");
        Ok(())
    }

    /// Step 3: Call `files.completeUploadExternal` to finalize the upload
    /// and share it to a channel.
    async fn complete_upload(
        &self,
        file_id: &str,
        title: Option<&str>,
        channel_id: Option<&str>,
        initial_comment: Option<&str>,
    ) -> Result<SlackCompleteUploadResponse, SlackError> {
        let url = self.api_url("files.completeUploadExternal");

        let request = SlackCompleteUploadRequest {
            files: vec![SlackFileReference {
                id: file_id.to_owned(),
                title: title.map(str::to_owned),
            }],
            channel_id: channel_id.map(str::to_owned),
            initial_comment: initial_comment.map(str::to_owned),
        };

        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.config.token)
            .json(&request)
            .send()
            .await?;

        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(SlackError::RateLimited);
        }

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(SlackError::Api(format!(
                "files.completeUploadExternal HTTP {status}: {body}"
            )));
        }

        let resp: SlackCompleteUploadResponse = response.json().await?;

        if !resp.ok {
            let err = resp.error.unwrap_or_else(|| "unknown_error".into());
            return Err(SlackError::Api(format!(
                "files.completeUploadExternal failed: {err}"
            )));
        }

        debug!(file_id = %file_id, "file upload completed successfully");
        Ok(resp)
    }
}

impl Provider for SlackProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "slack"
    }

    #[instrument(skip(self, action), fields(action_id = %action.id, provider = "slack"))]
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        match action.action_type.as_str() {
            "send_message" => self.execute_send_message(action).await,
            "upload_image" => self.execute_upload_image(action).await,
            other => Err(ProviderError::ExecutionFailed(format!(
                "unsupported action type: '{other}' (supported: send_message, upload_image)"
            ))),
        }
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

    /// A mock server that handles the three-step file upload flow.
    struct MockUploadServer {
        listener: tokio::net::TcpListener,
        base_url: String,
    }

    impl MockUploadServer {
        async fn start() -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("failed to bind mock upload server");
            let port = listener.local_addr().unwrap().port();
            let base_url = format!("http://127.0.0.1:{port}");
            Self { listener, base_url }
        }

        /// Handle the three sequential requests of the upload flow.
        async fn handle_upload_flow(self) {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};

            // Request 1: files.getUploadURLExternal
            let (mut stream, _) = self.listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let _ = stream.read(&mut buf).await.unwrap();

            let upload_url = format!("{}/upload-target", self.base_url);
            let body = format!(
                r#"{{"ok":true,"upload_url":"{upload_url}","file_id":"F_MOCK_123"}}"#
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n\
                 {body}",
                body.len()
            );
            stream.write_all(resp.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();

            // Request 2: Upload to presigned URL
            let (mut stream, _) = self.listener.accept().await.unwrap();
            let mut buf = vec![0u8; 65536];
            let _ = stream.read(&mut buf).await.unwrap();

            let body = "OK";
            let resp = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: text/plain\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n\
                 {body}",
                body.len()
            );
            stream.write_all(resp.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();

            // Request 3: files.completeUploadExternal
            let (mut stream, _) = self.listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let _ = stream.read(&mut buf).await.unwrap();

            let body = r#"{"ok":true,"files":[{"id":"F_MOCK_123","title":"image.png"}]}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n\
                 {body}",
                body.len()
            );
            stream.write_all(resp.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
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

    fn make_upload_action(payload: serde_json::Value) -> Action {
        Action::new(
            "notifications",
            "tenant-1",
            "slack",
            "upload_image",
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
    async fn execute_unsupported_action_type() {
        let config = SlackConfig::new("xoxb-test").with_api_base_url("http://localhost:1");
        let provider = SlackProvider::new(config);

        let action = Action::new(
            "notifications",
            "tenant-1",
            "slack",
            "delete_channel",
            serde_json::json!({}),
        );

        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::ExecutionFailed(_)));
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

    // ─── Image upload tests ──────────────────────────────────────────

    #[tokio::test]
    async fn upload_image_with_generate() {
        let server = MockUploadServer::start().await;
        let config = SlackConfig::new("xoxb-test").with_api_base_url(&server.base_url);
        let provider = SlackProvider::new(config);

        let action = make_upload_action(serde_json::json!({
            "channel": "#general",
            "filename": "chart.png",
            "title": "My Chart",
            "generate": {
                "type": "solid_color",
                "width": 10,
                "height": 10,
                "color": "#FF0000"
            }
        }));

        let server_handle = tokio::spawn(async move {
            server.handle_upload_flow().await;
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        let response = result.expect("upload_image should succeed");
        assert_eq!(response.status, acteon_core::ResponseStatus::Success);
        assert_eq!(response.body["ok"], true);
        assert_eq!(response.body["file_id"], "F_MOCK_123");
        assert_eq!(response.body["channel"], "#general");
    }

    #[tokio::test]
    async fn upload_image_with_base64() {
        let server = MockUploadServer::start().await;
        let config = SlackConfig::new("xoxb-test").with_api_base_url(&server.base_url);
        let provider = SlackProvider::new(config);

        // Create a tiny valid PNG via the image generator, then base64 it.
        let png_bytes = image_gen::generate_image(&ImageSpec::SolidColor {
            width: 2,
            height: 2,
            color: "#00FF00".into(),
        })
        .unwrap();
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);

        let action = make_upload_action(serde_json::json!({
            "channel": "#general",
            "image_base64": b64,
        }));

        let server_handle = tokio::spawn(async move {
            server.handle_upload_flow().await;
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        let response = result.expect("upload_image with base64 should succeed");
        assert_eq!(response.status, acteon_core::ResponseStatus::Success);
        assert_eq!(response.body["ok"], true);
    }

    #[tokio::test]
    async fn upload_image_missing_source() {
        let config = SlackConfig::new("xoxb-test").with_api_base_url("http://localhost:1");
        let provider = SlackProvider::new(config);

        let action = make_upload_action(serde_json::json!({
            "channel": "#general",
        }));

        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
    }

    #[tokio::test]
    async fn upload_image_invalid_base64() {
        let config = SlackConfig::new("xoxb-test").with_api_base_url("http://localhost:1");
        let provider = SlackProvider::new(config);

        let action = make_upload_action(serde_json::json!({
            "channel": "#general",
            "image_base64": "not-valid-base64!!!",
        }));

        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
    }

    #[tokio::test]
    async fn upload_image_with_bar_chart() {
        let server = MockUploadServer::start().await;
        let config = SlackConfig::new("xoxb-test").with_api_base_url(&server.base_url);
        let provider = SlackProvider::new(config);

        let action = make_upload_action(serde_json::json!({
            "channel": "#metrics",
            "filename": "metrics.png",
            "title": "Weekly Metrics",
            "initial_comment": "Here are this week's metrics!",
            "generate": {
                "type": "bar_chart",
                "width": 400,
                "height": 200,
                "values": [10.0, 25.0, 15.0, 30.0, 20.0],
                "bar_color": "#3366CC",
                "background_color": "#FFFFFF"
            }
        }));

        let server_handle = tokio::spawn(async move {
            server.handle_upload_flow().await;
        });

        let result = provider.execute(&action).await;
        server_handle.await.unwrap();

        let response = result.expect("bar chart upload should succeed");
        assert_eq!(response.status, acteon_core::ResponseStatus::Success);
    }
}
