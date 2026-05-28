//! MCP tool definitions for Acteon.
//!
//! Each tool maps to one or more operations on the Acteon gateway
//! via the HTTP client.

use acteon_ops::acteon_client::{
    AuditQuery, CreateProfileRequest, CreateQuotaRequest, CreateRecurringAction,
    CreateRetentionRequest, CreateSilenceRequest, CreateTemplateRequest, EventQuery,
    ListSilencesQuery, RecurringFilter, RenderPreviewRequest, ReplayQuery, UpdateProfileRequest,
    UpdateQuotaRequest, UpdateRecurringAction, UpdateRetentionRequest, UpdateSilenceRequest,
    UpdateTemplateRequest, VerifyHashChainRequest,
};
use acteon_ops::acteon_core::Action;
use acteon_ops::test_rules;
use rmcp::{
    ErrorData as McpError,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    schemars, tool, tool_router,
};
use serde::Deserialize;
use std::collections::HashMap;

use crate::server::ActeonMcpServer;

// ---------------------------------------------------------------------------
// JSON value extraction helpers (for types that only derive Serialize)
// ---------------------------------------------------------------------------

fn val_str(v: &serde_json::Value, key: &str) -> Result<String, McpError> {
    v.get(key)
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| mcp_err(format!("missing or invalid field '{key}'")))
}

fn val_opt_str(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
}

fn val_u64(v: &serde_json::Value, key: &str) -> Result<u64, McpError> {
    v.get(key)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| mcp_err(format!("missing or invalid field '{key}'")))
}

fn val_opt_u64(v: &serde_json::Value, key: &str) -> Option<u64> {
    v.get(key).and_then(serde_json::Value::as_u64)
}

fn val_opt_bool(v: &serde_json::Value, key: &str) -> Option<bool> {
    v.get(key).and_then(serde_json::Value::as_bool)
}

fn val_bool_or(v: &serde_json::Value, key: &str, default: bool) -> bool {
    v.get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(default)
}

fn val_json(v: &serde_json::Value, key: &str) -> Result<serde_json::Value, McpError> {
    v.get(key)
        .cloned()
        .ok_or_else(|| mcp_err(format!("missing field '{key}'")))
}

fn val_opt_json(v: &serde_json::Value, key: &str) -> Option<serde_json::Value> {
    v.get(key).cloned()
}

fn val_hashmap(v: &serde_json::Value, key: &str) -> HashMap<String, String> {
    v.get(key)
        .and_then(serde_json::Value::as_object)
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn val_opt_hashmap_str(v: &serde_json::Value, key: &str) -> Option<HashMap<String, String>> {
    v.get(key)
        .and_then(serde_json::Value::as_object)
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
}

fn val_opt_hashmap_json(
    v: &serde_json::Value,
    key: &str,
) -> Option<HashMap<String, serde_json::Value>> {
    v.get(key)
        .and_then(serde_json::Value::as_object)
        .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}

fn val_hashmap_json(
    v: &serde_json::Value,
    key: &str,
) -> Result<HashMap<String, serde_json::Value>, McpError> {
    v.get(key)
        .and_then(serde_json::Value::as_object)
        .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .ok_or_else(|| mcp_err(format!("missing or invalid field '{key}'")))
}

// ---------------------------------------------------------------------------
// Struct builders from serde_json::Value (for types without Deserialize)
// ---------------------------------------------------------------------------

fn build_create_recurring(v: &serde_json::Value) -> Result<CreateRecurringAction, McpError> {
    Ok(CreateRecurringAction {
        namespace: val_str(v, "namespace")?,
        tenant: val_str(v, "tenant")?,
        provider: val_str(v, "provider")?,
        action_type: val_str(v, "action_type")?,
        payload: val_json(v, "payload")?,
        cron_expression: val_str(v, "cron_expression")?,
        name: val_opt_str(v, "name"),
        timezone: val_opt_str(v, "timezone"),
        metadata: val_hashmap(v, "metadata"),
        end_date: val_opt_str(v, "end_date"),
        description: val_opt_str(v, "description"),
        dedup_key: val_opt_str(v, "dedup_key"),
        labels: val_hashmap(v, "labels"),
    })
}

fn build_update_recurring(v: &serde_json::Value) -> Result<UpdateRecurringAction, McpError> {
    Ok(UpdateRecurringAction {
        namespace: val_str(v, "namespace")?,
        tenant: val_str(v, "tenant")?,
        name: val_opt_str(v, "name"),
        payload: val_opt_json(v, "payload"),
        metadata: v.get("metadata").and_then(|v| v.as_object()).map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        }),
        cron_expression: val_opt_str(v, "cron_expression"),
        timezone: val_opt_str(v, "timezone"),
        end_date: val_opt_str(v, "end_date"),
        description: val_opt_str(v, "description"),
        dedup_key: val_opt_str(v, "dedup_key"),
        labels: val_opt_hashmap_str(v, "labels"),
    })
}

fn build_create_quota(v: &serde_json::Value) -> Result<CreateQuotaRequest, McpError> {
    Ok(CreateQuotaRequest {
        namespace: val_str(v, "namespace")?,
        tenant: val_str(v, "tenant")?,
        provider: val_opt_str(v, "provider"),
        principal: val_opt_str(v, "principal"),
        per_principal: val_bool_or(v, "per_principal", false),
        max_actions: val_u64(v, "max_actions")?,
        window: val_str(v, "window")?,
        overage_behavior: val_str(v, "overage_behavior")?,
        description: val_opt_str(v, "description"),
        labels: val_opt_hashmap_str(v, "labels"),
    })
}

fn build_update_quota(v: &serde_json::Value) -> Result<UpdateQuotaRequest, McpError> {
    Ok(UpdateQuotaRequest {
        namespace: val_str(v, "namespace")?,
        tenant: val_str(v, "tenant")?,
        max_actions: val_opt_u64(v, "max_actions"),
        window: val_opt_str(v, "window"),
        overage_behavior: val_opt_str(v, "overage_behavior"),
        description: val_opt_str(v, "description"),
        enabled: val_opt_bool(v, "enabled"),
        per_principal: val_opt_bool(v, "per_principal"),
        labels: val_opt_hashmap_str(v, "labels"),
    })
}

fn build_create_retention(v: &serde_json::Value) -> Result<CreateRetentionRequest, McpError> {
    Ok(CreateRetentionRequest {
        namespace: val_str(v, "namespace")?,
        tenant: val_str(v, "tenant")?,
        audit_ttl_seconds: val_opt_u64(v, "audit_ttl_seconds"),
        state_ttl_seconds: val_opt_u64(v, "state_ttl_seconds"),
        event_ttl_seconds: val_opt_u64(v, "event_ttl_seconds"),
        compliance_hold: val_bool_or(v, "compliance_hold", false),
        description: val_opt_str(v, "description"),
        labels: val_opt_hashmap_str(v, "labels"),
    })
}

fn build_update_retention(v: &serde_json::Value) -> UpdateRetentionRequest {
    UpdateRetentionRequest {
        enabled: val_opt_bool(v, "enabled"),
        audit_ttl_seconds: val_opt_u64(v, "audit_ttl_seconds"),
        state_ttl_seconds: val_opt_u64(v, "state_ttl_seconds"),
        event_ttl_seconds: val_opt_u64(v, "event_ttl_seconds"),
        compliance_hold: val_opt_bool(v, "compliance_hold"),
        description: val_opt_str(v, "description"),
        labels: val_opt_hashmap_str(v, "labels"),
    }
}

fn build_create_silence(v: &serde_json::Value) -> Result<CreateSilenceRequest, McpError> {
    // Parse matchers array: [{"name": "severity", "value": "warning", "op": "equal"}, ...]
    let matchers_val = v
        .get("matchers")
        .and_then(|m| m.as_array())
        .ok_or_else(|| mcp_err("'matchers' array is required"))?;
    let mut matchers = Vec::with_capacity(matchers_val.len());
    for m in matchers_val {
        let name = m
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| mcp_err("matcher 'name' is required"))?;
        let value = m.get("value").and_then(|n| n.as_str()).unwrap_or("");
        let op = match m.get("op").and_then(|o| o.as_str()).unwrap_or("equal") {
            "equal" => acteon_core::MatchOp::Equal,
            "not_equal" => acteon_core::MatchOp::NotEqual,
            "regex" => acteon_core::MatchOp::Regex,
            "not_regex" => acteon_core::MatchOp::NotRegex,
            other => return Err(mcp_err(format!("unknown match op: {other}"))),
        };
        let matcher = acteon_core::SilenceMatcher::new(name, value, op).map_err(mcp_err)?;
        matchers.push(matcher);
    }

    // Parse optional datetime fields.
    let starts_at = val_opt_str(v, "starts_at")
        .map(|s| s.parse::<chrono::DateTime<chrono::Utc>>())
        .transpose()
        .map_err(|e| mcp_err(format!("invalid starts_at: {e}")))?;
    let ends_at = val_opt_str(v, "ends_at")
        .map(|s| s.parse::<chrono::DateTime<chrono::Utc>>())
        .transpose()
        .map_err(|e| mcp_err(format!("invalid ends_at: {e}")))?;

    Ok(CreateSilenceRequest {
        namespace: val_str(v, "namespace")?,
        tenant: val_str(v, "tenant")?,
        matchers,
        comment: val_str(v, "comment")?,
        starts_at,
        ends_at,
        duration_seconds: val_opt_u64(v, "duration_seconds"),
    })
}

fn build_update_silence(v: &serde_json::Value) -> Result<UpdateSilenceRequest, McpError> {
    let ends_at = val_opt_str(v, "ends_at")
        .map(|s| s.parse::<chrono::DateTime<chrono::Utc>>())
        .transpose()
        .map_err(|e| mcp_err(format!("invalid ends_at: {e}")))?;
    Ok(UpdateSilenceRequest {
        ends_at,
        comment: val_opt_str(v, "comment"),
    })
}

fn build_create_template(v: &serde_json::Value) -> Result<CreateTemplateRequest, McpError> {
    Ok(CreateTemplateRequest {
        name: val_str(v, "name")?,
        namespace: val_str(v, "namespace")?,
        tenant: val_str(v, "tenant")?,
        content: val_str(v, "content")?,
        description: val_opt_str(v, "description"),
        labels: val_opt_hashmap_str(v, "labels"),
    })
}

fn build_update_template(v: &serde_json::Value) -> UpdateTemplateRequest {
    UpdateTemplateRequest {
        content: val_opt_str(v, "content"),
        description: val_opt_str(v, "description"),
        labels: val_opt_hashmap_str(v, "labels"),
    }
}

fn build_create_profile(v: &serde_json::Value) -> Result<CreateProfileRequest, McpError> {
    Ok(CreateProfileRequest {
        name: val_str(v, "name")?,
        namespace: val_str(v, "namespace")?,
        tenant: val_str(v, "tenant")?,
        fields: val_hashmap_json(v, "fields")?,
        description: val_opt_str(v, "description"),
        labels: val_opt_hashmap_str(v, "labels"),
    })
}

