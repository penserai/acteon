use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// A single audit record capturing the full lifecycle of a dispatched action.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AuditRecord {
    /// Unique identifier for this audit record (UUID v7).
    pub id: String,

    // -- Action fields (denormalized) --
    /// The action's unique identifier.
    pub action_id: String,
    /// Chain execution ID, if this record is part of a chain lifecycle.
    #[serde(default)]
    pub chain_id: Option<String>,
    /// Namespace the action belongs to.
    pub namespace: String,
    /// Tenant that owns the action.
    pub tenant: String,
    /// Target provider for the action.
    pub provider: String,
    /// Action type discriminator (e.g. `send_email`).
    pub action_type: String,

    // -- Rule evaluation --
    /// Verdict produced by rule evaluation (e.g. "allow", "deny", "suppress").
    pub verdict: String,
    /// Name of the rule that fired, if any.
    pub matched_rule: Option<String>,

    // -- Outcome --
    /// Final outcome of the dispatch (e.g. "executed", "failed", "suppressed").
    pub outcome: String,

    // -- Flexible JSONB columns --
    /// The action payload (omitted when privacy mode is enabled).
    pub action_payload: Option<serde_json::Value>,
    /// Details about the verdict evaluation.
    pub verdict_details: serde_json::Value,
    /// Details about the execution outcome.
    pub outcome_details: serde_json::Value,
    /// Action metadata labels.
    pub metadata: serde_json::Value,

    // -- Timestamps --
    /// When the action was dispatched.
    pub dispatched_at: DateTime<Utc>,
    /// When the action completed processing.
    pub completed_at: DateTime<Utc>,
    /// Duration of the dispatch pipeline in milliseconds.
    pub duration_ms: u64,

    // -- TTL --
    /// When this record expires (for automatic cleanup).
    pub expires_at: Option<DateTime<Utc>>,

    // -- Caller identity --
    /// Identity of the caller who triggered the action (empty if auth disabled).
    #[serde(default)]
    pub caller_id: String,
    /// Authentication method used (`"jwt"`, `"api_key"`, `"anonymous"`).
    #[serde(default)]
    pub auth_method: String,

    // -- Hash chain (compliance mode) --
    /// `SHA-256` hex digest of the canonicalized record content.
    #[serde(default)]
    pub record_hash: Option<String>,
    /// Hash of the previous record in the chain (for hash-chain integrity).
    #[serde(default)]
    pub previous_hash: Option<String>,
    /// Monotonic sequence number within the `(namespace, tenant)` pair.
    #[serde(default)]
    pub sequence_number: Option<u64>,

    /// Attachment metadata (`id`, `name`, `filename`, `content_type`, `size_bytes`)
    /// for each attachment on the action. Never contains binary data.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment_metadata: Vec<serde_json::Value>,
}

/// Query parameters for searching audit records.
#[derive(Debug, Default, Clone, Serialize, Deserialize, ToSchema)]
pub struct AuditQuery {
    /// Filter by namespace.
    pub namespace: Option<String>,
    /// Filter by tenant.
    pub tenant: Option<String>,
    /// Filter by provider.
    pub provider: Option<String>,
    /// Filter by action type.
    pub action_type: Option<String>,
    /// Filter by outcome.
    pub outcome: Option<String>,
    /// Filter by verdict.
    pub verdict: Option<String>,
    /// Filter by matched rule name.
    pub matched_rule: Option<String>,
    /// Filter by caller identity.
    pub caller_id: Option<String>,
    /// Filter by chain execution ID.
    pub chain_id: Option<String>,
    /// Only records dispatched at or after this time.
    pub from: Option<DateTime<Utc>>,
    /// Only records dispatched at or before this time.
    pub to: Option<DateTime<Utc>>,
    /// Maximum number of records to return (default 50, max 1000).
    pub limit: Option<u32>,
    /// Number of records to skip for pagination.
    pub offset: Option<u32>,
    /// When true, sort by `sequence_number ASC` instead of the default
    /// `dispatched_at DESC`. Used by hash chain verification to iterate
    /// records in chain order with bounded memory.
    #[serde(default)]
    pub sort_by_sequence_asc: bool,
}

impl AuditQuery {
    /// Return the effective limit, clamped to 1..=1000, defaulting to 50.
    pub fn effective_limit(&self) -> u32 {
        self.limit.unwrap_or(50).clamp(1, 1000)
    }

    /// Return the effective offset, defaulting to 0.
    pub fn effective_offset(&self) -> u32 {
        self.offset.unwrap_or(0)
    }
}

/// A paginated page of audit records.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AuditPage {
    /// The records matching the query.
    pub records: Vec<AuditRecord>,
    /// Total number of records matching the query (before pagination).
    pub total: u64,
    /// The limit used for this page.
    pub limit: u32,
    /// The offset used for this page.
    pub offset: u32,
}
