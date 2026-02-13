use std::collections::HashMap;
use std::path::Path;

use axum::Json;
use axum::extract::{self, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::info;
use utoipa::ToSchema;

use acteon_core::Action;
use acteon_rules_yaml::YamlFrontend;

use crate::auth::identity::CallerIdentity;
use crate::auth::role::Permission;

use super::AppState;
use super::schemas::{
    ErrorResponse, ReloadRequest, ReloadResponse, RuleSummary, SetEnabledRequest,
    SetEnabledResponse,
};

// ---------------------------------------------------------------------------
// Rule Playground types
// ---------------------------------------------------------------------------

/// Request body for evaluating rules without dispatching.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EvaluateRulesRequest {
    /// Namespace for the test action.
    #[schema(example = "default")]
    pub namespace: String,
    /// Tenant for the test action.
    #[schema(example = "acme")]
    pub tenant: String,
    /// Provider for the test action.
    #[schema(example = "email")]
    pub provider: String,
    /// Action type for the test action.
    #[schema(example = "notification")]
    pub action_type: String,
    /// Payload to evaluate against the rules.
    pub payload: serde_json::Value,
    /// Optional metadata for the test action.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// When `true`, includes disabled rules in the trace (marked as skipped).
    #[serde(default)]
    pub include_disabled: bool,
    /// When `true`, evaluates every rule even after the first match.
    #[serde(default)]
    pub evaluate_all: bool,
    /// Optional timestamp override for time-travel debugging of
    /// time-sensitive rules (maintenance windows, weekday restrictions, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluate_at: Option<DateTime<Utc>>,
    /// Optional state key overrides for testing state-dependent conditions
    /// without mutating real state.
    #[serde(default)]
    pub mock_state: HashMap<String, String>,
}

/// Per-rule trace entry in the evaluation response.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RuleTraceEntryResponse {
    /// Name of the rule.
    pub rule_name: String,
    /// Rule priority (lower is evaluated first).
    pub priority: i32,
    /// Whether the rule is enabled.
    pub enabled: bool,
    /// Human-readable condition expression.
    pub condition_display: String,
    /// Evaluation result: `"matched"`, `"not_matched"`, `"skipped"`, or `"error"`.
    pub result: String,
    /// Time spent evaluating this rule in microseconds.
    pub evaluation_duration_us: u64,
    /// The action this rule would take (e.g. `"Deny"`, `"Suppress"`).
    pub action: String,
    /// Where the rule was loaded from (e.g. `"Inline"`, `"Yaml"`).
    pub source: String,
    /// Optional rule description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Reason the rule was skipped, if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    /// Error message if evaluation failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Response from the rule evaluation playground.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EvaluateRulesResponse {
    /// The final verdict (`"allow"`, `"deny"`, `"suppress"`, `"modify"`, `"error"`).
    pub verdict: String,
    /// Name of the first rule that matched, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_rule: Option<String>,
    /// `true` when one or more rules produced an error during evaluation.
    #[serde(default)]
    pub has_errors: bool,
    /// Number of rules whose conditions were actually evaluated.
    pub total_rules_evaluated: usize,
    /// Number of rules that were skipped.
    pub total_rules_skipped: usize,
    /// Total wall-clock time for the entire evaluation in microseconds.
    pub evaluation_duration_us: u64,
    /// Per-rule trace entries in evaluation (priority) order.
    pub trace: Vec<RuleTraceEntryResponse>,
    /// Contextual information about the evaluation environment.
    pub context: serde_json::Value,
    /// When the matched rule is a `Modify` action, contains the resulting
    /// payload after applying the JSON merge patch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_payload: Option<serde_json::Value>,
}

/// `GET /v1/rules` -- list all loaded rules.
///
/// Returns an array of rule summaries (name, priority, enabled, description).
#[utoipa::path(
    get,
    path = "/v1/rules",
    tag = "Rules",
    summary = "List rules",
    description = "Returns all loaded rules with their priority, enabled state, and description.",
    responses(
        (status = 200, description = "List of loaded rules", body = Vec<RuleSummary>)
    )
)]
pub async fn list_rules(
    State(state): State<AppState>,
    axum::Extension(_identity): axum::Extension<CallerIdentity>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let rules = gw.rules();

    let body: Vec<RuleSummary> = rules
        .iter()
        .map(|r| RuleSummary {
            name: r.name.clone(),
            priority: r.priority,
            enabled: r.enabled,
            description: r.description.clone(),
        })
        .collect();

    (StatusCode::OK, Json(body))
}

