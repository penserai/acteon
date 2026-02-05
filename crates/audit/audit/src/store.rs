use async_trait::async_trait;

use crate::error::AuditError;
use crate::record::{AuditPage, AuditQuery, AuditRecord};

/// Trait for audit record storage backends.
///
/// Implementations must be `Send + Sync` to be shared across async tasks.
#[async_trait]
pub trait AuditStore: Send + Sync {
    /// Persist an audit record.
    async fn record(&self, entry: AuditRecord) -> Result<(), AuditError>;

    /// Retrieve the most recent audit record for a given action ID.
    async fn get_by_action_id(&self, action_id: &str) -> Result<Option<AuditRecord>, AuditError>;

    /// Retrieve an audit record by its unique ID.
    async fn get_by_id(&self, id: &str) -> Result<Option<AuditRecord>, AuditError>;

    /// Query audit records with filters and pagination.
    async fn query(&self, query: &AuditQuery) -> Result<AuditPage, AuditError>;

    /// Remove expired records. Returns the number of records deleted.
    async fn cleanup_expired(&self) -> Result<u64, AuditError>;
}
