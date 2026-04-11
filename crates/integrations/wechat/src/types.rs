use serde::{Deserialize, Serialize};

/// Supported `WeChat` Work message types.
///
/// The `WeChat` API documents a dozen message types; we ship the
/// three that cover virtually all alerting use cases. Image,
/// voice, video, file, news, taskcard, `template_card`, mpnews,
/// and `miniprogram_notice` can be added in a follow-up if
/// demand emerges — their payload shapes are substantially more
/// complex and are really meant for content delivery rather
/// than alerting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeChatMsgType {
    /// Plain text body. Required field: `content`.
    Text,
    /// `WeChat`-flavored markdown. Required field: `content`.
    /// Note that `WeChat` supports only a subset of standard
    /// markdown — see the API docs for the syntax reference.
    Markdown,
    /// Clickable card with a title, description, and link.
    /// Required fields: `title`, `description`, `url`. Optional:
    /// `btntxt`.
    TextCard,
}

impl WeChatMsgType {
    /// Parse an Acteon `msgtype` string.
    ///
    /// # Errors
    ///
    /// Returns `Err` with the unrecognized string when the input
    /// does not match a supported message type.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "text" => Ok(Self::Text),
            "markdown" => Ok(Self::Markdown),
            "textcard" => Ok(Self::TextCard),
            other => Err(format!(
                "invalid msgtype '{other}': must be one of 'text', 'markdown', or 'textcard'"
            )),
        }
    }

    /// Wire-format string used in the JSON `msgtype` field.
    #[must_use]
    pub const fn as_wire(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Markdown => "markdown",
            Self::TextCard => "textcard",
        }
    }
}

/// Text / markdown body.
#[derive(Debug, Clone, Serialize)]
pub struct WeChatTextBody {
    /// The message body.
    pub content: String,
}

/// Textcard body.
#[derive(Debug, Clone, Serialize)]
pub struct WeChatTextCardBody {
    /// Card title (max 128 chars per the API).
    pub title: String,
    /// Card description (max 512 chars per the API).
    pub description: String,
    /// URL the card links to when tapped.
    pub url: String,
    /// Optional label for the button. Defaults to `"详情"`
    /// (Chinese for "Details") if omitted per the API docs,
    /// which we pass through unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub btntxt: Option<String>,
}

/// Request body for `POST /cgi-bin/message/send`.
///
/// Only one of `text`, `markdown`, or `textcard` is populated
/// per request, matching the `msgtype` field.
#[derive(Debug, Clone, Serialize)]
pub struct WeChatSendRequest {
    /// `|`-separated list of user IDs or `@all`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub touser: Option<String>,
    /// `|`-separated list of department (party) IDs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toparty: Option<String>,
    /// `|`-separated list of tag IDs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub totag: Option<String>,
    /// Wire-format `msgtype` (`"text"`, `"markdown"`, or
    /// `"textcard"`).
    pub msgtype: &'static str,
    /// `agentid` — which `WeChat` Work app is sending.
    pub agentid: i64,
    /// Text body (populated when `msgtype == "text"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<WeChatTextBody>,
    /// Markdown body (populated when `msgtype == "markdown"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<WeChatTextBody>,
    /// Textcard body (populated when `msgtype == "textcard"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub textcard: Option<WeChatTextCardBody>,
    /// Confidential delivery flag (0 or 1).
    pub safe: i32,
    /// Whether to enable server-side duplicate detection (0 or 1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_duplicate_check: Option<i32>,
    /// Duplicate-check window in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duplicate_check_interval: Option<u32>,
}

/// Response body from `POST /cgi-bin/message/send`.
///
/// A successful request has `errcode == 0`. The `invaliduser`,
/// `invalidparty`, and `invalidtag` fields are populated when
/// some recipients were rejected but the send itself succeeded —
/// we surface them in the outcome body so operators can see a
/// partial-delivery result.
#[derive(Debug, Clone, Deserialize)]
pub struct WeChatApiResponse {
    /// `0` on success, non-zero on failure.
    #[serde(default)]
    pub errcode: i32,
    /// Human-readable error message. `"ok"` on success.
    #[serde(default)]
    pub errmsg: String,
    /// Server-assigned message ID (populated on success).
    #[serde(default)]
    pub msgid: Option<String>,
    /// Users that were not reachable at send time.
    #[serde(default)]
    pub invaliduser: Option<String>,
    /// Departments that were not reachable at send time.
    #[serde(default)]
    pub invalidparty: Option<String>,
    /// Tags that were not reachable at send time.
    #[serde(default)]
    pub invalidtag: Option<String>,
    /// Users that were blocked by the `safe` confidential flag.
    #[serde(default)]
    pub unlicenseduser: Option<String>,
}

impl WeChatApiResponse {
    /// Whether the response indicates success.
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.errcode == 0
    }
}

