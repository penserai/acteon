use serde::{Deserialize, Serialize};

/// An attachment to be included with an action dispatch.
///
/// Attachments can be either references to blobs stored in an external blob
/// store, or inline base64-encoded data. Providers that support attachments
/// (email, Slack, Discord, webhook) resolve these at execution time; providers
/// that don't simply ignore them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum Attachment {
    /// A reference to a blob stored in an external [`BlobStore`](acteon_blob::BlobStore).
    ///
    /// The blob is resolved at dispatch time. Requires a blob store to be
    /// configured on the gateway.
    BlobRef {
        /// The blob identifier returned by the blob store.
        blob_id: String,
        /// Optional filename override (uses the blob's original filename if omitted).
        filename: Option<String>,
    },
    /// An inline base64-encoded file.
    ///
    /// Suitable for small files that don't warrant a separate upload step.
    /// The server config controls the maximum allowed inline size.
    Inline {
        /// Base64-encoded file content.
        data_base64: String,
        /// MIME content type (e.g. `"application/pdf"`).
        content_type: String,
        /// Filename for the attachment.
        filename: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_blob_ref_serde_roundtrip() {
        let attachment = Attachment::BlobRef {
            blob_id: "blob-123".into(),
            filename: Some("report.pdf".into()),
        };
        let json = serde_json::to_string(&attachment).unwrap();
        assert!(json.contains("\"type\":\"blob_ref\""));
        let back: Attachment = serde_json::from_str(&json).unwrap();
        match back {
            Attachment::BlobRef { blob_id, filename } => {
                assert_eq!(blob_id, "blob-123");
                assert_eq!(filename.as_deref(), Some("report.pdf"));
            }
            _ => panic!("expected BlobRef"),
        }
    }

    #[test]
    fn attachment_inline_serde_roundtrip() {
        let attachment = Attachment::Inline {
            data_base64: "SGVsbG8gV29ybGQ=".into(),
            content_type: "text/plain".into(),
            filename: "hello.txt".into(),
        };
        let json = serde_json::to_string(&attachment).unwrap();
        assert!(json.contains("\"type\":\"inline\""));
        let back: Attachment = serde_json::from_str(&json).unwrap();
        match back {
            Attachment::Inline {
                data_base64,
                content_type,
                filename,
            } => {
                assert_eq!(data_base64, "SGVsbG8gV29ybGQ=");
                assert_eq!(content_type, "text/plain");
                assert_eq!(filename, "hello.txt");
            }
            _ => panic!("expected Inline"),
        }
    }

    #[test]
    fn empty_attachments_vec_deserializes_from_missing_field() {
        // Simulates backward compatibility: old payloads without "attachments"
        let json = r#"[]"#;
        let attachments: Vec<Attachment> = serde_json::from_str(json).unwrap();
        assert!(attachments.is_empty());
    }
}
