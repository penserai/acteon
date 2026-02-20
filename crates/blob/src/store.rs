use async_trait::async_trait;
use bytes::Bytes;

use crate::error::BlobError;
use crate::types::BlobMetadata;

/// Pluggable blob storage backend for file attachments.
///
/// Implementors provide the actual storage mechanism (e.g. S3, GCS, filesystem).
/// Acteon does not ship a built-in implementation; users bring their own.
#[async_trait]
pub trait BlobStore: Send + Sync {
    /// Store a blob and return its metadata.
    ///
    /// The store assigns a unique ID and computes a `SHA-256` checksum.
    /// An optional `ttl_seconds` sets the expiration time.
    async fn put(
        &self,
        namespace: &str,
        tenant: &str,
        filename: &str,
        content_type: &str,
        data: Bytes,
        ttl_seconds: Option<u64>,
    ) -> Result<BlobMetadata, BlobError>;

    /// Retrieve a blob by ID, returning both metadata and content.
    ///
    /// Returns `None` if the blob does not exist or has expired.
    async fn get(&self, id: &str) -> Result<Option<(BlobMetadata, Bytes)>, BlobError>;

    /// Retrieve only the metadata for a blob (without downloading content).
    async fn get_metadata(&self, id: &str) -> Result<Option<BlobMetadata>, BlobError>;

    /// Delete a blob by ID. Returns `true` if the blob existed.
    async fn delete(&self, id: &str) -> Result<bool, BlobError>;

    /// Remove all expired blobs. Returns the number of blobs removed.
    async fn reap_expired(&self) -> Result<u64, BlobError>;

    /// List blobs for a given namespace and tenant.
    async fn list(
        &self,
        namespace: &str,
        tenant: &str,
        limit: Option<u32>,
    ) -> Result<Vec<BlobMetadata>, BlobError>;
}
