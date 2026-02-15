//! MCP tool definitions for Acteon.
//!
//! Each tool maps to one or more operations on the Acteon gateway
//! via the HTTP client.

use acteon_ops::acteon_client::{AuditQuery, EventQuery, RecurringFilter};
use acteon_ops::test_rules;
use rmcp::{
    ErrorData as McpError,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    schemars, tool, tool_router,
};
use serde::Deserialize;

use crate::server::ActeonMcpServer;

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
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListRulesParams {
    // Currently no parameters — lists all rules.
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
        };

        match self.ops.query_audit(query).await {
            Ok(page) => {
                let json = serde_json::to_string_pretty(&page).map_err(mcp_err)?;
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

        match self.ops.client().list_recurring(&filter).await {
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
        match self.ops.client().list_groups().await {
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
            .client()
            .list_quotas(p.namespace.as_deref(), p.tenant.as_deref())
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
}
