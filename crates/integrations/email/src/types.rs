use serde::{Deserialize, Serialize};

/// Payload for sending an email via the email provider.
///
/// This struct is deserialized from the `action.payload` JSON value. At
/// minimum, `to` and `subject` must be provided. Either `body` (plain text)
/// or `html_body` (HTML content) should be set -- if both are provided the
/// email is sent as a multipart message.
///
/// # Examples
///
/// ```
/// use acteon_email::EmailPayload;
///
/// let json = serde_json::json!({
///     "to": "user@example.com",
///     "subject": "Hello",
///     "body": "Plain text body"
/// });
/// let payload: EmailPayload = serde_json::from_value(json).unwrap();
/// assert_eq!(payload.to, "user@example.com");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailPayload {
    /// Recipient email address.
    pub to: String,

    /// Email subject line.
    pub subject: String,

    /// Plain-text email body. Optional if `html_body` is provided.
    pub body: Option<String>,

    /// HTML email body. Optional if `body` is provided.
    pub html_body: Option<String>,

    /// Optional CC recipients (comma-separated or single address).
    pub cc: Option<String>,

    /// Optional BCC recipients (comma-separated or single address).
    pub bcc: Option<String>,

    /// Optional reply-to address.
    pub reply_to: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_minimal_payload() {
        let json = serde_json::json!({
            "to": "recipient@example.com",
            "subject": "Test Subject"
        });
        let payload: EmailPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.to, "recipient@example.com");
        assert_eq!(payload.subject, "Test Subject");
        assert!(payload.body.is_none());
        assert!(payload.html_body.is_none());
        assert!(payload.cc.is_none());
        assert!(payload.bcc.is_none());
        assert!(payload.reply_to.is_none());
    }

    #[test]
    fn deserialize_full_payload() {
        let json = serde_json::json!({
            "to": "user@example.com",
            "subject": "Welcome",
            "body": "Hello, welcome!",
            "html_body": "<h1>Hello</h1><p>Welcome!</p>",
            "cc": "cc@example.com",
            "bcc": "bcc@example.com",
            "reply_to": "reply@example.com"
        });
        let payload: EmailPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.to, "user@example.com");
        assert_eq!(payload.subject, "Welcome");
        assert_eq!(payload.body.as_deref(), Some("Hello, welcome!"));
        assert_eq!(
            payload.html_body.as_deref(),
            Some("<h1>Hello</h1><p>Welcome!</p>")
        );
        assert_eq!(payload.cc.as_deref(), Some("cc@example.com"));
        assert_eq!(payload.bcc.as_deref(), Some("bcc@example.com"));
        assert_eq!(payload.reply_to.as_deref(), Some("reply@example.com"));
    }

    #[test]
    fn deserialize_with_plain_body_only() {
        let json = serde_json::json!({
            "to": "user@example.com",
            "subject": "Plain",
            "body": "Just plain text"
        });
        let payload: EmailPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.body.as_deref(), Some("Just plain text"));
        assert!(payload.html_body.is_none());
    }

    #[test]
    fn deserialize_with_html_body_only() {
        let json = serde_json::json!({
            "to": "user@example.com",
            "subject": "HTML",
            "html_body": "<p>Rich content</p>"
        });
        let payload: EmailPayload = serde_json::from_value(json).unwrap();
        assert!(payload.body.is_none());
        assert_eq!(payload.html_body.as_deref(), Some("<p>Rich content</p>"));
    }

    #[test]
    fn deserialize_missing_to_field_fails() {
        let json = serde_json::json!({
            "subject": "No recipient"
        });
        let result = serde_json::from_value::<EmailPayload>(json);
        assert!(result.is_err());
    }

    #[test]
    fn deserialize_missing_subject_field_fails() {
        let json = serde_json::json!({
            "to": "user@example.com"
        });
        let result = serde_json::from_value::<EmailPayload>(json);
        assert!(result.is_err());
    }

    #[test]
    fn payload_serde_roundtrip() {
        let payload = EmailPayload {
            to: "user@example.com".to_owned(),
            subject: "Test".to_owned(),
            body: Some("body".to_owned()),
            html_body: Some("<p>body</p>".to_owned()),
            cc: None,
            bcc: None,
            reply_to: Some("reply@example.com".to_owned()),
        };
        let json = serde_json::to_value(&payload).unwrap();
        let back: EmailPayload = serde_json::from_value(json).unwrap();
        assert_eq!(back.to, payload.to);
        assert_eq!(back.subject, payload.subject);
        assert_eq!(back.body, payload.body);
        assert_eq!(back.html_body, payload.html_body);
        assert_eq!(back.reply_to, payload.reply_to);
    }
}
