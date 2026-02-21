use serde::{Deserialize, Serialize};

/// An attachment to be included with an action dispatch.
///
/// Each attachment carries a user-defined `id` that makes it referenceable
/// across actions and sub-chains, a human-readable `name`, the original
/// `filename` with extension, a MIME `content_type`, and the file content
/// as `base64`-encoded data.
///
/// Providers that support attachments (email, Slack, Discord, webhook) resolve
/// these at dispatch time; providers that don't simply ignore them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Attachment {
    /// User-set identifier for referencing this attachment in chains.
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// Filename with extension (e.g. `"report.pdf"`).
    pub filename: String,
    /// MIME content type (e.g. `"application/pdf"`).
    pub content_type: String,
    /// `Base64`-encoded file content.
    pub data_base64: String,
}

/// A fully resolved attachment with decoded binary data.
///
/// Built by the gateway after decoding `base64` content. Passed to providers
/// via [`DispatchContext`](crate::attachment::ResolvedAttachment).
#[derive(Debug, Clone)]
pub struct ResolvedAttachment {
    /// User-set identifier.
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// Filename with extension.
    pub filename: String,
    /// MIME content type.
    pub content_type: String,
    /// Decoded binary content.
    pub data: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_serde_roundtrip() {
        let attachment = Attachment {
            id: "att-1".into(),
            name: "Hello File".into(),
            filename: "hello.txt".into(),
            content_type: "text/plain".into(),
            data_base64: "SGVsbG8gV29ybGQ=".into(),
        };
        let json = serde_json::to_string(&attachment).unwrap();
        assert!(json.contains("\"id\":\"att-1\""));
        assert!(json.contains("\"name\":\"Hello File\""));
        let back: Attachment = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "att-1");
        assert_eq!(back.name, "Hello File");
        assert_eq!(back.filename, "hello.txt");
        assert_eq!(back.content_type, "text/plain");
        assert_eq!(back.data_base64, "SGVsbG8gV29ybGQ=");
    }

    #[test]
    fn empty_attachments_vec_deserializes_from_missing_field() {
        // Simulates backward compatibility: old payloads without "attachments"
        let json = r#"[]"#;
        let attachments: Vec<Attachment> = serde_json::from_str(json).unwrap();
        assert!(attachments.is_empty());
    }
}
