use serde::{Deserialize, Serialize};

/// Request body for the Telegram Bot API's `sendMessage` endpoint.
///
/// The API is JSON and accepts the shape documented at
/// <https://core.telegram.org/bots/api#sendmessage>. Only `chat_id`
/// and `text` are required; everything else is optional and skipped
/// at serialization time when `None`.
#[derive(Debug, Clone, Serialize)]
pub struct TelegramSendMessageRequest {
    /// Target chat identifier. Can be numeric (`-1001234567890`)
    /// or a string `@channelusername`.
    pub chat_id: String,
    /// Message body.
    pub text: String,
    /// Rich-text parse mode: `"HTML"`, `"Markdown"`, or `"MarkdownV2"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<String>,
    /// When `true`, Telegram delivers the message silently.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_notification: Option<bool>,
    /// When `true`, Telegram suppresses URL previews in the message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_web_page_preview: Option<bool>,
    /// When `true`, recipients cannot forward or save the message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protect_content: Option<bool>,
    /// ID of an existing message this message replies to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i64>,
    /// Topic ID inside a forum group — required for bots posting
    /// to a specific topic in a topics-enabled group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i64>,
}

/// Response body returned by the Telegram Bot API.
///
/// On success `ok == true` and `result` carries the full message
/// object. On failure `ok == false` and `description` carries a
/// human-readable error string; rate-limit failures additionally
/// populate `parameters.retry_after` with the number of seconds
/// to wait before retrying.
#[derive(Debug, Clone, Deserialize)]
pub struct TelegramApiResponse {
    /// `true` on success, `false` on failure. Independent of the
    /// HTTP status code.
    #[serde(default)]
    pub ok: bool,
    /// Numeric error code on failure (e.g. `400`, `403`, `429`).
    #[serde(default)]
    pub error_code: Option<i32>,
    /// Human-readable error description on failure.
    #[serde(default)]
    pub description: Option<String>,
    /// Full message object on success (opaque here — we surface
    /// the `message_id` field to the outcome body via a
    /// `serde_json::Value` because we don't need the full shape).
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    /// Error parameters. Populated with `retry_after` on HTTP 429.
    #[serde(default)]
    pub parameters: Option<TelegramResponseParameters>,
}

/// Supplementary fields returned alongside an error response.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TelegramResponseParameters {
    /// Number of seconds to wait before retrying (populated on
    /// rate-limit failures).
    #[serde(default)]
    pub retry_after: Option<u32>,
    /// When a group is migrated to a supergroup, this field holds
    /// the new supergroup's chat id.
    #[serde(default)]
    pub migrate_to_chat_id: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_serializes_minimum() {
        let req = TelegramSendMessageRequest {
            chat_id: "-1001234".into(),
            text: "hello".into(),
            parse_mode: None,
            disable_notification: None,
            disable_web_page_preview: None,
            protect_content: None,
            reply_to_message_id: None,
            message_thread_id: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["chat_id"], "-1001234");
        assert_eq!(json["text"], "hello");
        // Every optional field is omitted.
        assert!(json.get("parse_mode").is_none());
        assert!(json.get("disable_notification").is_none());
        assert!(json.get("reply_to_message_id").is_none());
    }

    #[test]
    fn request_serializes_full() {
        let req = TelegramSendMessageRequest {
            chat_id: "@opschannel".into(),
            text: "<b>ALERT</b>".into(),
            parse_mode: Some("HTML".into()),
            disable_notification: Some(false),
            disable_web_page_preview: Some(true),
            protect_content: Some(true),
            reply_to_message_id: Some(42),
            message_thread_id: Some(7),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["chat_id"], "@opschannel");
        assert_eq!(json["parse_mode"], "HTML");
        assert_eq!(json["disable_web_page_preview"], true);
        assert_eq!(json["protect_content"], true);
        assert_eq!(json["reply_to_message_id"], 42);
        assert_eq!(json["message_thread_id"], 7);
    }

    #[test]
    fn response_deserializes_success() {
        let json = r#"{"ok":true,"result":{"message_id":123,"text":"hi"}}"#;
        let resp: TelegramApiResponse = serde_json::from_str(json).unwrap();
        assert!(resp.ok);
        assert_eq!(resp.result.as_ref().unwrap()["message_id"], 123);
    }

    #[test]
    fn response_deserializes_error_with_retry_after() {
        let json = r#"{
            "ok": false,
            "error_code": 429,
            "description": "Too Many Requests: retry after 15",
            "parameters": {"retry_after": 15}
        }"#;
        let resp: TelegramApiResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.ok);
        assert_eq!(resp.error_code, Some(429));
        assert_eq!(
            resp.parameters.as_ref().and_then(|p| p.retry_after),
            Some(15)
        );
    }

    #[test]
    fn response_deserializes_error_without_parameters() {
        let json = r#"{"ok":false,"error_code":400,"description":"Bad Request: chat not found"}"#;
        let resp: TelegramApiResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.ok);
        assert_eq!(resp.error_code, Some(400));
        assert_eq!(
            resp.description.as_deref(),
            Some("Bad Request: chat not found")
        );
        assert!(resp.parameters.is_none());
    }

    #[test]
    fn response_tolerates_missing_fields() {
        let resp: TelegramApiResponse = serde_json::from_str("{}").unwrap();
        assert!(!resp.ok);
        assert!(resp.error_code.is_none());
        assert!(resp.description.is_none());
        assert!(resp.result.is_none());
    }
}