fn build_update_profile(v: &serde_json::Value) -> UpdateProfileRequest {
    UpdateProfileRequest {
        fields: val_opt_hashmap_json(v, "fields"),
        description: val_opt_str(v, "description"),
        labels: val_opt_hashmap_str(v, "labels"),
    }
}

fn build_render_preview(v: &serde_json::Value) -> Result<RenderPreviewRequest, McpError> {
    Ok(RenderPreviewRequest {
        profile: val_str(v, "profile")?,
        namespace: val_str(v, "namespace")?,
        tenant: val_str(v, "tenant")?,
        payload: val_json(v, "payload")?,
    })
}

// ---------------------------------------------------------------------------
// Parameter types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DispatchParams {
    /// Namespace for the action (e.g. "notifications").
    pub namespace: String,
    /// Tenant identifier.
    pub tenant: String,
    /// Target provider name (e.g. "slack", "email", "webhook").
    pub provider: String,
    /// Action type discriminator (e.g. `send_alert`, `create_ticket`).
    pub action_type: String,
    /// JSON payload for the provider.
    pub payload: serde_json::Value,
    /// Optional metadata labels.
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
    /// When true, evaluate rules without executing the action.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BatchDispatchParams {
    /// Array of action JSON objects to dispatch.
    pub actions: Vec<serde_json::Value>,
    /// When true, evaluate rules without executing the actions.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct QueryAuditParams {
    /// Filter by tenant.
    #[serde(default)]
    pub tenant: Option<String>,
    /// Filter by namespace.
    #[serde(default)]
    pub namespace: Option<String>,
    /// Filter by provider.
    #[serde(default)]
    pub provider: Option<String>,
    /// Filter by action type.
    #[serde(default)]
    pub action_type: Option<String>,
    /// Filter by outcome (e.g. "executed", "suppressed", "failed").
    #[serde(default)]
    pub outcome: Option<String>,
    /// Maximum number of records (default 20).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Opaque pagination cursor returned by the previous page.
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListRulesParams {
    // Currently no parameters — lists all rules.
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReloadRulesParams {
    // No parameters needed.
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EvaluateRulesParams {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Provider.
    pub provider: String,
    /// Action type.
    pub action_type: String,
    /// JSON payload to evaluate against rules.
    pub payload: serde_json::Value,
    /// Include disabled rules in the trace.
    #[serde(default)]
    pub include_disabled: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ManageEventParams {
    /// Event fingerprint.
    pub fingerprint: String,
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Target state: "acknowledged", "resolved", "investigating", etc.
    pub action: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListEventsParams {
    /// Namespace (required).
    pub namespace: String,
    /// Tenant (required).
    pub tenant: String,
    /// Filter by state (e.g. "open", "acknowledged").
    #[serde(default)]
    pub status: Option<String>,
    /// Maximum number of events to return.
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListChainsParams {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Optional status filter (e.g. "running", "completed").
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetRuleEnabledParams {
    /// Rule name.
    pub rule_name: String,
    /// Set to true to enable, false to disable.
    pub enabled: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListRecurringParams {
    /// Namespace (required).
    pub namespace: String,
    /// Tenant (required).
    pub tenant: String,
    /// Optional status filter: "active" or "paused".
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListGroupsParams {
    // Currently no filters exposed by client list_groups.
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListQuotasParams {
    /// Filter by namespace.
    #[serde(default)]
    pub namespace: Option<String>,
    /// Filter by tenant.
    #[serde(default)]
    pub tenant: Option<String>,
    /// Filter by provider scope: pass `"generic"` to match only
    /// policies with no provider scope, or a provider name (e.g.
    /// `"slack"`) to match only per-provider policies for that
    /// provider.
    #[serde(default)]
    pub provider: Option<String>,
    /// Filter by principal (caller) scope: pass `"any"` to match
    /// only policies with no principal scope, or a caller id to
    /// match only policies scoped to that caller.
    #[serde(default)]
    pub principal: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListRetentionParams {
    /// Filter by namespace.
    #[serde(default)]
    pub namespace: Option<String>,
    /// Filter by tenant.
    #[serde(default)]
    pub tenant: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListCircuitBreakersParams {
    // No params.
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct QueryAnalyticsParams {
    /// Metric to compute: `volume`, `outcome_breakdown`, `top_action_types`, `latency`, `error_rate`.
    pub metric: String,
    /// Filter by namespace.
    #[serde(default)]
    pub namespace: Option<String>,
    /// Filter by tenant.
    #[serde(default)]
    pub tenant: Option<String>,
    /// Filter by provider.
    #[serde(default)]
    pub provider: Option<String>,
    /// Filter by action type.
    #[serde(default)]
    pub action_type: Option<String>,
    /// Filter by outcome.
    #[serde(default)]
    pub outcome: Option<String>,
    /// Time interval: "hourly", "daily", "weekly", "monthly" (default "daily").
    #[serde(default)]
    pub interval: Option<String>,
    /// Start of time range (RFC 3339).
    #[serde(default)]
    pub from: Option<String>,
    /// End of time range (RFC 3339).
    #[serde(default)]
    pub to: Option<String>,
    /// Dimension to group by (e.g. `provider`, `action_type`, `outcome`).
    #[serde(default)]
    pub group_by: Option<String>,
    /// Number of top entries for `top_action_types` (default 10).
    #[serde(default)]
    pub top_n: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ManageCircuitBreakerParams {
    /// Provider name.
    pub provider: String,
    /// Action: "trip" (open) or "reset" (close).
    pub action: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TestRulesParams {
    /// Path to a YAML test-fixture file.
    pub fixtures_path: String,
    /// Only run tests whose name contains this substring.
    #[serde(default)]
    pub filter: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetAuditRecordParams {
    /// The action ID of the audit record to retrieve.
    pub action_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReplayActionParams {
    /// The action ID to replay from the audit trail.
    pub action_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReplayAuditParams {
    /// Filter by namespace.
    #[serde(default)]
    pub namespace: Option<String>,
    /// Filter by tenant.
    #[serde(default)]
    pub tenant: Option<String>,
    /// Filter by provider.
    #[serde(default)]
    pub provider: Option<String>,
    /// Filter by action type.
    #[serde(default)]
    pub action_type: Option<String>,
    /// Maximum number of actions to replay.
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetChainParams {
    /// Chain instance ID.
    pub chain_id: String,
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetChainHistoryParams {
    /// Chain instance ID.
    pub chain_id: String,
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CancelChainParams {
    /// Chain instance ID.
    pub chain_id: String,
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Optional cancellation reason.
    #[serde(default)]
    pub reason: Option<String>,
    /// Optional identifier of who cancelled.
    #[serde(default)]
    pub cancelled_by: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetChainDagParams {
    /// Chain instance ID (provide this OR name, not both).
    #[serde(default)]
    pub chain_id: Option<String>,
    /// Chain definition name (provide this OR `chain_id`, not both).
    #[serde(default)]
    pub name: Option<String>,
    /// Namespace (required when using `chain_id`).
    #[serde(default)]
    pub namespace: Option<String>,
    /// Tenant (required when using `chain_id`).
    #[serde(default)]
    pub tenant: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ManageChainDefinitionsParams {
    /// Action to perform: "list", "get", "put", "delete".
    pub action: String,
    /// Chain definition name (required for get, put, delete).
    #[serde(default)]
    pub name: Option<String>,
    /// Chain configuration JSON (required for put).
    #[serde(default)]
    pub config: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ManageRecurringParams {
    /// Action to perform: "list", "get", "create", "update", "delete", "pause", "resume".
    pub action: String,
    /// Recurring action ID (required for get, update, delete, pause, resume).
    #[serde(default)]
    pub id: Option<String>,
    /// Namespace (required for list, get, delete, pause, resume).
    #[serde(default)]
    pub namespace: Option<String>,
    /// Tenant (required for list, get, delete, pause, resume).
    #[serde(default)]
    pub tenant: Option<String>,
    /// Status filter for list (e.g. "active", "paused").
    #[serde(default)]
    pub status: Option<String>,
    /// JSON data for create or update operations.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ManageQuotasParams {
    /// Action to perform: "list", "get", "create", "update", "delete", "usage".
    pub action: String,
    /// Quota ID (required for get, update, delete, usage).
    #[serde(default)]
    pub id: Option<String>,
    /// Namespace filter for list; required for delete.
    #[serde(default)]
    pub namespace: Option<String>,
    /// Tenant filter for list; required for delete.
    #[serde(default)]
    pub tenant: Option<String>,
    /// Provider scope filter for list: `"generic"` matches
    /// policies without a provider scope; any other value matches
    /// only per-provider policies for that provider.
    #[serde(default)]
    pub provider: Option<String>,
    /// Principal (caller) scope filter for list: `"any"` matches
    /// policies without a principal scope; any other value matches
    /// only policies scoped to that caller.
    #[serde(default)]
    pub principal: Option<String>,
    /// JSON data for create or update operations.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ManageRetentionParams {
    /// Action to perform: "list", "get", "create", "update", "delete".
    pub action: String,
    /// Retention policy ID (required for get, update, delete).
    #[serde(default)]
    pub id: Option<String>,
    /// Namespace filter for list.
    #[serde(default)]
    pub namespace: Option<String>,
    /// Tenant filter for list.
    #[serde(default)]
    pub tenant: Option<String>,
    /// JSON data for create or update operations.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ManageTemplatesParams {
    /// Action to perform: "list", "get", "create", "update", "delete".
    pub action: String,
    /// Template ID (required for get, update, delete).
    #[serde(default)]
    pub id: Option<String>,
    /// Namespace filter for list.
    #[serde(default)]
    pub namespace: Option<String>,
    /// Tenant filter for list.
    #[serde(default)]
    pub tenant: Option<String>,
    /// JSON data for create or update operations.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ManageTemplateProfilesParams {
    /// Action to perform: "list", "get", "create", "update", "delete", "render".
    pub action: String,
    /// Profile ID (required for get, update, delete).
    #[serde(default)]
    pub id: Option<String>,
    /// Namespace filter for list.
    #[serde(default)]
    pub namespace: Option<String>,
    /// Tenant filter for list.
    #[serde(default)]
    pub tenant: Option<String>,
    /// JSON data for create, update, or render operations.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ManagePluginsParams {
    /// Action to perform: "list", "delete".
    pub action: String,
    /// Plugin name (required for delete).
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ManageGroupsParams {
    /// Action to perform: "list", "get", "flush".
    pub action: String,
    /// Group key (required for get, flush).
    #[serde(default)]
    pub key: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ManageSilencesParams {
    /// Action to perform: "list", "get", "create", "update", "delete".
    pub action: String,
    /// Silence ID (required for get, update, delete).
    #[serde(default)]
    pub id: Option<String>,
    /// Namespace filter for list.
    #[serde(default)]
    pub namespace: Option<String>,
    /// Tenant filter for list.
    #[serde(default)]
    pub tenant: Option<String>,
    /// Include expired silences in list (defaults to false).
    #[serde(default)]
    pub include_expired: Option<bool>,
    /// JSON data for create or update operations.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ManageDlqParams {
    /// Action to perform: "stats", "drain".
    pub action: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ComplianceStatusParams {
    /// Action to perform: "status" (default) or "verify".
    #[serde(default = "default_status_action")]
    pub action: String,
    /// JSON data for verify action (`VerifyHashChainRequest`).
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

fn default_status_action() -> String {
    "status".to_string()
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProviderHealthParams {
    // No params.
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListApprovalsParams {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
}

// ---------------------------------------------------------------------------
// Agentic bus parameter types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ManageBusTopicsParams {
    /// Action: "list", "create", "delete", "publish".
    pub action: String,
    /// Namespace filter (list) or component of new topic (create).
    #[serde(default)]
    pub namespace: Option<String>,
    /// Tenant filter (list) or component of new topic (create).
    #[serde(default)]
    pub tenant: Option<String>,
    /// Full Kafka topic name (`namespace.tenant.name`) — required for delete.
    #[serde(default)]
    pub kafka_name: Option<String>,
    /// JSON body — `CreateBusTopic` for create, `PublishBusMessage` for publish.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ManageBusSubscriptionsParams {
    /// Action: "list", "create", "delete", "lag", "ack".
    pub action: String,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub tenant: Option<String>,
    /// Subscription id — required for delete, lag, ack.
    #[serde(default)]
    pub id: Option<String>,
    /// Topic filter for list.
    #[serde(default)]
    pub topic: Option<String>,
    /// Partition for ack.
    #[serde(default)]
    pub partition: Option<i32>,
    /// Offset for ack.
    #[serde(default)]
    pub offset: Option<i64>,
    /// JSON body — `CreateSubscription` for create.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ManageBusSchemasParams {
    /// Action: "list", "versions", "get", "register", "delete", "bind", "unbind".
    pub action: String,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub tenant: Option<String>,
    /// Schema subject — required for versions, get, delete, bind.
    #[serde(default)]
    pub subject: Option<String>,
    /// Schema version (numeric for delete/bind, "latest" or string for get).
    #[serde(default)]
    pub version: Option<serde_json::Value>,
    /// Topic logical name — required for bind, unbind.
    #[serde(default)]
    pub topic: Option<String>,
    /// `latest_only` filter for list.
    #[serde(default)]
    pub latest_only: Option<bool>,
    /// JSON body — `RegisterBusSchema` for register.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ManageBusAgentsParams {
    /// Action: "list", "get", "register", "update", "delete", "heartbeat", "send".
    pub action: String,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub tenant: Option<String>,
    /// Agent id — required for get, update, delete, heartbeat, send.
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Capability filter for list.
    #[serde(default)]
    pub capability: Option<String>,
    /// Status filter for list (`online`, `idle`, `offline`).
    #[serde(default)]
    pub status: Option<String>,
    /// JSON body — `RegisterBusAgent` for register, `UpdateBusAgent` for update,
    /// `SendToBusAgent` for send.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ManageBusConversationsParams {
    /// Action: "list", "get", "create", "update", "delete", "transition", "append", "replay".
    pub action: String,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub tenant: Option<String>,
    /// Conversation id — required for get, update, delete, transition, append, replay.
    #[serde(default)]
    pub conversation_id: Option<String>,
    /// State filter for list (`active`, `resolved`, `archived`).
    #[serde(default)]
    pub state: Option<String>,
    /// Participant filter for list.
    #[serde(default)]
    pub participant: Option<String>,
    /// Transition for transition action (`resolve`, `reopen`, `archive`).
    #[serde(default)]
    pub transition: Option<String>,
    /// JSON body — `RegisterBusConversation` for create, `UpdateBusConversation` for
    /// update, `AppendBusConversationMessage` for append, `ReplayBusConversationParams`
    /// for replay.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BusToolCallParams {
    /// Action: `post`, `post_result`, `lookup`.
    pub action: String,
    pub namespace: String,
    pub tenant: String,
    /// Conversation id — required for `post`, `post_result`, `lookup`.
    #[serde(default)]
    pub conversation_id: Option<String>,
    /// Call id — required for `lookup`; appears inside `data` for `post` and `post_result`.
    #[serde(default)]
    pub call_id: Option<String>,
    /// Resume cursor from a prior post receipt — strongly recommended for `lookup`.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Lookup deadline (ms). Default 5000, max 30000.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// JSON body — `PostBusToolCall` for `post`, `PostBusToolResult` for `post_result`.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BusStreamParams {
    /// Action: `chunk`, `end`, `consume_url`.
    pub action: String,
    pub namespace: String,
    pub tenant: String,
    pub conversation_id: String,
    /// Stream id — required for `consume_url`; appears inside `data` for `chunk` and `end`.
    #[serde(default)]
    pub stream_id: Option<String>,
    /// JSON body — `PostBusStreamChunk` for chunk, `PostBusStreamEnd` for end.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ManageBusApprovalsParams {
    /// Action: "list", "get", "approve", "reject".
    pub action: String,
    pub namespace: String,
    pub tenant: String,
    /// Approval id — required for get, approve, reject.
    #[serde(default)]
    pub approval_id: Option<String>,
    /// Status filter for list (`pending`, `approved`, `rejected`, `expired`).
    #[serde(default)]
    pub status: Option<String>,
    /// Conversation id filter for list.
    #[serde(default)]
    pub conversation_id: Option<String>,
    /// JSON body — `BusApprovalDecisionRequest` for approve, reject.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

fn mcp_err(msg: impl std::fmt::Display) -> McpError {
    McpError::internal_error(msg.to_string(), None)
}

#[tool_router]
impl ActeonMcpServer {
    /// Build the tool router. Exposed as `pub(crate)` so `server.rs` can call it.
    pub(crate) fn create_tool_router() -> rmcp::handler::server::router::tool::ToolRouter<Self> {
        Self::tool_router()
    }

    /// Send a new action through the Acteon gateway. Supports dry-run mode
    /// to preview rule evaluation without side effects.
    #[tool(
        description = "Dispatch an action through Acteon (send notifications, trigger workflows). Set dry_run=true to preview without executing."
    )]
    async fn dispatch(
        &self,
        Parameters(p): Parameters<DispatchParams>,
    ) -> Result<CallToolResult, McpError> {
        let options = acteon_ops::DispatchOptions {
            metadata: p.metadata,
            dry_run: p.dry_run,
        };

        match self
            .ops
            .dispatch(
                p.namespace,
                p.tenant,
                p.provider,
                p.action_type,
                p.payload,
                options,
            )
            .await
        {
            Ok(o) => {
                let json = serde_json::to_string_pretty(&o).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Dispatch a batch of actions through the gateway.
    #[tool(
        description = "Dispatch multiple actions in a single batch. Each element in 'actions' is a full action JSON object. Set dry_run=true to preview."
    )]
    async fn batch_dispatch(
        &self,
        Parameters(p): Parameters<BatchDispatchParams>,
    ) -> Result<CallToolResult, McpError> {
        let actions: Vec<Action> = p
            .actions
            .into_iter()
            .map(serde_json::from_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| mcp_err(format!("failed to parse actions: {e}")))?;

        match self.ops.dispatch_batch(&actions, p.dry_run).await {
            Ok(resp) => {
                let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Search the audit trail for historical events. Returns recent dispatch
    /// records filtered by tenant, provider, action type, or outcome.
    #[tool(
        description = "Query the audit trail for historical dispatch records. Filter by tenant, provider, action_type, or outcome."
    )]
    async fn query_audit(
        &self,
        Parameters(p): Parameters<QueryAuditParams>,
    ) -> Result<CallToolResult, McpError> {
        let query = AuditQuery {
            tenant: p.tenant,
            namespace: p.namespace,
            provider: p.provider,
            action_type: p.action_type,
            outcome: p.outcome,
            limit: Some(p.limit.unwrap_or(20)),
            offset: None,
            cursor: p.cursor,
        };

        match self.ops.query_audit(query).await {
            Ok(page) => {
                let json = serde_json::to_string_pretty(&page).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Get a single audit record by action ID.
    #[tool(description = "Retrieve a single audit record by its action ID.")]
    async fn get_audit_record(
        &self,
        Parameters(p): Parameters<GetAuditRecordParams>,
    ) -> Result<CallToolResult, McpError> {
        match self.ops.get_audit_record(&p.action_id).await {
            Ok(resp) => {
                let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Replay a single action from the audit trail.
    #[tool(description = "Replay a previously dispatched action by its action ID.")]
    async fn replay_action(
        &self,
        Parameters(p): Parameters<ReplayActionParams>,
    ) -> Result<CallToolResult, McpError> {
        match self.ops.replay_action(&p.action_id).await {
            Ok(resp) => {
                let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Replay multiple actions matching a query filter.
    #[tool(
        description = "Replay multiple actions from the audit trail matching optional filters (namespace, tenant, provider, action_type, limit)."
    )]
    async fn replay_audit(
        &self,
        Parameters(p): Parameters<ReplayAuditParams>,
    ) -> Result<CallToolResult, McpError> {
        let query = ReplayQuery {
            namespace: p.namespace,
            tenant: p.tenant,
            provider: p.provider,
            action_type: p.action_type,
            outcome: None,
            verdict: None,
            matched_rule: None,
            from: None,
            to: None,
            limit: p.limit,
        };

        match self.ops.replay_audit(query).await {
            Ok(resp) => {
                let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// List all loaded routing rules. Shows rule name, priority, enabled
    /// status, and description.
    #[tool(description = "List all active routing and filtering rules loaded in the gateway.")]
    async fn list_rules(
        &self,
        Parameters(_p): Parameters<ListRulesParams>,
    ) -> Result<CallToolResult, McpError> {
        match self.ops.list_rules().await {
            Ok(rules) => {
                let json = serde_json::to_string_pretty(&rules).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Reload rules from the configured rule source.
    #[tool(description = "Reload all routing rules from the YAML directory or configured source.")]
    async fn reload_rules(
        &self,
        Parameters(_p): Parameters<ReloadRulesParams>,
    ) -> Result<CallToolResult, McpError> {
        match self.ops.reload_rules().await {
            Ok(resp) => {
                let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Run a test action through the rule engine without side effects.
    /// Returns a full per-rule evaluation trace showing which rules
    /// matched, were skipped, or errored.
    #[tool(
        description = "Evaluate rules against a test action (rule playground). Returns a detailed per-rule trace without executing anything."
    )]
    async fn evaluate_rules(
        &self,
        Parameters(p): Parameters<EvaluateRulesParams>,
    ) -> Result<CallToolResult, McpError> {
        match self
            .ops
            .evaluate_rules(
                p.namespace,
                p.tenant,
                p.provider,
                p.action_type,
                p.payload,
                p.include_disabled,
            )
            .await
        {
            Ok(trace) => {
                let json = serde_json::to_string_pretty(&trace).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Transition a stateful event to a new state (e.g. acknowledge,
    /// resolve, investigate).
    #[tool(
        description = "Manage a stateful event: transition its state (e.g. 'acknowledged', 'resolved', 'investigating')."
    )]
    async fn manage_event(
        &self,
        Parameters(p): Parameters<ManageEventParams>,
    ) -> Result<CallToolResult, McpError> {
        match self
            .ops
            .transition_event(p.fingerprint, p.action, p.namespace, p.tenant)
            .await
        {
            Ok(resp) => {
                let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// List stateful events for a given namespace and tenant, optionally
    /// filtered by state.
    #[tool(
        description = "List stateful events (open incidents, acknowledged alerts) for a namespace and tenant."
    )]
    async fn list_events(
        &self,
        Parameters(p): Parameters<ListEventsParams>,
    ) -> Result<CallToolResult, McpError> {
        let query = EventQuery {
            namespace: p.namespace,
            tenant: p.tenant,
            status: p.status,
            limit: p.limit,
        };

        match self.ops.list_events(query).await {
            Ok(resp) => {
                let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// List action chains (multi-step workflows) for a tenant.
    #[tool(
        description = "List action chains (multi-step workflows) for a namespace and tenant. Optionally filter by status."
    )]
    async fn list_chains(
        &self,
        Parameters(p): Parameters<ListChainsParams>,
    ) -> Result<CallToolResult, McpError> {
        match self.ops.list_chains(p.namespace, p.tenant, p.status).await {
            Ok(resp) => {
                let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Get the full details of a chain instance.
    #[tool(description = "Get the full details of a chain instance by ID, namespace, and tenant.")]
    async fn get_chain(
        &self,
        Parameters(p): Parameters<GetChainParams>,
    ) -> Result<CallToolResult, McpError> {
        match self
            .ops
            .get_chain(&p.chain_id, &p.namespace, &p.tenant)
            .await
        {
            Ok(resp) => {
                let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Get per-step execution history with retry attempts for a chain.
    #[tool(
        description = "Get the execution history of a chain, showing all retry attempts per step. Useful for debugging failed steps and understanding retry behavior."
    )]
    async fn get_chain_history(
        &self,
        Parameters(p): Parameters<GetChainHistoryParams>,
    ) -> Result<CallToolResult, McpError> {
        match self
            .ops
            .get_chain_history(&p.chain_id, &p.namespace, &p.tenant)
            .await
        {
            Ok(resp) => {
                let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Cancel a running chain.
    #[tool(description = "Cancel a running chain by ID with an optional reason.")]
    async fn cancel_chain(
        &self,
        Parameters(p): Parameters<CancelChainParams>,
    ) -> Result<CallToolResult, McpError> {
        match self
            .ops
            .cancel_chain(
                &p.chain_id,
                &p.namespace,
                &p.tenant,
                p.reason.as_deref(),
                p.cancelled_by.as_deref(),
            )
            .await
        {
            Ok(resp) => {
                let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Get the DAG visualization for a chain instance or definition.
    #[tool(
        description = "Get the DAG (directed acyclic graph) for a chain. Provide chain_id+namespace+tenant for an instance, or name for a definition."
    )]
    async fn get_chain_dag(
        &self,
        Parameters(p): Parameters<GetChainDagParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = if let Some(ref chain_id) = p.chain_id {
            let ns = p
                .namespace
                .as_deref()
                .ok_or_else(|| mcp_err("'namespace' is required when using chain_id"))?;
            let tenant = p
                .tenant
                .as_deref()
                .ok_or_else(|| mcp_err("'tenant' is required when using chain_id"))?;
            self.ops.get_chain_dag(chain_id, ns, tenant).await
        } else if let Some(ref name) = p.name {
            self.ops.get_chain_definition_dag(name).await
        } else {
            return Ok(CallToolResult::error(vec![Content::text(
                "Either 'chain_id' or 'name' must be provided.",
            )]));
        };

        match result {
            Ok(resp) => {
                let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Manage chain definitions (list, get, put, delete).
    #[tool(
        description = "Manage chain definitions. Actions: list, get, put (create/update), delete. Provide 'name' for get/put/delete, 'config' JSON for put."
    )]
    async fn manage_chain_definitions(
        &self,
        Parameters(p): Parameters<ManageChainDefinitionsParams>,
    ) -> Result<CallToolResult, McpError> {
        match p.action.as_str() {
            "list" => match self.ops.list_chain_definitions().await {
                Ok(resp) => {
                    let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                    Ok(CallToolResult::success(vec![Content::text(json)]))
                }
                Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
            },
            "get" => {
                let name = p
                    .name
                    .as_deref()
                    .ok_or_else(|| mcp_err("'name' is required for get"))?;
                match self.ops.get_chain_definition(name).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "put" => {
                let name = p
                    .name
                    .as_deref()
                    .ok_or_else(|| mcp_err("'name' is required for put"))?;
                let config = p
                    .config
                    .as_ref()
                    .ok_or_else(|| mcp_err("'config' is required for put"))?;
                match self.ops.put_chain_definition(name, config).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "delete" => {
                let name = p
                    .name
                    .as_deref()
                    .ok_or_else(|| mcp_err("'name' is required for delete"))?;
                match self.ops.delete_chain_definition(name).await {
                    Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                        "Chain definition '{name}' deleted."
                    ))])),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            other => Ok(CallToolResult::error(vec![Content::text(format!(
                "Unknown action: {other}. Valid: list, get, put, delete"
            ))])),
        }
    }

    /// Enable or disable a routing rule by name.
    #[tool(description = "Enable or disable a routing rule by name.")]
    async fn set_rule_enabled(
        &self,
        Parameters(p): Parameters<SetRuleEnabledParams>,
    ) -> Result<CallToolResult, McpError> {
        match self
            .ops
            .set_rule_enabled(p.rule_name.clone(), p.enabled)
            .await
        {
            Ok(()) => {
                let state = if p.enabled { "enabled" } else { "disabled" };
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Rule '{}' is now {state}.",
                    p.rule_name
                ))]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Check gateway health and service status.
    #[tool(description = "Check if the Acteon gateway is healthy and responding.")]
    async fn check_health(&self) -> Result<CallToolResult, McpError> {
        match self.ops.health().await {
            Ok(true) => Ok(CallToolResult::success(vec![Content::text(
                "Acteon gateway is healthy.",
            )])),
            Ok(false) => Ok(CallToolResult::error(vec![Content::text(
                "Acteon gateway returned unhealthy status.",
            )])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to reach gateway: {e}"
            ))])),
        }
    }

    /// List recurring actions for a tenant.
    #[tool(description = "List recurring actions (scheduled jobs) for a namespace and tenant.")]
    async fn list_recurring_actions(
        &self,
        Parameters(p): Parameters<ListRecurringParams>,
    ) -> Result<CallToolResult, McpError> {
        let filter = RecurringFilter {
            namespace: p.namespace,
            tenant: p.tenant,
            status: p.status,
            ..Default::default()
        };

        match self.ops.list_recurring(&filter).await {
            Ok(resp) => {
                let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// List active event groups.
    #[tool(description = "List active event groups (batched notifications).")]
    async fn list_groups(
        &self,
        Parameters(_p): Parameters<ListGroupsParams>,
    ) -> Result<CallToolResult, McpError> {
        match self.ops.list_groups().await {
            Ok(resp) => {
                let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// List tenant quotas.
    #[tool(description = "List action quotas for tenants.")]
    async fn list_quotas(
        &self,
        Parameters(p): Parameters<ListQuotasParams>,
    ) -> Result<CallToolResult, McpError> {
        match self
            .ops
            .list_quotas(
                p.namespace.as_deref(),
                p.tenant.as_deref(),
                p.provider.as_deref(),
                p.principal.as_deref(),
            )
            .await
        {
            Ok(resp) => {
                let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// List data retention policies.
    #[tool(description = "List data retention policies for tenants.")]
    async fn list_retention(
        &self,
        Parameters(p): Parameters<ListRetentionParams>,
    ) -> Result<CallToolResult, McpError> {
        match self.ops.list_retention(p.namespace, p.tenant).await {
            Ok(resp) => {
                let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// List circuit breaker statuses.
    #[tool(description = "List all circuit breakers and their current status (Open/Closed).")]
    async fn list_circuit_breakers(
        &self,
        Parameters(_p): Parameters<ListCircuitBreakersParams>,
    ) -> Result<CallToolResult, McpError> {
        match self.ops.list_circuit_breakers().await {
            Ok(resp) => {
                let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Run a test suite of rule fixtures against the gateway and return
    /// pass/fail results for each test case.
    #[tool(
        description = "Run a test suite of rule fixtures against the gateway. Returns pass/fail results for each test case."
    )]
    async fn test_rules(
        &self,
        Parameters(p): Parameters<TestRulesParams>,
    ) -> Result<CallToolResult, McpError> {
        let yaml = std::fs::read_to_string(&p.fixtures_path)
            .map_err(|e| mcp_err(format!("failed to read {}: {e}", p.fixtures_path)))?;

        let fixture_file = test_rules::parse_fixture(&yaml).map_err(mcp_err)?;

        match test_rules::run_test_suite(&self.ops, &fixture_file, p.filter.as_deref()).await {
            Ok(summary) => {
                let json = serde_json::to_string_pretty(&summary).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Query aggregated action analytics (volume, latency, error rate, etc.).
    #[tool(
        description = "Query aggregated action analytics. Metrics: volume, outcome_breakdown, top_action_types, latency, error_rate."
    )]
    async fn query_analytics(
        &self,
        Parameters(p): Parameters<QueryAnalyticsParams>,
    ) -> Result<CallToolResult, McpError> {
        let metric = match p.metric.as_str() {
            "volume" => acteon_core::AnalyticsMetric::Volume,
            "outcome_breakdown" => acteon_core::AnalyticsMetric::OutcomeBreakdown,
            "top_action_types" => acteon_core::AnalyticsMetric::TopActionTypes,
            "latency" => acteon_core::AnalyticsMetric::Latency,
            "error_rate" => acteon_core::AnalyticsMetric::ErrorRate,
            other => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Unknown metric: {other}. Valid: volume, outcome_breakdown, top_action_types, latency, error_rate"
                ))]));
            }
        };

        let interval = match p.interval.as_deref() {
            Some("hourly") => acteon_core::AnalyticsInterval::Hourly,
            Some("weekly") => acteon_core::AnalyticsInterval::Weekly,
            Some("monthly") => acteon_core::AnalyticsInterval::Monthly,
            _ => acteon_core::AnalyticsInterval::Daily,
        };

        let from = p
            .from
            .as_deref()
            .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok());
        let to =
            p.to.as_deref()
                .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok());

        let query = acteon_core::AnalyticsQuery {
            metric,
            namespace: p.namespace,
            tenant: p.tenant,
            provider: p.provider,
            action_type: p.action_type,
            outcome: p.outcome,
            interval,
            from,
            to,
            group_by: p.group_by,
            top_n: p.top_n,
            // Authorization scope is applied server-side from grants; clients
            // never set it (the field is `#[serde(skip)]`).
            tenant_scope: Vec::new(),
        };

        match self.ops.query_analytics(query).await {
            Ok(resp) => {
                let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Manage a circuit breaker (trip or reset).
    #[tool(
        description = "Trip (force open) or reset (force close) a circuit breaker for a provider."
    )]
    async fn manage_circuit_breaker(
        &self,
        Parameters(p): Parameters<ManageCircuitBreakerParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = match p.action.as_str() {
            "trip" => self.ops.trip_circuit_breaker(p.provider).await,
            "reset" => self.ops.reset_circuit_breaker(p.provider).await,
            _ => {
                return Ok(CallToolResult::error(vec![Content::text(
                    "Invalid action: must be 'trip' or 'reset'",
                )]));
            }
        };

        match result {
            Ok(resp) => {
                let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Manage recurring actions (CRUD + pause/resume).
    #[tool(
        description = "Manage recurring actions. Actions: list, get, create, update, delete, pause, resume. Provide namespace/tenant for list; id+namespace+tenant for get/delete/pause/resume; data JSON for create/update."
    )]
    async fn manage_recurring(
        &self,
        Parameters(p): Parameters<ManageRecurringParams>,
    ) -> Result<CallToolResult, McpError> {
        match p.action.as_str() {
            "list" => {
                let filter = RecurringFilter {
                    namespace: p.namespace.unwrap_or_default(),
                    tenant: p.tenant.unwrap_or_default(),
                    status: p.status,
                    ..Default::default()
                };
                match self.ops.list_recurring(&filter).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "get" => {
                let id =
                    p.id.as_deref()
                        .ok_or_else(|| mcp_err("'id' is required for get"))?;
                let ns = p
                    .namespace
                    .as_deref()
                    .ok_or_else(|| mcp_err("'namespace' is required for get"))?;
                let tenant = p
                    .tenant
                    .as_deref()
                    .ok_or_else(|| mcp_err("'tenant' is required for get"))?;
                match self.ops.get_recurring(id, ns, tenant).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "create" => {
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for create"))?;
                let req = build_create_recurring(&data)?;
                match self.ops.create_recurring(&req).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "update" => {
                let id =
                    p.id.as_deref()
                        .ok_or_else(|| mcp_err("'id' is required for update"))?;
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for update"))?;
                let update = build_update_recurring(&data)?;
                match self.ops.update_recurring(id, &update).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "delete" => {
                let id =
                    p.id.as_deref()
                        .ok_or_else(|| mcp_err("'id' is required for delete"))?;
                let ns = p
                    .namespace
                    .as_deref()
                    .ok_or_else(|| mcp_err("'namespace' is required for delete"))?;
                let tenant = p
                    .tenant
                    .as_deref()
                    .ok_or_else(|| mcp_err("'tenant' is required for delete"))?;
                match self.ops.delete_recurring(id, ns, tenant).await {
                    Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                        "Recurring action '{id}' deleted."
                    ))])),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "pause" => {
                let id =
                    p.id.as_deref()
                        .ok_or_else(|| mcp_err("'id' is required for pause"))?;
                let ns = p
                    .namespace
                    .as_deref()
                    .ok_or_else(|| mcp_err("'namespace' is required for pause"))?;
                let tenant = p
                    .tenant
                    .as_deref()
                    .ok_or_else(|| mcp_err("'tenant' is required for pause"))?;
                match self.ops.pause_recurring(id, ns, tenant).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "resume" => {
                let id =
                    p.id.as_deref()
                        .ok_or_else(|| mcp_err("'id' is required for resume"))?;
                let ns = p
                    .namespace
                    .as_deref()
                    .ok_or_else(|| mcp_err("'namespace' is required for resume"))?;
                let tenant = p
                    .tenant
                    .as_deref()
                    .ok_or_else(|| mcp_err("'tenant' is required for resume"))?;
                match self.ops.resume_recurring(id, ns, tenant).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            other => Ok(CallToolResult::error(vec![Content::text(format!(
                "Unknown action: {other}. Valid: list, get, create, update, delete, pause, resume"
            ))])),
        }
    }

    /// Manage quota policies (CRUD + usage).
    #[tool(
        description = "Manage quota policies. Actions: list, get, create, update, delete, usage. Provide namespace/tenant for list/delete; id for get/update/delete/usage; data JSON for create/update."
    )]
    async fn manage_quotas(
        &self,
        Parameters(p): Parameters<ManageQuotasParams>,
    ) -> Result<CallToolResult, McpError> {
        match p.action.as_str() {
            "list" => {
                match self
                    .ops
                    .list_quotas(
                        p.namespace.as_deref(),
                        p.tenant.as_deref(),
                        p.provider.as_deref(),
                        p.principal.as_deref(),
                    )
                    .await
                {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "get" => {
                let id =
                    p.id.as_deref()
                        .ok_or_else(|| mcp_err("'id' is required for get"))?;
                match self.ops.get_quota(id).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "create" => {
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for create"))?;
                let req = build_create_quota(&data)?;
                match self.ops.create_quota(&req).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "update" => {
                let id =
                    p.id.as_deref()
                        .ok_or_else(|| mcp_err("'id' is required for update"))?;
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for update"))?;
                let update = build_update_quota(&data)?;
                match self.ops.update_quota(id, &update).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "delete" => {
                let id =
                    p.id.as_deref()
                        .ok_or_else(|| mcp_err("'id' is required for delete"))?;
                let ns = p
                    .namespace
                    .as_deref()
                    .ok_or_else(|| mcp_err("'namespace' is required for delete"))?;
                let tenant = p
                    .tenant
                    .as_deref()
                    .ok_or_else(|| mcp_err("'tenant' is required for delete"))?;
                match self.ops.delete_quota(id, ns, tenant).await {
                    Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                        "Quota '{id}' deleted."
                    ))])),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "usage" => {
                let id =
                    p.id.as_deref()
                        .ok_or_else(|| mcp_err("'id' is required for usage"))?;
                match self.ops.get_quota_usage(id).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            other => Ok(CallToolResult::error(vec![Content::text(format!(
                "Unknown action: {other}. Valid: list, get, create, update, delete, usage"
            ))])),
        }
    }

    /// Manage data retention policies (CRUD).
    #[tool(
        description = "Manage data retention policies. Actions: list, get, create, update, delete. Provide namespace/tenant for list; id for get/update/delete; data JSON for create/update."
    )]
    async fn manage_retention(
        &self,
        Parameters(p): Parameters<ManageRetentionParams>,
    ) -> Result<CallToolResult, McpError> {
        match p.action.as_str() {
            "list" => match self.ops.list_retention(p.namespace, p.tenant).await {
                Ok(resp) => {
                    let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                    Ok(CallToolResult::success(vec![Content::text(json)]))
                }
                Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
            },
            "get" => {
                let id =
                    p.id.as_deref()
                        .ok_or_else(|| mcp_err("'id' is required for get"))?;
                match self.ops.get_retention(id).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "create" => {
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for create"))?;
                let req = build_create_retention(&data)?;
                match self.ops.create_retention(&req).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "update" => {
                let id =
                    p.id.as_deref()
                        .ok_or_else(|| mcp_err("'id' is required for update"))?;
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for update"))?;
                let update = build_update_retention(&data);
                match self.ops.update_retention(id, &update).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "delete" => {
                let id =
                    p.id.as_deref()
                        .ok_or_else(|| mcp_err("'id' is required for delete"))?;
                match self.ops.delete_retention(id).await {
                    Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                        "Retention policy '{id}' deleted."
                    ))])),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            other => Ok(CallToolResult::error(vec![Content::text(format!(
                "Unknown action: {other}. Valid: list, get, create, update, delete"
            ))])),
        }
    }

    /// Manage payload templates (CRUD).
    #[tool(
        description = "Manage payload templates. Actions: list, get, create, update, delete. Provide namespace/tenant for list; id for get/update/delete; data JSON for create/update."
    )]
    async fn manage_templates(
        &self,
        Parameters(p): Parameters<ManageTemplatesParams>,
    ) -> Result<CallToolResult, McpError> {
        match p.action.as_str() {
            "list" => {
                match self
                    .ops
                    .list_templates(p.namespace.as_deref(), p.tenant.as_deref())
                    .await
                {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "get" => {
                let id =
                    p.id.as_deref()
                        .ok_or_else(|| mcp_err("'id' is required for get"))?;
                match self.ops.get_template(id).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "create" => {
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for create"))?;
                let req = build_create_template(&data)?;
                match self.ops.create_template(&req).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "update" => {
                let id =
                    p.id.as_deref()
                        .ok_or_else(|| mcp_err("'id' is required for update"))?;
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for update"))?;
                let update = build_update_template(&data);
                match self.ops.update_template(id, &update).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "delete" => {
                let id =
                    p.id.as_deref()
                        .ok_or_else(|| mcp_err("'id' is required for delete"))?;
                match self.ops.delete_template(id).await {
                    Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                        "Template '{id}' deleted."
                    ))])),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            other => Ok(CallToolResult::error(vec![Content::text(format!(
                "Unknown action: {other}. Valid: list, get, create, update, delete"
            ))])),
        }
    }

    /// Manage template profiles (CRUD + render preview).
    #[tool(
        description = "Manage template profiles. Actions: list, get, create, update, delete, render. Provide namespace/tenant for list; id for get/update/delete; data JSON for create/update/render."
    )]
    async fn manage_template_profiles(
        &self,
        Parameters(p): Parameters<ManageTemplateProfilesParams>,
    ) -> Result<CallToolResult, McpError> {
        match p.action.as_str() {
            "list" => {
                match self
                    .ops
                    .list_profiles(p.namespace.as_deref(), p.tenant.as_deref())
                    .await
                {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "get" => {
                let id =
                    p.id.as_deref()
                        .ok_or_else(|| mcp_err("'id' is required for get"))?;
                match self.ops.get_profile(id).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "create" => {
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for create"))?;
                let req = build_create_profile(&data)?;
                match self.ops.create_profile(&req).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "update" => {
                let id =
                    p.id.as_deref()
                        .ok_or_else(|| mcp_err("'id' is required for update"))?;
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for update"))?;
                let update = build_update_profile(&data);
                match self.ops.update_profile(id, &update).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "delete" => {
                let id =
                    p.id.as_deref()
                        .ok_or_else(|| mcp_err("'id' is required for delete"))?;
                match self.ops.delete_profile(id).await {
                    Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                        "Template profile '{id}' deleted."
                    ))])),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "render" => {
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for render"))?;
                let req = build_render_preview(&data)?;
                match self.ops.render_preview(&req).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            other => Ok(CallToolResult::error(vec![Content::text(format!(
                "Unknown action: {other}. Valid: list, get, create, update, delete, render"
            ))])),
        }
    }

    /// Manage WASM plugins (list, delete).
    #[tool(description = "Manage WASM plugins. Actions: list, delete. Provide 'name' for delete.")]
    async fn manage_plugins(
        &self,
        Parameters(p): Parameters<ManagePluginsParams>,
    ) -> Result<CallToolResult, McpError> {
        match p.action.as_str() {
            "list" => match self.ops.list_plugins().await {
                Ok(resp) => {
                    let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                    Ok(CallToolResult::success(vec![Content::text(json)]))
                }
                Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
            },
            "delete" => {
                let name = p
                    .name
                    .as_deref()
                    .ok_or_else(|| mcp_err("'name' is required for delete"))?;
                match self.ops.delete_plugin(name).await {
                    Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                        "Plugin '{name}' deleted."
                    ))])),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            other => Ok(CallToolResult::error(vec![Content::text(format!(
                "Unknown action: {other}. Valid: list, delete"
            ))])),
        }
    }

    /// Manage event groups (list, get, flush).
    #[tool(
        description = "Manage event groups. Actions: list, get, flush. Provide 'key' for get/flush."
    )]
    async fn manage_groups(
        &self,
        Parameters(p): Parameters<ManageGroupsParams>,
    ) -> Result<CallToolResult, McpError> {
        match p.action.as_str() {
            "list" => match self.ops.list_groups().await {
                Ok(resp) => {
                    let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                    Ok(CallToolResult::success(vec![Content::text(json)]))
                }
                Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
            },
            "get" => {
                let key = p
                    .key
                    .as_deref()
                    .ok_or_else(|| mcp_err("'key' is required for get"))?;
                match self.ops.get_group(key).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "flush" => {
                let key = p
                    .key
                    .as_deref()
                    .ok_or_else(|| mcp_err("'key' is required for flush"))?;
                match self.ops.flush_group(key).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            other => Ok(CallToolResult::error(vec![Content::text(format!(
                "Unknown action: {other}. Valid: list, get, flush"
            ))])),
        }
    }

    /// Manage silences (CRUD).
    #[tool(
        description = "Manage silences — time-bounded label-pattern mutes that suppress dispatched actions. Actions: list, get, create, update, delete. Provide namespace/tenant/include_expired for list; id for get/update/delete; data JSON for create/update. Create data must include: namespace, tenant, matchers (array of {name, value, op}), comment, and either ends_at or duration_seconds. Update data may include ends_at and comment."
    )]
    async fn manage_silences(
        &self,
        Parameters(p): Parameters<ManageSilencesParams>,
    ) -> Result<CallToolResult, McpError> {
        match p.action.as_str() {
            "list" => {
                let query = ListSilencesQuery {
                    namespace: p.namespace.clone(),
                    tenant: p.tenant.clone(),
                    include_expired: p.include_expired.unwrap_or(false),
                };
                match self.ops.list_silences(&query).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "get" => {
                let id =
                    p.id.as_deref()
                        .ok_or_else(|| mcp_err("'id' is required for get"))?;
                match self.ops.get_silence(id).await {
                    Ok(Some(resp)) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Ok(None) => Ok(CallToolResult::error(vec![Content::text(format!(
                        "Silence not found: {id}"
                    ))])),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "create" => {
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for create"))?;
                let req = build_create_silence(&data)?;
                match self.ops.create_silence(&req).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "update" => {
                let id =
                    p.id.as_deref()
                        .ok_or_else(|| mcp_err("'id' is required for update"))?;
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for update"))?;
                let update = build_update_silence(&data)?;
                match self.ops.update_silence(id, &update).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "delete" => {
                let id =
                    p.id.as_deref()
                        .ok_or_else(|| mcp_err("'id' is required for delete"))?;
                match self.ops.delete_silence(id).await {
                    Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                        "Silence '{id}' expired."
                    ))])),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            other => Ok(CallToolResult::error(vec![Content::text(format!(
                "Unknown action: {other}. Valid: list, get, create, update, delete"
            ))])),
        }
    }

    /// Manage the dead letter queue (stats, drain).
    #[tool(
        description = "Manage the dead letter queue. Actions: stats (view statistics), drain (reprocess all DLQ entries)."
    )]
    async fn manage_dlq(
        &self,
        Parameters(p): Parameters<ManageDlqParams>,
    ) -> Result<CallToolResult, McpError> {
        match p.action.as_str() {
            "stats" => match self.ops.dlq_stats().await {
                Ok(resp) => {
                    let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                    Ok(CallToolResult::success(vec![Content::text(json)]))
                }
                Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
            },
            "drain" => match self.ops.dlq_drain().await {
                Ok(resp) => {
                    let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                    Ok(CallToolResult::success(vec![Content::text(json)]))
                }
                Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
            },
            other => Ok(CallToolResult::error(vec![Content::text(format!(
                "Unknown action: {other}. Valid: stats, drain"
            ))])),
        }
    }

    /// Check compliance status or verify audit hash chain integrity.
    #[tool(
        description = "Check compliance status or verify audit hash chain integrity. Actions: status (default), verify (requires data with VerifyHashChainRequest)."
    )]
    async fn compliance_status(
        &self,
        Parameters(p): Parameters<ComplianceStatusParams>,
    ) -> Result<CallToolResult, McpError> {
        match p.action.as_str() {
            "status" => match self.ops.get_compliance_status().await {
                Ok(resp) => {
                    let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                    Ok(CallToolResult::success(vec![Content::text(json)]))
                }
                Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
            },
            "verify" => {
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for verify"))?;
                let req: VerifyHashChainRequest = serde_json::from_value(data)
                    .map_err(|e| mcp_err(format!("invalid data: {e}")))?;
                match self.ops.verify_audit_chain(&req).await {
                    Ok(resp) => {
                        let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                        Ok(CallToolResult::success(vec![Content::text(json)]))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            other => Ok(CallToolResult::error(vec![Content::text(format!(
                "Unknown action: {other}. Valid: status, verify"
            ))])),
        }
    }

    /// Get health status for all configured providers.
    #[tool(
        description = "Get health status, latency metrics, and circuit breaker state for all configured providers."
    )]
    async fn provider_health(
        &self,
        Parameters(_p): Parameters<ProviderHealthParams>,
    ) -> Result<CallToolResult, McpError> {
        match self.ops.list_provider_health().await {
            Ok(resp) => {
                let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// List pending approvals for a namespace and tenant.
    #[tool(description = "List pending approval requests for a namespace and tenant.")]
    async fn list_approvals(
        &self,
        Parameters(p): Parameters<ListApprovalsParams>,
    ) -> Result<CallToolResult, McpError> {
        match self.ops.list_approvals(&p.namespace, &p.tenant).await {
            Ok(resp) => {
                let json = serde_json::to_string_pretty(&resp).map_err(mcp_err)?;
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    // ---------------------------------------------------------------------
    // Agentic bus tools
    // ---------------------------------------------------------------------

    /// Manage bus topics (list/create/delete/publish).
    #[tool(
        description = "Manage agentic bus topics. Actions: list (filter by namespace/tenant), create (data = CreateBusTopic), delete (kafka_name required), publish (data = PublishBusMessage)."
    )]
    async fn manage_bus_topics(
        &self,
        Parameters(p): Parameters<ManageBusTopicsParams>,
    ) -> Result<CallToolResult, McpError> {
        use acteon_ops::acteon_client::{BusTopicFilter, CreateBusTopic, PublishBusMessage};
        let client = self.ops.client();
        match p.action.as_str() {
            "list" => {
                let filter = BusTopicFilter {
                    namespace: p.namespace,
                    tenant: p.tenant,
                };
                ok_or_err(client.list_bus_topics(&filter).await)
            }
            "create" => {
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for create"))?;
                let req: CreateBusTopic = serde_json::from_value(data)
                    .map_err(|e| mcp_err(format!("invalid data: {e}")))?;
                ok_or_err(client.create_bus_topic(&req).await)
            }
            "delete" => {
                let name = p
                    .kafka_name
                    .as_deref()
                    .ok_or_else(|| mcp_err("'kafka_name' is required for delete"))?;
                match client.delete_bus_topic(name).await {
                    Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                        "Topic '{name}' deleted."
                    ))])),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "publish" => {
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for publish"))?;
                let req: PublishBusMessage = serde_json::from_value(data)
                    .map_err(|e| mcp_err(format!("invalid data: {e}")))?;
                ok_or_err(client.publish_message(&req).await)
            }
            other => Ok(CallToolResult::error(vec![Content::text(format!(
                "Unknown action: {other}. Valid: list, create, delete, publish"
            ))])),
        }
    }

    /// Manage bus subscriptions (list/create/delete/lag/ack).
    #[tool(
        description = "Manage agentic bus subscriptions. Actions: list (filter by namespace/tenant/topic), create (data = CreateSubscription), delete (namespace+tenant+id), lag (namespace+tenant+id), ack (namespace+tenant+id+partition+offset). Ack performs a full broker round-trip — use for end-of-batch checkpoints, not per-record."
    )]
    async fn manage_bus_subscriptions(
        &self,
        Parameters(p): Parameters<ManageBusSubscriptionsParams>,
    ) -> Result<CallToolResult, McpError> {
        use acteon_ops::acteon_client::{AckOffset, BusSubscriptionFilter, CreateSubscription};
        let client = self.ops.client();
        match p.action.as_str() {
            "list" => {
                let filter = BusSubscriptionFilter {
                    namespace: p.namespace,
                    tenant: p.tenant,
                    topic: p.topic,
                };
                ok_or_err(client.list_bus_subscriptions(&filter).await)
            }
            "create" => {
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for create"))?;
                let req: CreateSubscription = serde_json::from_value(data)
                    .map_err(|e| mcp_err(format!("invalid data: {e}")))?;
                ok_or_err(client.create_bus_subscription(&req).await)
            }
            "delete" => {
                let (ns, t, id) = bus_three_tuple(
                    p.namespace.as_ref(),
                    p.tenant.as_ref(),
                    p.id.as_ref(),
                    "delete",
                )?;
                match client.delete_bus_subscription(ns, t, id).await {
                    Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                        "Subscription '{id}' deleted."
                    ))])),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "lag" => {
                let (ns, t, id) = bus_three_tuple(
                    p.namespace.as_ref(),
                    p.tenant.as_ref(),
                    p.id.as_ref(),
                    "lag",
                )?;
                ok_or_err(client.get_bus_lag(ns, t, id).await)
            }
            "ack" => {
                let (ns, t, id) = bus_three_tuple(
                    p.namespace.as_ref(),
                    p.tenant.as_ref(),
                    p.id.as_ref(),
                    "ack",
                )?;
                let partition = p
                    .partition
                    .ok_or_else(|| mcp_err("'partition' is required for ack"))?;
                let offset = p
                    .offset
                    .ok_or_else(|| mcp_err("'offset' is required for ack"))?;
                match client
                    .ack_bus_subscription(ns, t, id, AckOffset { partition, offset })
                    .await
                {
                    Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                        "Offset committed: subscription '{id}' partition {partition} offset {offset}"
                    ))])),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            other => Ok(CallToolResult::error(vec![Content::text(format!(
                "Unknown action: {other}. Valid: list, create, delete, lag, ack"
            ))])),
        }
    }

    /// Manage JSON Schemas and topic-schema bindings.
    #[tool(
        description = "Manage agentic bus JSON Schemas. Actions: list (filter by namespace/tenant/subject + latest_only), versions (namespace+tenant+subject), get (namespace+tenant+subject+version, version may be 'latest' or a number), register (data = RegisterBusSchema), delete (namespace+tenant+subject+version numeric), bind (namespace+tenant+topic+subject+version), unbind (namespace+tenant+topic)."
    )]
    async fn manage_bus_schemas(
        &self,
        Parameters(p): Parameters<ManageBusSchemasParams>,
    ) -> Result<CallToolResult, McpError> {
        use acteon_ops::acteon_client::{BusSchemaFilter, RegisterBusSchema};
        let client = self.ops.client();
        match p.action.as_str() {
            "list" => {
                let filter = BusSchemaFilter {
                    namespace: p.namespace,
                    tenant: p.tenant,
                    subject: p.subject,
                    latest_only: p.latest_only.unwrap_or(false),
                };
                ok_or_err(client.list_bus_schemas(&filter).await)
            }
            "versions" => {
                let (ns, t, subject) = bus_three_tuple(
                    p.namespace.as_ref(),
                    p.tenant.as_ref(),
                    p.subject.as_ref(),
                    "versions",
                )?;
                ok_or_err(client.get_bus_schema_versions(ns, t, subject).await)
            }
            "get" => {
                let (ns, t, subject) = bus_three_tuple(
                    p.namespace.as_ref(),
                    p.tenant.as_ref(),
                    p.subject.as_ref(),
                    "get",
                )?;
                let version = match p.version.as_ref() {
                    Some(serde_json::Value::String(s)) => s.clone(),
                    Some(serde_json::Value::Number(n)) => n.to_string(),
                    Some(_) | None => "latest".to_string(),
                };
                ok_or_err(client.get_bus_schema(ns, t, subject, &version).await)
            }
            "register" => {
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for register"))?;
                let req: RegisterBusSchema = serde_json::from_value(data)
                    .map_err(|e| mcp_err(format!("invalid data: {e}")))?;
                ok_or_err(client.register_bus_schema(&req).await)
            }
            "delete" => {
                let (ns, t, subject) = bus_three_tuple(
                    p.namespace.as_ref(),
                    p.tenant.as_ref(),
                    p.subject.as_ref(),
                    "delete",
                )?;
                let version = p
                    .version
                    .as_ref()
                    .and_then(serde_json::Value::as_i64)
                    .ok_or_else(|| mcp_err("numeric 'version' is required for delete"))?;
                let v =
                    i32::try_from(version).map_err(|_| mcp_err("'version' out of i32 range"))?;
                match client.delete_bus_schema(ns, t, subject, v).await {
                    Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                        "Schema '{subject}' v{v} deleted."
                    ))])),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "bind" => {
                let (ns, t, topic) = bus_three_tuple(
                    p.namespace.as_ref(),
                    p.tenant.as_ref(),
                    p.topic.as_ref(),
                    "bind",
                )?;
                let subject = p
                    .subject
                    .as_deref()
                    .ok_or_else(|| mcp_err("'subject' is required for bind"))?;
                let version = p
                    .version
                    .as_ref()
                    .and_then(serde_json::Value::as_i64)
                    .ok_or_else(|| mcp_err("numeric 'version' is required for bind"))?;
                let v =
                    i32::try_from(version).map_err(|_| mcp_err("'version' out of i32 range"))?;
                ok_or_err(client.bind_topic_schema(ns, t, topic, subject, v).await)
            }
            "unbind" => {
                let (ns, t, topic) = bus_three_tuple(
                    p.namespace.as_ref(),
                    p.tenant.as_ref(),
                    p.topic.as_ref(),
                    "unbind",
                )?;
                match client.unbind_topic_schema(ns, t, topic).await {
                    Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                        "Topic '{topic}' schema binding removed."
                    ))])),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            other => Ok(CallToolResult::error(vec![Content::text(format!(
                "Unknown action: {other}. Valid: list, versions, get, register, delete, bind, unbind"
            ))])),
        }
    }

    /// Manage agents (list/get/register/update/delete/heartbeat/send).
    #[tool(
        description = "Manage agentic bus agents. Actions: list (filter by namespace/tenant/capability/status), get (namespace+tenant+agent_id), register (data = RegisterBusAgent), update (namespace+tenant+agent_id, data = UpdateBusAgent), delete (namespace+tenant+agent_id), heartbeat (namespace+tenant+agent_id), send (namespace+tenant+agent_id, data = SendToBusAgent)."
    )]
    async fn manage_bus_agents(
        &self,
        Parameters(p): Parameters<ManageBusAgentsParams>,
    ) -> Result<CallToolResult, McpError> {
        use acteon_ops::acteon_client::{
            BusAgentFilter, RegisterBusAgent, SendToBusAgent, UpdateBusAgent,
        };
        let client = self.ops.client();
        match p.action.as_str() {
            "list" => {
                let filter = BusAgentFilter {
                    namespace: p.namespace,
                    tenant: p.tenant,
                    capability: p.capability,
                    status: p.status,
                    // MCP params don't surface admin-state yet;
                    // add it to ManageBusAgentsParams in a
                    // follow-up if MCP-driven moderation needs it.
                    admin_state: None,
                };
                ok_or_err(client.list_bus_agents(&filter).await)
            }
            "get" => {
                let (ns, t, id) = bus_three_tuple(
                    p.namespace.as_ref(),
                    p.tenant.as_ref(),
                    p.agent_id.as_ref(),
                    "get",
                )?;
                ok_or_err(client.get_bus_agent(ns, t, id).await)
            }
            "register" => {
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for register"))?;
                let req: RegisterBusAgent = serde_json::from_value(data)
                    .map_err(|e| mcp_err(format!("invalid data: {e}")))?;
                ok_or_err(client.register_bus_agent(&req).await)
            }
            "update" => {
                let (ns, t, id) = bus_three_tuple(
                    p.namespace.as_ref(),
                    p.tenant.as_ref(),
                    p.agent_id.as_ref(),
                    "update",
                )?;
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for update"))?;
                let req: UpdateBusAgent = serde_json::from_value(data)
                    .map_err(|e| mcp_err(format!("invalid data: {e}")))?;
                ok_or_err(client.update_bus_agent(ns, t, id, &req).await)
            }
            "delete" => {
                let (ns, t, id) = bus_three_tuple(
                    p.namespace.as_ref(),
                    p.tenant.as_ref(),
                    p.agent_id.as_ref(),
                    "delete",
                )?;
                match client.delete_bus_agent(ns, t, id).await {
                    Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                        "Agent '{id}' deleted."
                    ))])),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "heartbeat" => {
                let (ns, t, id) = bus_three_tuple(
                    p.namespace.as_ref(),
                    p.tenant.as_ref(),
                    p.agent_id.as_ref(),
                    "heartbeat",
                )?;
                ok_or_err(client.heartbeat_bus_agent(ns, t, id).await)
            }
            "send" => {
                let (ns, t, id) = bus_three_tuple(
                    p.namespace.as_ref(),
                    p.tenant.as_ref(),
                    p.agent_id.as_ref(),
                    "send",
                )?;
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for send"))?;
                let req: SendToBusAgent = serde_json::from_value(data)
                    .map_err(|e| mcp_err(format!("invalid data: {e}")))?;
                ok_or_err(client.send_to_bus_agent(ns, t, id, &req).await)
            }
            other => Ok(CallToolResult::error(vec![Content::text(format!(
                "Unknown action: {other}. Valid: list, get, register, update, delete, heartbeat, send"
            ))])),
        }
    }

    /// Manage conversations (list/get/create/update/delete/transition/append/replay).
    #[tool(
        description = "Manage agentic bus conversations. Actions: list (filter by namespace/tenant/state/participant), get/update/delete/transition/append/replay (namespace+tenant+conversation_id), create (data = RegisterBusConversation), update (data = UpdateBusConversation), transition (transition: resolve/reopen/archive), append (data = AppendBusConversationMessage), replay (data = ReplayBusConversationParams; cursor and limit recommended)."
    )]
    async fn manage_bus_conversations(
        &self,
        Parameters(p): Parameters<ManageBusConversationsParams>,
    ) -> Result<CallToolResult, McpError> {
        use acteon_ops::acteon_client::{
            AppendBusConversationMessage, BusConversationFilter, BusConversationTransition,
            RegisterBusConversation, ReplayBusConversationParams, UpdateBusConversation,
        };
        let client = self.ops.client();
        match p.action.as_str() {
            "list" => {
                let filter = BusConversationFilter {
                    namespace: p.namespace,
                    tenant: p.tenant,
                    state: p.state,
                    participant: p.participant,
                };
                ok_or_err(client.list_bus_conversations(&filter).await)
            }
            "get" => {
                let (ns, t, id) = bus_three_tuple(
                    p.namespace.as_ref(),
                    p.tenant.as_ref(),
                    p.conversation_id.as_ref(),
                    "get",
                )?;
                ok_or_err(client.get_bus_conversation(ns, t, id).await)
            }
            "create" => {
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for create"))?;
                let req: RegisterBusConversation = serde_json::from_value(data)
                    .map_err(|e| mcp_err(format!("invalid data: {e}")))?;
                ok_or_err(client.register_bus_conversation(&req).await)
            }
            "update" => {
                let (ns, t, id) = bus_three_tuple(
                    p.namespace.as_ref(),
                    p.tenant.as_ref(),
                    p.conversation_id.as_ref(),
                    "update",
                )?;
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for update"))?;
                let req: UpdateBusConversation = serde_json::from_value(data)
                    .map_err(|e| mcp_err(format!("invalid data: {e}")))?;
                ok_or_err(client.update_bus_conversation(ns, t, id, &req).await)
            }
            "delete" => {
                let (ns, t, id) = bus_three_tuple(
                    p.namespace.as_ref(),
                    p.tenant.as_ref(),
                    p.conversation_id.as_ref(),
                    "delete",
                )?;
                match client.delete_bus_conversation(ns, t, id).await {
                    Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                        "Conversation '{id}' deleted."
                    ))])),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "transition" => {
                let (ns, t, id) = bus_three_tuple(
                    p.namespace.as_ref(),
                    p.tenant.as_ref(),
                    p.conversation_id.as_ref(),
                    "transition",
                )?;
                let transition = match p.transition.as_deref() {
                    Some("resolve") => BusConversationTransition::Resolve,
                    Some("reopen") => BusConversationTransition::Reopen,
                    Some("archive") => BusConversationTransition::Archive,
                    Some(other) => {
                        return Err(mcp_err(format!(
                            "unknown transition '{other}'. Valid: resolve, reopen, archive"
                        )));
                    }
                    None => {
                        return Err(mcp_err("'transition' is required for transition action"));
                    }
                };
                ok_or_err(
                    client
                        .transition_bus_conversation(ns, t, id, transition)
                        .await,
                )
            }
            "append" => {
                let (ns, t, id) = bus_three_tuple(
                    p.namespace.as_ref(),
                    p.tenant.as_ref(),
                    p.conversation_id.as_ref(),
                    "append",
                )?;
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for append"))?;
                let req: AppendBusConversationMessage = serde_json::from_value(data)
                    .map_err(|e| mcp_err(format!("invalid data: {e}")))?;
                ok_or_err(
                    client
                        .append_bus_conversation_message(ns, t, id, &req)
                        .await,
                )
            }
            "replay" => {
                let (ns, t, id) = bus_three_tuple(
                    p.namespace.as_ref(),
                    p.tenant.as_ref(),
                    p.conversation_id.as_ref(),
                    "replay",
                )?;
                let params: ReplayBusConversationParams = match p.data {
                    Some(d) => serde_json::from_value(d)
                        .map_err(|e| mcp_err(format!("invalid data: {e}")))?,
                    None => ReplayBusConversationParams::default(),
                };
                ok_or_err(
                    client
                        .replay_bus_conversation_messages(ns, t, id, &params)
                        .await,
                )
            }
            other => Ok(CallToolResult::error(vec![Content::text(format!(
                "Unknown action: {other}. Valid: list, get, create, update, delete, transition, append, replay"
            ))])),
        }
    }

    /// Post and look up tool-call envelopes.
    #[tool(
        description = "Post and look up agentic-bus tool-call envelopes. Actions: post (namespace+tenant+conversation_id, data = PostBusToolCall — set require_approval=true to gate behind HITL approval), post_result (namespace+tenant+conversation_id, data = PostBusToolResult), lookup (namespace+tenant+call_id+conversation_id; pass cursor from the post receipt and timeout_ms <= 30000 for reliable matches)."
    )]
    async fn bus_tool_call(
        &self,
        Parameters(p): Parameters<BusToolCallParams>,
    ) -> Result<CallToolResult, McpError> {
        use acteon_ops::acteon_client::{
            BusToolResultLookupParams, PostBusToolCall, PostBusToolCallOutcome, PostBusToolResult,
        };
        let client = self.ops.client();
        match p.action.as_str() {
            "post" => {
                let conversation_id = p
                    .conversation_id
                    .as_deref()
                    .ok_or_else(|| mcp_err("'conversation_id' is required for post"))?;
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for post"))?;
                let req: PostBusToolCall = serde_json::from_value(data)
                    .map_err(|e| mcp_err(format!("invalid data: {e}")))?;
                match client
                    .post_bus_tool_call(&p.namespace, &p.tenant, conversation_id, &req)
                    .await
                {
                    Ok(PostBusToolCallOutcome::Produced(receipt)) => {
                        let body = serde_json::json!({
                            "outcome": "produced",
                            "receipt": receipt,
                        });
                        ok_or_err::<_, acteon_ops::acteon_client::Error>(Ok(body))
                    }
                    Ok(PostBusToolCallOutcome::Parked(parked)) => {
                        let body = serde_json::json!({
                            "outcome": "parked",
                            "approval": parked,
                        });
                        ok_or_err::<_, acteon_ops::acteon_client::Error>(Ok(body))
                    }
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            }
            "post_result" => {
                let conversation_id = p
                    .conversation_id
                    .as_deref()
                    .ok_or_else(|| mcp_err("'conversation_id' is required for post_result"))?;
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for post_result"))?;
                let req: PostBusToolResult = serde_json::from_value(data)
                    .map_err(|e| mcp_err(format!("invalid data: {e}")))?;
                ok_or_err(
                    client
                        .post_bus_tool_result(&p.namespace, &p.tenant, conversation_id, &req)
                        .await,
                )
            }
            "lookup" => {
                let call_id = p
                    .call_id
                    .as_deref()
                    .ok_or_else(|| mcp_err("'call_id' is required for lookup"))?;
                let conversation_id = p
                    .conversation_id
                    .ok_or_else(|| mcp_err("'conversation_id' is required for lookup"))?;
                let params = BusToolResultLookupParams {
                    conversation_id,
                    cursor: p.cursor,
                    timeout_ms: p.timeout_ms,
                };
                ok_or_err(
                    client
                        .lookup_bus_tool_result(&p.namespace, &p.tenant, call_id, &params)
                        .await,
                )
            }
            other => Ok(CallToolResult::error(vec![Content::text(format!(
                "Unknown action: {other}. Valid: post, post_result, lookup"
            ))])),
        }
    }

    /// Stream-chunk envelopes and SSE consume URLs.
    #[tool(
        description = "Drive bus streaming envelopes. Actions: chunk (data = PostBusStreamChunk), end (data = PostBusStreamEnd), consume_url (stream_id required; returns the SSE URL for tailing — pipe into curl -N --header 'accept: text/event-stream' or any EventSource client)."
    )]
    async fn bus_stream(
        &self,
        Parameters(p): Parameters<BusStreamParams>,
    ) -> Result<CallToolResult, McpError> {
        use acteon_ops::acteon_client::{PostBusStreamChunk, PostBusStreamEnd};
        let client = self.ops.client();
        match p.action.as_str() {
            "chunk" => {
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for chunk"))?;
                let req: PostBusStreamChunk = serde_json::from_value(data)
                    .map_err(|e| mcp_err(format!("invalid data: {e}")))?;
                ok_or_err(
                    client
                        .post_bus_stream_chunk(&p.namespace, &p.tenant, &p.conversation_id, &req)
                        .await,
                )
            }
            "end" => {
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for end"))?;
                let req: PostBusStreamEnd = serde_json::from_value(data)
                    .map_err(|e| mcp_err(format!("invalid data: {e}")))?;
                ok_or_err(
                    client
                        .post_bus_stream_end(&p.namespace, &p.tenant, &p.conversation_id, &req)
                        .await,
                )
            }
            "consume_url" => {
                let stream_id = p
                    .stream_id
                    .as_deref()
                    .ok_or_else(|| mcp_err("'stream_id' is required for consume_url"))?;
                let url = client.bus_stream_consume_url(
                    &p.namespace,
                    &p.tenant,
                    &p.conversation_id,
                    stream_id,
                );
                Ok(CallToolResult::success(vec![Content::text(url)]))
            }
            other => Ok(CallToolResult::error(vec![Content::text(format!(
                "Unknown action: {other}. Valid: chunk, end, consume_url"
            ))])),
        }
    }

    /// Manage HITL pre-publish approvals (list/get/approve/reject).
    #[tool(
        description = "Manage agentic-bus HITL approvals. Actions: list (namespace+tenant; optional status filter pending/approved/rejected/expired and conversation_id), get (namespace+tenant+approval_id), approve (namespace+tenant+approval_id, data = BusApprovalDecisionRequest with decided_by + optional decision_note), reject (namespace+tenant+approval_id, data = BusApprovalDecisionRequest)."
    )]
    async fn manage_bus_approvals(
        &self,
        Parameters(p): Parameters<ManageBusApprovalsParams>,
    ) -> Result<CallToolResult, McpError> {
        use acteon_ops::acteon_client::{
            BusApprovalDecisionRequest, BusApprovalStatus, ListBusApprovalsParams,
        };
        let client = self.ops.client();
        match p.action.as_str() {
            "list" => {
                let status = match p.status.as_deref() {
                    None => None,
                    Some("pending") => Some(BusApprovalStatus::Pending),
                    Some("approved") => Some(BusApprovalStatus::Approved),
                    Some("rejected") => Some(BusApprovalStatus::Rejected),
                    Some("expired") => Some(BusApprovalStatus::Expired),
                    Some(other) => {
                        return Err(mcp_err(format!(
                            "unknown status '{other}'. Valid: pending, approved, rejected, expired"
                        )));
                    }
                };
                let params = ListBusApprovalsParams {
                    status,
                    conversation_id: p.conversation_id,
                };
                ok_or_err(
                    client
                        .list_bus_approvals(&p.namespace, &p.tenant, &params)
                        .await,
                )
            }
            "get" => {
                let id = p
                    .approval_id
                    .as_deref()
                    .ok_or_else(|| mcp_err("'approval_id' is required for get"))?;
                ok_or_err(client.get_bus_approval(&p.namespace, &p.tenant, id).await)
            }
            "approve" => {
                let id = p
                    .approval_id
                    .as_deref()
                    .ok_or_else(|| mcp_err("'approval_id' is required for approve"))?;
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for approve"))?;
                let req: BusApprovalDecisionRequest = serde_json::from_value(data)
                    .map_err(|e| mcp_err(format!("invalid data: {e}")))?;
                ok_or_err(
                    client
                        .approve_bus_approval(&p.namespace, &p.tenant, id, &req)
                        .await,
                )
            }
            "reject" => {
                let id = p
                    .approval_id
                    .as_deref()
                    .ok_or_else(|| mcp_err("'approval_id' is required for reject"))?;
                let data = p
                    .data
                    .ok_or_else(|| mcp_err("'data' is required for reject"))?;
                let req: BusApprovalDecisionRequest = serde_json::from_value(data)
                    .map_err(|e| mcp_err(format!("invalid data: {e}")))?;
                ok_or_err(
                    client
                        .reject_bus_approval(&p.namespace, &p.tenant, id, &req)
                        .await,
                )
            }
            other => Ok(CallToolResult::error(vec![Content::text(format!(
                "Unknown action: {other}. Valid: list, get, approve, reject"
            ))])),
        }
    }
}

// ---------------------------------------------------------------------------
// Bus tool helpers
// ---------------------------------------------------------------------------

/// Render a `Result<T, E>` from a bus client call into the MCP success/error
/// shape, where `T: Serialize` and `E: Display`. Used by the bus tool family
/// to keep each match arm readable.
fn ok_or_err<T, E>(result: Result<T, E>) -> Result<CallToolResult, McpError>
where
    T: serde::Serialize,
    E: std::fmt::Display,
{
    match result {
        Ok(v) => {
            let json = serde_json::to_string_pretty(&v).map_err(mcp_err)?;
            Ok(CallToolResult::success(vec![Content::text(json)]))
        }
        Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
    }
}

/// Resolve `namespace`, `tenant`, and a third id-shaped field that all three
/// must be present for. Centralises the "missing required field" error
/// messages so individual match arms stay short.
fn bus_three_tuple<'a>(
    namespace: Option<&'a String>,
    tenant: Option<&'a String>,
    third: Option<&'a String>,
    action: &str,
) -> Result<(&'a str, &'a str, &'a str), McpError> {
    let ns = namespace
        .map(String::as_str)
        .ok_or_else(|| mcp_err(format!("'namespace' is required for {action}")))?;
    let t = tenant
        .map(String::as_str)
        .ok_or_else(|| mcp_err(format!("'tenant' is required for {action}")))?;
    let i = third
        .map(String::as_str)
        .ok_or_else(|| mcp_err(format!("required id is missing for {action}")))?;
    Ok((ns, t, i))
}
