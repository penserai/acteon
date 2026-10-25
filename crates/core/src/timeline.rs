//! Wire contracts for the tenant event timeline. CDC history and current events are distinct.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT_TIMELINE_TTL_SECONDS: u64 = 604_800;
pub const DEFAULT_LEDGER_TTL_SECONDS: u64 = 86_400;
pub const DEFAULT_ARCHIVE_TTL_SECONDS: u64 = 2_592_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct TimelineScope {
    pub namespace: String,
    pub tenant: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ChangeOperation {
    Create,
    Update,
    Delete,
    Snapshot,
    /// Ledger-only rejection of invalid input; never accepted from a producer.
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventBody {
    pub event_group: String,
    pub occurred_at: DateTime<Utc>,
    pub event_type: String,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default = "empty_object")]
    pub attributes: Value,
}
fn empty_object() -> Value {
    serde_json::json!({})
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimelineChange {
    pub schema_version: u16,
    pub change_id: String,
    pub source: String,
    pub event_id: String,
    pub revision: u64,
    pub operation: ChangeOperation,
    #[serde(default)]
    pub before: Option<EventBody>,
    #[serde(default)]
    pub after: Option<EventBody>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct TimelineEvent {
    pub scope: TimelineScope,
    pub source: String,
    pub event_id: String,
    pub revision: u64,
    pub first_ingested_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub event: EventBody,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttributeEquals {
    /// JSON Pointer into attributes. Array scalar elements match at the array path.
    pub path: String,
    pub value: Value,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum TextMode {
    #[default]
    All,
    Any,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextSearch {
    pub text: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub mode: TextMode,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimelineQuery {
    /// Optional CEL predicate, combined with the structured filters using AND.
    /// Supports the documented timeline subset, not arbitrary CEL evaluation.
    pub filter: Option<String>,
    pub event_group: Option<String>,
    pub event_type: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    #[serde(default)]
    pub equals: Vec<AttributeEquals>,
    #[serde(default)]
    pub exists: Vec<String>,
    pub text: Option<TextSearch>,
    pub ascending: Option<bool>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct TimelinePage {
    pub events: Vec<TimelineEvent>,
    pub next_cursor: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ChangeDisposition {
    Applied,
    Duplicate,
    Stale,
    Terminal,
    Quarantined,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ChangeReceipt {
    pub change_id: String,
    pub revision: u64,
    pub disposition: ChangeDisposition,
}