/// `POST /v1/rules/reload` -- reload rules from a YAML directory.
///
/// The rules directory path is provided in the JSON body.
#[utoipa::path(
    post,
    path = "/v1/rules/reload",
    tag = "Rules",
    summary = "Reload rules",
    description = "Replaces all loaded rules by scanning YAML files in the given directory.",
    request_body(content = ReloadRequest, description = "Directory containing YAML rule files"),
    responses(
        (status = 200, description = "Rules reloaded successfully", body = ReloadResponse),
        (status = 400, description = "Missing or invalid directory", body = ErrorResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse),
        (status = 500, description = "Failed to parse rules", body = ErrorResponse)
    )
)]
pub async fn reload_rules(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    body: Option<Json<ReloadRequest>>,
) -> impl IntoResponse {
    if !identity.role.has_permission(Permission::RulesManage) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!(ErrorResponse {
                error: "insufficient permissions: rules management requires admin or operator role"
                    .into(),
            })),
        );
    }

    let dir = body.and_then(|b| b.0.directory.clone());

    let Some(dir) = dir else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!(ErrorResponse {
                error: "missing 'directory' field in request body".into(),
            })),
        );
    };

    let path = Path::new(&dir);
    if !path.is_dir() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!(ErrorResponse {
                error: format!("directory not found: {dir}"),
            })),
        );
    }

    let yaml_frontend = YamlFrontend;
    let frontends: Vec<&dyn acteon_rules::RuleFrontend> = vec![&yaml_frontend];

    let mut gw = state.gateway.write().await;
    match gw.load_rules_from_directory(path, &frontends) {
        Ok(count) => {
            info!(count, directory = %dir, "rules reloaded");
            (
                StatusCode::OK,
                Json(serde_json::json!(ReloadResponse {
                    reloaded: count,
                    directory: dir,
                })),
            )
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse {
                error: e.to_string(),
            })),
        ),
    }
}

/// `PUT /v1/rules/{name}/enabled` -- enable or disable a rule by name.
#[utoipa::path(
    put,
    path = "/v1/rules/{name}/enabled",
    tag = "Rules",
    summary = "Toggle rule",
    description = "Enables or disables a rule by name.",
    params(
        ("name" = String, Path, description = "Rule name")
    ),
    request_body(content = SetEnabledRequest, description = "Desired enabled state"),
    responses(
        (status = 200, description = "Rule toggled successfully", body = SetEnabledResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse),
        (status = 404, description = "Rule not found", body = ErrorResponse)
    )
)]
pub async fn set_rule_enabled(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    extract::Path(name): extract::Path<String>,
    Json(body): Json<SetEnabledRequest>,
) -> impl IntoResponse {
    if !identity.role.has_permission(Permission::RulesManage) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!(ErrorResponse {
                error: "insufficient permissions: rules management requires admin or operator role"
                    .into(),
            })),
        );
    }

    let mut gw = state.gateway.write().await;

    let found = if body.enabled {
        gw.enable_rule(&name)
    } else {
        gw.disable_rule(&name)
    };

    if found {
        let status = if body.enabled { "enabled" } else { "disabled" };
        (
            StatusCode::OK,
            Json(serde_json::json!(SetEnabledResponse {
                name,
                enabled: body.enabled,
                status: status.into(),
            })),
        )
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                error: format!("rule not found: {name}"),
            })),
        )
    }
}

/// `POST /v1/rules/evaluate` -- evaluate rules against a test action without dispatching.
///
/// This is a read-only debugging endpoint that returns a detailed trace of how
/// the rule engine would process the given action, without executing any side
/// effects.
#[utoipa::path(
    post,
    path = "/v1/rules/evaluate",
    tag = "Rules",
    summary = "Evaluate rules (playground)",
    description = "Evaluates the loaded rules against a test action and returns a detailed trace. No side effects are executed.",
    request_body(content = EvaluateRulesRequest, description = "Test action to evaluate"),
    responses(
        (status = 200, description = "Rule evaluation trace", body = EvaluateRulesResponse),
        (status = 400, description = "Invalid request", body = ErrorResponse),
        (status = 500, description = "Internal error", body = ErrorResponse)
    )
)]
pub async fn evaluate_rules(
    State(state): State<AppState>,
    axum::Extension(_identity): axum::Extension<CallerIdentity>,
    Json(req): Json<EvaluateRulesRequest>,
) -> impl IntoResponse {
    let action = Action::new(
        req.namespace,
        req.tenant,
        req.provider,
        req.action_type,
        req.payload,
    )
    .with_metadata(acteon_core::ActionMetadata {
        labels: req.metadata,
    });

    let gw = state.gateway.read().await;
    match gw
        .evaluate_rules(
            &action,
            req.include_disabled,
            req.evaluate_all,
            req.evaluate_at,
            req.mock_state,
        )
        .await
    {
        Ok(trace) => {
            let entries: Vec<RuleTraceEntryResponse> = trace
                .trace
                .iter()
                .map(|e| RuleTraceEntryResponse {
                    rule_name: e.rule_name.clone(),
                    priority: e.priority,
                    enabled: e.enabled,
                    condition_display: e.condition_display.clone(),
                    result: e.result.as_str().to_owned(),
                    evaluation_duration_us: e.evaluation_duration_us,
                    action: e.action.clone(),
                    source: e.source.clone(),
                    description: e.description.clone(),
                    skip_reason: e.skip_reason.clone(),
                    error: e.error.clone(),
                })
                .collect();

            let resp = EvaluateRulesResponse {
                verdict: trace.verdict,
                matched_rule: trace.matched_rule,
                has_errors: trace.has_errors,
                total_rules_evaluated: trace.total_rules_evaluated,
                total_rules_skipped: trace.total_rules_skipped,
                evaluation_duration_us: trace.evaluation_duration_us,
                trace: entries,
                context: serde_json::to_value(&trace.context).unwrap_or_default(),
                modified_payload: trace.modified_payload,
            };

            (StatusCode::OK, Json(serde_json::json!(resp)))
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse {
                error: e.to_string(),
            })),
        ),
    }
}
