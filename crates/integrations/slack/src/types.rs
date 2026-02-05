use serde::{Deserialize, Serialize};

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
}
