use std::path::Path;

use axum::Json;
use axum::extract::{self, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use tracing::info;

use acteon_rules_yaml::YamlFrontend;

use crate::auth::identity::CallerIdentity;
use crate::auth::role::Permission;

use super::AppState;
use super::schemas::{
    ErrorResponse, ReloadRequest, ReloadResponse, RuleSummary, SetEnabledRequest,
    SetEnabledResponse,
};

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
