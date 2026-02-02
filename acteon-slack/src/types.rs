use serde::{Deserialize, Serialize};

// ─── chat.postMessage ────────────────────────────────────────────────

/// Request body for the Slack `chat.postMessage` API.
#[derive(Debug, Clone, Serialize)]
pub struct SlackPostMessageRequest {
    /// Target channel, DM, or group ID.
    pub channel: String,

    /// Plain-text message content. At least one of `text` or `blocks` must be
    /// present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    /// Block Kit layout blocks for rich message formatting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocks: Option<serde_json::Value>,
}

// ─── API Responses ───────────────────────────────────────────────────

/// Envelope returned by all Slack Web API methods.
#[derive(Debug, Clone, Deserialize)]
pub struct SlackApiResponse {
    /// Whether the API call succeeded.
    pub ok: bool,

    /// Human-readable error code when `ok` is `false`.
    pub error: Option<String>,

    /// Channel the message was posted to (present on success).
    pub channel: Option<String>,

    /// Timestamp identifier of the posted message (present on success).
    pub ts: Option<String>,
}

/// Response from the Slack `auth.test` API.
#[derive(Debug, Clone, Deserialize)]
pub struct SlackAuthTestResponse {
    /// Whether the API call succeeded.
    pub ok: bool,

    /// Human-readable error code when `ok` is `false`.
    pub error: Option<String>,

    /// Authenticated user ID.
    pub user_id: Option<String>,

    /// Authenticated team ID.
    pub team_id: Option<String>,
}

// ─── files.getUploadURLExternal ──────────────────────────────────────

/// Request body for the Slack `files.getUploadURLExternal` API.
#[derive(Debug, Clone, Serialize)]
pub struct SlackGetUploadUrlRequest {
    /// The name of the file to upload.
    pub filename: String,
    /// Length of the file in bytes.
    pub length: u64,
}

/// Response from `files.getUploadURLExternal`.
#[derive(Debug, Clone, Deserialize)]
pub struct SlackGetUploadUrlResponse {
    pub ok: bool,
    pub error: Option<String>,
    /// Presigned URL to upload the file data to.
    pub upload_url: Option<String>,
    /// Opaque file ID used to complete the upload.
    pub file_id: Option<String>,
}

// ─── files.completeUploadExternal ────────────────────────────────────

/// Request body for the Slack `files.completeUploadExternal` API.
#[derive(Debug, Clone, Serialize)]
pub struct SlackCompleteUploadRequest {
    /// Files to complete, each with their `id` and optional `title`.
    pub files: Vec<SlackFileReference>,
    /// Channel to share the file in (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    /// Initial comment to post with the file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_comment: Option<String>,
}

/// A reference to a file being completed, containing its ID.
#[derive(Debug, Clone, Serialize)]
pub struct SlackFileReference {
    /// The file ID returned from `getUploadURLExternal`.
    pub id: String,
    /// Optional title for the file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Response from `files.completeUploadExternal`.
#[derive(Debug, Clone, Deserialize)]
pub struct SlackCompleteUploadResponse {
    pub ok: bool,
    pub error: Option<String>,
    /// Completed file objects.
    pub files: Option<Vec<SlackFileObject>>,
}

/// Minimal representation of a Slack file object.
#[derive(Debug, Clone, Deserialize)]
pub struct SlackFileObject {
    pub id: Option<String>,
    pub title: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_message_request_serializes_without_optional_fields() {
        let req = SlackPostMessageRequest {
            channel: "C12345".into(),
            text: Some("hello".into()),
            blocks: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["channel"], "C12345");
        assert_eq!(json["text"], "hello");
        assert!(json.get("blocks").is_none());
    }

    #[test]
    fn post_message_request_includes_blocks_when_present() {
        let req = SlackPostMessageRequest {
            channel: "C12345".into(),
            text: None,
            blocks: Some(serde_json::json!([{"type": "section"}])),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("text").is_none());
        assert!(json.get("blocks").is_some());
    }

    #[test]
    fn api_response_deserializes_success() {
        let json = r#"{"ok": true, "channel": "C12345", "ts": "1234567890.123456"}"#;
        let resp: SlackApiResponse = serde_json::from_str(json).unwrap();
        assert!(resp.ok);
        assert_eq!(resp.channel.as_deref(), Some("C12345"));
        assert_eq!(resp.ts.as_deref(), Some("1234567890.123456"));
        assert!(resp.error.is_none());
    }

    #[test]
    fn api_response_deserializes_error() {
        let json = r#"{"ok": false, "error": "channel_not_found"}"#;
        let resp: SlackApiResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.ok);
        assert_eq!(resp.error.as_deref(), Some("channel_not_found"));
    }

    #[test]
    fn auth_test_response_deserializes() {
        let json = r#"{"ok": true, "user_id": "U123", "team_id": "T456"}"#;
        let resp: SlackAuthTestResponse = serde_json::from_str(json).unwrap();
        assert!(resp.ok);
        assert_eq!(resp.user_id.as_deref(), Some("U123"));
        assert_eq!(resp.team_id.as_deref(), Some("T456"));
    }

    #[test]
    fn get_upload_url_request_serializes() {
        let req = SlackGetUploadUrlRequest {
            filename: "chart.png".into(),
            length: 1024,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["filename"], "chart.png");
        assert_eq!(json["length"], 1024);
    }

    #[test]
    fn get_upload_url_response_deserializes() {
        let json = r#"{"ok":true,"upload_url":"https://files.slack.com/upload/xxx","file_id":"F123"}"#;
        let resp: SlackGetUploadUrlResponse = serde_json::from_str(json).unwrap();
        assert!(resp.ok);
        assert_eq!(
            resp.upload_url.as_deref(),
            Some("https://files.slack.com/upload/xxx")
        );
        assert_eq!(resp.file_id.as_deref(), Some("F123"));
    }

    #[test]
    fn complete_upload_request_serializes() {
        let req = SlackCompleteUploadRequest {
            files: vec![SlackFileReference {
                id: "F123".into(),
                title: Some("My Chart".into()),
            }],
            channel_id: Some("C456".into()),
            initial_comment: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["files"][0]["id"], "F123");
        assert_eq!(json["files"][0]["title"], "My Chart");
        assert_eq!(json["channel_id"], "C456");
        assert!(json.get("initial_comment").is_none());
    }

    #[test]
    fn complete_upload_response_deserializes() {
        let json = r#"{"ok":true,"files":[{"id":"F123","title":"chart.png"}]}"#;
        let resp: SlackCompleteUploadResponse = serde_json::from_str(json).unwrap();
        assert!(resp.ok);
        let files = resp.files.unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].id.as_deref(), Some("F123"));
    }
}
