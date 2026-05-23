//! Pre-defined MCP prompt templates for operational workflows.
//!
//! These guide the LLM through common Acteon tasks like incident
//! investigation, alert tuning, and guardrail drafting.

use rmcp::{
    ErrorData as McpError, RoleServer,
    handler::server::wrapper::Parameters,
    model::{GetPromptResult, PromptMessage, PromptMessageRole},
    prompt, prompt_router, schemars,
    service::RequestContext,
};
use serde::{Deserialize, Serialize};

use crate::server::ActeonMcpServer;

// ---------------------------------------------------------------------------
// Prompt argument types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct InvestigateIncidentArgs {
    /// The name of the service experiencing the incident.
    pub service: String,
    /// Tenant context.
    #[serde(default = "default_tenant")]
    pub tenant: String,
}

fn default_tenant() -> String {
    "default".to_string()
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OptimizeAlertsArgs {
    /// The notification provider to analyze (e.g. "slack", "email", "pagerduty").
    pub provider: String,
    /// Tenant context.
    #[serde(default = "default_tenant")]
    pub tenant: String,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DraftGuardrailArgs {
    /// The team or channel to protect.
    pub team: String,
    /// Quiet hours start (e.g. "22:00").
    #[serde(default)]
    pub quiet_start: Option<String>,
    /// Quiet hours end (e.g. "08:00").
    #[serde(default)]
    pub quiet_end: Option<String>,
}

// ---------------------------------------------------------------------------
// Prompt implementations
// ---------------------------------------------------------------------------

#[prompt_router]
impl ActeonMcpServer {
    /// Build the prompt router. Exposed as `pub(crate)` so `server.rs` can call it.
    pub(crate) fn create_prompt_router() -> rmcp::handler::server::router::prompt::PromptRouter<Self>
    {
        Self::prompt_router()
    }

    /// Investigate an incident for a specific service using Acteon's audit
    /// trail, event state, and rule evaluation.
    #[prompt(name = "investigate_incident")]
    async fn investigate_incident(
        &self,
        Parameters(args): Parameters<InvestigateIncidentArgs>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, McpError> {
        let messages = vec![PromptMessage::new_text(
            PromptMessageRole::User,
            format!(
                "I see a spike in errors for service '{service}' (tenant: '{tenant}'). \
                     Please help me investigate:\n\n\
                     1. Use the `query_audit` tool to find events related to '{service}' \
                        in the last 30 minutes.\n\
                     2. Use `list_events` to check for any open/acknowledged stateful events \
                        in the '{tenant}' tenant.\n\
                     3. Use `list_rules` to see if any rules were recently changed or disabled.\n\
                     4. Correlate the findings and summarize the probable root cause.\n\
                     5. Suggest next steps (acknowledge, escalate, or resolve).",
                service = args.service,
                tenant = args.tenant,
            ),
        )];

        Ok(GetPromptResult {
            description: Some(format!(
                "Investigate incident for service '{}'",
                args.service
            )),
            messages,
        })
    }

    /// Analyze notification volume for a provider and suggest grouping
    /// rules to reduce alert fatigue.
    #[prompt(name = "optimize_alerts")]
    async fn optimize_alerts(
        &self,
        Parameters(args): Parameters<OptimizeAlertsArgs>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, McpError> {
        let messages = vec![PromptMessage::new_text(
            PromptMessageRole::User,
            format!(
                "Analyze notifications sent to the '{provider}' provider for \
                     tenant '{tenant}' over the last 24 hours.\n\n\
                     1. Use `query_audit` with provider='{provider}' and tenant='{tenant}' \
                        to see recent dispatch history.\n\
                     2. Identify noisy patterns — action types that fire frequently \
                        but are often duplicates.\n\
                     3. Use `evaluate_rules` to test how the current rules handle \
                        a representative sample event.\n\
                     4. Suggest grouping or deduplication rules to reduce fatigue.\n\
                     5. Show before/after metrics (estimated notification counts).",
                provider = args.provider,
                tenant = args.tenant,
            ),
        )];

        Ok(GetPromptResult {
            description: Some(format!("Optimize alerts for '{}' provider", args.provider)),
            messages,
        })
    }

    /// Draft a natural language guardrail policy for Acteon's LLM evaluator.
    #[prompt(name = "draft_guardrail")]
    async fn draft_guardrail(
        &self,
        Parameters(args): Parameters<DraftGuardrailArgs>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, McpError> {
        let quiet_window = match (&args.quiet_start, &args.quiet_end) {
            (Some(start), Some(end)) => format!("between {start} and {end}"),
            _ => "during off-hours".to_string(),
        };

        let messages = vec![PromptMessage::new_text(
            PromptMessageRole::User,
            format!(
                "I need a policy that prevents notifications from being sent to \
                     '{team}' {quiet_window} unless the severity is 'critical'.\n\n\
                     Please draft a natural language policy string suitable for \
                     Acteon's LLM guardrail evaluator. The policy should:\n\
                     1. Block non-critical alerts during quiet hours.\n\
                     2. Always allow critical severity through.\n\
                     3. Be concise but unambiguous.\n\
                     4. Include an example of how to set it in a rule's metadata \
                        as `llm_policy`.",
                team = args.team,
            ),
        )];

        Ok(GetPromptResult {
            description: Some(format!("Draft guardrail policy for '{}'", args.team)),
            messages,
        })
    }
}
