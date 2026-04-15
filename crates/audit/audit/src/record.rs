use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single audit record capturing the full lifecycle of a dispatched action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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

    // -- Action signing --
    /// Ed25519 signature over the action's canonical bytes, base64-encoded.
    #[serde(default)]
    pub signature: Option<String>,
    /// Identifier of the key that produced the signature.
    #[serde(default)]
    pub signer_id: Option<String>,
    /// Optional key identifier for rotation. When the same `signer_id`
    /// has multiple active keys, `kid` records which one signed this
    /// action. `None` for legacy single-key signatures.
    #[serde(default)]
    pub kid: Option<String>,
    /// SHA-256 hex digest of the action's canonical bytes at dispatch
    /// time. Stored so the verify endpoint can confirm the signature
    /// without needing to reconstruct the full original action (which
    /// the audit record does not carry in its entirety).
    #[serde(default)]
    pub canonical_hash: Option<String>,
}

/// Query parameters for searching audit records.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
    /// Filter by the `signer_id` recorded on the audit entry. Useful
    /// during incident response to list every action a particular
    /// signer dispatched (e.g. a compromised key before its rotation).
    /// Unsigned actions never match.
    #[serde(default)]
    pub signer_id: Option<String>,
    /// Filter by the `kid` (key identifier) recorded on the audit
    /// entry. Combined with `signer_id`, narrows a query to a specific
    /// (signer, key) pair across a rotation window. Unsigned actions
    /// and pre-rotation entries with no `kid` never match.
    #[serde(default)]
    pub kid: Option<String>,
    /// Only records dispatched at or after this time.
    pub from: Option<DateTime<Utc>>,
    /// Only records dispatched at or before this time.
    pub to: Option<DateTime<Utc>>,
    /// Maximum number of records to return (default 50, max 1000).
    pub limit: Option<u32>,
    /// Number of records to skip for pagination.
    ///
    /// Backends fall back to offset-based pagination only when no
    /// `cursor` is supplied. Prefer `cursor` for deep pagination — large
    /// offsets degrade linearly on every backend.
    pub offset: Option<u32>,
    /// Opaque pagination cursor returned by the previous page.
    ///
    /// When set, backends use keyset pagination from the cursor's sort
    /// key and ignore `offset`. Cursors must be round-tripped verbatim
    /// from a prior `AuditPage::next_cursor`; do not construct or
    /// modify them on the client side.
    #[serde(default)]
    pub cursor: Option<String>,
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
///
/// # Detecting the last page
///
/// Always rely on `next_cursor`: when `next_cursor.is_none()` you have
/// reached the end of the result set. `records.len() < limit` is *not*
/// a reliable end-of-stream signal because filter expressions on
/// `DynamoDB` and Elasticsearch can produce short-but-not-final pages.
///
/// # `total` semantics
///
/// `total` is intentionally a *best-effort* field. It is populated only
/// when the backend can compute it cheaply, and is `None` whenever the
/// caller paginated with a `cursor` (the count would defeat the
/// purpose of cursor pagination). Concrete behaviour by backend:
///
/// | Backend         | Offset path (`cursor` is `None`) | Cursor path  |
/// |-----------------|----------------------------------|--------------|
/// | Memory          | `Some(matches)`                  | `None`       |
/// | Postgres        | `Some(SELECT COUNT(*))`          | `None`       |
/// | `ClickHouse`    | `Some(count())`                  | `None`       |
/// | Elasticsearch   | `Some(track_total_hits)`         | `None`       |
/// | `DynamoDB`      | `None` (count was the bottleneck)| `None`       |
///
/// Treat `total` as a UI hint, not a state-of-the-world fact. Do not
/// build pagination control flow on it — use `next_cursor`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AuditPage {
    /// The records matching the query.
    pub records: Vec<AuditRecord>,
    /// Best-effort total number of matching records. See the
    /// [type-level docs](AuditPage) for when this is populated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    /// The limit used for this page.
    pub limit: u32,
    /// The offset used for this page (0 when cursor pagination is used).
    pub offset: u32,
    /// Opaque cursor pointing at the next page, or `None` when this is
    /// the last page. **This is the authoritative end-of-stream
    /// signal** — `records.len() < limit` is not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}
