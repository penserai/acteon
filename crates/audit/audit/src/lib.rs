pub mod analytics;
pub mod compliance;
pub mod cursor;
pub mod encrypt;
pub mod error;
pub mod record;
pub mod redact;
pub mod store;

pub use analytics::{AnalyticsStore, InMemoryAnalytics};
pub use compliance::{ComplianceAuditStore, HashChainAuditStore};
pub use cursor::{AuditCursor, CursorKind};
pub use encrypt::EncryptingAuditStore;
pub use error::AuditError;
pub use record::{
    A2A_AUDIT_PROVIDER, AuditEventKind, AuditPage, AuditQuery, AuditRecord, INTENT_OUTCOME,
};
pub use redact::{RedactConfig, RedactingAuditStore, Redactor};
pub use store::AuditStore;

/// Hierarchical tenant authorization scope helpers, re-exported from
/// `acteon-core` so every audit/analytics backend (which all depend on this
/// crate) can apply scope filtering without taking a direct `acteon-core` dep.
pub use acteon_core::tenant_scope;
