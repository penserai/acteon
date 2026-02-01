use std::path::Path;
use std::sync::Arc;

use axum::extract::{self, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::info;

use acteon_gateway::Gateway;
use acteon_rules_yaml::YamlFrontend;

/// `GET /v1/rules` -- list all loaded rules.
///
/// Returns an array of rule summaries (name, priority, enabled, description).
pub async fn list_rules(State(gateway): State<Arc<RwLock<Gateway>>>) -> impl IntoResponse {
    let gw = gateway.read().await;
    let rules = gw.rules();

    let body: Vec<serde_json::Value> = rules
        .iter()
        .map(|r| {
            serde_json::json!({
                "name": r.name,
                "priority": r.priority,
                "enabled": r.enabled,
                "description": r.description,
            })
        })
        .collect();

    (StatusCode::OK, Json(body))
}

/// `POST /v1/rules/reload` -- reload rules from the configured directory.
///
/// The rules directory path is passed as a query parameter or defaults to the
/// configured one. For simplicity, we accept an optional JSON body:
/// `{"directory": "/path/to/rules"}`.
pub async fn reload_rules(
    State(gateway): State<Arc<RwLock<Gateway>>>,
    body: Option<Json<ReloadRequest>>,
) -> impl IntoResponse {
    let dir = body.and_then(|b| b.directory.clone());

    let Some(dir) = dir else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "missing 'directory' field in request body"
            })),
        );
    };

    let path = Path::new(&dir);
    if !path.is_dir() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!("directory not found: {dir}")
            })),
        );
    }

    let yaml_frontend = YamlFrontend;
    let frontends: Vec<&dyn acteon_rules::RuleFrontend> = vec![&yaml_frontend];

    let mut gw = gateway.write().await;
    match gw.load_rules_from_directory(path, &frontends) {
        Ok(count) => {
            info!(count, directory = %dir, "rules reloaded");
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "reloaded": count,
                    "directory": dir,
                })),
            )
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

/// Request body for the reload endpoint.
#[derive(Debug, Deserialize)]
pub struct ReloadRequest {
    /// Path to the rules directory.
    pub directory: Option<String>,
}

/// Request body for the enable/disable endpoint.
#[derive(Debug, Deserialize)]
pub struct SetEnabledRequest {
    /// Whether the rule should be enabled.
    pub enabled: bool,
}

/// `PUT /v1/rules/:name/enabled` -- enable or disable a rule by name.
pub async fn set_rule_enabled(
    State(gateway): State<Arc<RwLock<Gateway>>>,
    extract::Path(name): extract::Path<String>,
    Json(body): Json<SetEnabledRequest>,
) -> impl IntoResponse {
    let mut gw = gateway.write().await;

    let found = if body.enabled {
        gw.enable_rule(&name)
    } else {
        gw.disable_rule(&name)
    };

    if found {
        let status = if body.enabled { "enabled" } else { "disabled" };
        (
            StatusCode::OK,
            Json(serde_json::json!({
                "name": name,
                "enabled": body.enabled,
                "status": status,
            })),
        )
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("rule not found: {name}")
            })),
        )
    }
}