/// Response body from `GET /cgi-bin/gettoken`.
#[derive(Debug, Clone, Deserialize)]
pub struct WeChatTokenResponse {
    /// `0` on success, non-zero on failure.
    #[serde(default)]
    pub errcode: i32,
    /// Human-readable error message.
    #[serde(default)]
    pub errmsg: String,
    /// The access token (populated on success).
    #[serde(default)]
    pub access_token: String,
    /// Seconds until the token expires (populated on success,
    /// always `7200` per the current API contract).
    #[serde(default)]
    pub expires_in: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msgtype_parse() {
        assert_eq!(WeChatMsgType::parse("text").unwrap(), WeChatMsgType::Text);
        assert_eq!(
            WeChatMsgType::parse("markdown").unwrap(),
            WeChatMsgType::Markdown
        );
        assert_eq!(
            WeChatMsgType::parse("textcard").unwrap(),
            WeChatMsgType::TextCard
        );
        assert!(WeChatMsgType::parse("image").is_err());
        assert!(WeChatMsgType::parse("invalid").is_err());
    }

    #[test]
    fn msgtype_wire_format() {
        assert_eq!(WeChatMsgType::Text.as_wire(), "text");
        assert_eq!(WeChatMsgType::Markdown.as_wire(), "markdown");
        assert_eq!(WeChatMsgType::TextCard.as_wire(), "textcard");
    }

    #[test]
    fn send_request_serializes_text() {
        let req = WeChatSendRequest {
            touser: Some("u1|u2".into()),
            toparty: None,
            totag: None,
            msgtype: "text",
            agentid: 1_000_002,
            text: Some(WeChatTextBody {
                content: "hello".into(),
            }),
            markdown: None,
            textcard: None,
            safe: 0,
            enable_duplicate_check: None,
            duplicate_check_interval: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["touser"], "u1|u2");
        assert_eq!(json["msgtype"], "text");
        assert_eq!(json["agentid"], 1_000_002);
        assert_eq!(json["text"]["content"], "hello");
        assert_eq!(json["safe"], 0);
        // Unused message-type fields are omitted.
        assert!(json.get("markdown").is_none());
        assert!(json.get("textcard").is_none());
        // Unused recipient fields are omitted.
        assert!(json.get("toparty").is_none());
        assert!(json.get("totag").is_none());
    }

    #[test]
    fn send_request_serializes_textcard() {
        let req = WeChatSendRequest {
            touser: None,
            toparty: Some("p1".into()),
            totag: None,
            msgtype: "textcard",
            agentid: 1,
            text: None,
            markdown: None,
            textcard: Some(WeChatTextCardBody {
                title: "High CPU".into(),
                description: "web-01 at 95%".into(),
                url: "https://runbook.example.com/cpu".into(),
                btntxt: Some("Open".into()),
            }),
            safe: 1,
            enable_duplicate_check: Some(1),
            duplicate_check_interval: Some(600),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["msgtype"], "textcard");
        assert_eq!(json["textcard"]["title"], "High CPU");
        assert_eq!(json["textcard"]["description"], "web-01 at 95%");
        assert_eq!(json["textcard"]["url"], "https://runbook.example.com/cpu");
        assert_eq!(json["textcard"]["btntxt"], "Open");
        assert_eq!(json["safe"], 1);
        assert_eq!(json["enable_duplicate_check"], 1);
        assert_eq!(json["duplicate_check_interval"], 600);
        assert!(json.get("text").is_none());
    }

    #[test]
    fn response_deserializes_success() {
        let json = r#"{"errcode":0,"errmsg":"ok","msgid":"xxxx","invaliduser":"u3"}"#;
        let resp: WeChatApiResponse = serde_json::from_str(json).unwrap();
        assert!(resp.is_success());
        assert_eq!(resp.errcode, 0);
        assert_eq!(resp.msgid.as_deref(), Some("xxxx"));
        assert_eq!(resp.invaliduser.as_deref(), Some("u3"));
    }

    #[test]
    fn response_deserializes_error() {
        let json = r#"{"errcode":40001,"errmsg":"invalid credential"}"#;
        let resp: WeChatApiResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.is_success());
        assert_eq!(resp.errcode, 40001);
        assert_eq!(resp.errmsg, "invalid credential");
    }

    #[test]
    fn response_tolerates_missing_fields() {
        let resp: WeChatApiResponse = serde_json::from_str("{}").unwrap();
        assert_eq!(resp.errcode, 0);
        assert_eq!(resp.errmsg, "");
        assert!(resp.msgid.is_none());
    }

    #[test]
    fn token_response_deserializes() {
        let json = r#"{"errcode":0,"errmsg":"ok","access_token":"abc123","expires_in":7200}"#;
        let resp: WeChatTokenResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.errcode, 0);
        assert_eq!(resp.access_token, "abc123");
        assert_eq!(resp.expires_in, 7200);
    }

    #[test]
    fn token_response_error() {
        let json = r#"{"errcode":40001,"errmsg":"invalid credential"}"#;
        let resp: WeChatTokenResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.errcode, 40001);
        assert_eq!(resp.access_token, "");
    }
}
