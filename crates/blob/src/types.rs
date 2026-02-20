use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Metadata for a stored blob (file attachment).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobMetadata {
    /// Unique blob identifier.
    pub id: String,
    /// Original filename.
    pub filename: String,
    /// MIME content type (e.g. `"application/pdf"`).
    pub content_type: String,
    /// Size in bytes.
    pub size_bytes: u64,
    /// `SHA-256` hex digest of the blob content.
    pub checksum_sha256: String,
    /// Namespace the blob belongs to.
    pub namespace: String,
    /// Tenant that owns the blob.
    pub tenant: String,
    /// When the blob was created.
    pub created_at: DateTime<Utc>,
    /// When the blob expires (if a TTL was set).
    pub expires_at: Option<DateTime<Utc>>,
}

/// A fully resolved blob: metadata plus the binary content.
#[derive(Debug, Clone)]
pub struct ResolvedBlob {
    /// Blob metadata.
    pub metadata: BlobMetadata,
    /// The raw binary content.
    pub data: bytes::Bytes,
}
