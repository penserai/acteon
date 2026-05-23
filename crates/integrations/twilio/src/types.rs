use serde::{Deserialize, Serialize};

/// Form-encoded request body for the Twilio Messages API.
///
/// Twilio expects `application/x-www-form-urlencoded` rather than JSON.
#[derive(Debug, Clone, Serialize)]
pub struct TwilioSendMessageRequest {
    /// Destination phone number in E.164 format.
    #[serde(rename = "To")]
    pub to: String,

    /// Sender phone number or messaging service SID.
    #[serde(rename = "From")]
    pub from: String,

    /// Message body text.
    #[serde(rename = "Body")]
    pub body: String,

    /// Optional media URL for MMS.
    #[serde(rename = "MediaUrl", skip_serializing_if = "Option::is_none")]
    pub media_url: Option<String>,
}

/// Response from the Twilio Messages API.
#[derive(Debug, Clone, Deserialize)]
pub struct TwilioApiResponse {
    /// Message SID (unique identifier).
    pub sid: Option<String>,

    /// Message status (e.g., `"queued"`, `"sent"`, `"delivered"`).
    pub status: Option<String>,

    /// Twilio error code (present on failure).
    pub error_code: Option<i32>,

    /// Twilio error message (present on failure).
    pub error_message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_message_request_serializes_form_encoded() {
        let req = TwilioSendMessageRequest {
            to: "+15559876543".into(),
            from: "+15551234567".into(),
            body: "Hello from Acteon!".into(),
            media_url: None,
        };
        let encoded = serde_urlencoded::to_string(&req).unwrap();
        assert!(encoded.contains("To=%2B15559876543"));
        assert!(encoded.contains("From=%2B15551234567"));
        assert!(encoded.contains("Body=Hello+from+Acteon%21"));
        assert!(!encoded.contains("MediaUrl"));
    }

    #[test]
    fn send_message_request_includes_media_url() {
        let req = TwilioSendMessageRequest {
            to: "+15559876543".into(),
            from: "+15551234567".into(),
            body: "Check this out".into(),
            media_url: Some("https://example.com/image.jpg".into()),
        };
        let encoded = serde_urlencoded::to_string(&req).unwrap();
        assert!(encoded.contains("MediaUrl="));
    }

    #[test]
    fn api_response_deserializes_success() {
        let json = r#"{"sid":"SM123","status":"queued","error_code":null,"error_message":null}"#;
        let resp: TwilioApiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.sid.as_deref(), Some("SM123"));
        assert_eq!(resp.status.as_deref(), Some("queued"));
        assert!(resp.error_code.is_none());
    }

    #[test]
    fn api_response_deserializes_error() {
        let json = r#"{"sid":null,"status":null,"error_code":21211,"error_message":"Invalid 'To' Phone Number"}"#;
        let resp: TwilioApiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.error_code, Some(21211));
        assert_eq!(
            resp.error_message.as_deref(),
            Some("Invalid 'To' Phone Number")
        );
    }
}
