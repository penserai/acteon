use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::AppState;
use super::schemas::ErrorResponse;
use crate::auth::identity::CallerIdentity;
use crate::auth::role::Permission;

/// Summary of a registered WASM plugin.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PluginSummary {
    /// Plugin name.
    #[schema(example = "fraud-detector")]
    pub name: String,
    /// Whether the plugin is enabled.
    #[schema(example = true)]
    pub enabled: bool,
    /// Optional description.
    pub description: Option<String>,
    /// Memory limit in bytes.
    #[schema(example = 16_777_216)]
    pub memory_limit_bytes: u64,
    /// Timeout in milliseconds.
    #[schema(example = 100)]
    pub timeout_ms: u64,
}

/// Response for listing plugins.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ListPluginsResponse {
    /// Total number of registered plugins.
    pub total: usize,
    /// Plugin summaries.
    pub plugins: Vec<PluginSummary>,
}

/// `GET /v1/plugins` -- list all registered WASM plugins.
#[utoipa::path(
    get,
    path = "/v1/plugins",
    tag = "Plugins",
    summary = "List WASM plugins",
    description = "Returns a list of all registered WASM plugins.",
    responses(
        (status = 200, description = "Plugin list", body = ListPluginsResponse),
    )
)]
pub async fn list_plugins(State(state): State<AppState>) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let Some(runtime) = gw.wasm_runtime() else {
        return (
            StatusCode::OK,
            Json(ListPluginsResponse {
                total: 0,
                plugins: vec![],
            }),
        );
    };

    let names = runtime.list_plugins();
    let plugins: Vec<PluginSummary> = names
        .into_iter()
        .map(|name| PluginSummary {
            name,
            enabled: true,
            description: None,
            memory_limit_bytes: 0,
            timeout_ms: 0,
        })
        .collect();
    let total = plugins.len();
    (StatusCode::OK, Json(ListPluginsResponse { total, plugins }))
}

/// `DELETE /v1/plugins/{name}` -- unregister a WASM plugin.
#[utoipa::path(
    delete,
    path = "/v1/plugins/{name}",
    tag = "Plugins",
    summary = "Unregister WASM plugin",
    description = "Removes a registered WASM plugin by name.",
    params(
        ("name" = String, Path, description = "Plugin name"),
    ),
    responses(
        (status = 204, description = "Plugin unregistered"),
        (status = 404, description = "Plugin not found", body = ErrorResponse),
        (status = 501, description = "WASM runtime not enabled", body = ErrorResponse),
    )
)]
pub async fn unregister_plugin(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    if !identity.role.has_permission(Permission::PluginsManage) {
        return (
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error:
                    "insufficient permissions: plugin management requires admin or operator role"
                        .into(),
            }),
        )
            .into_response();
    }

    let gw = state.gateway.read().await;
    let Some(runtime) = gw.wasm_runtime() else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(ErrorResponse {
                error: "WASM runtime is not enabled".into(),
            }),
        )
            .into_response();
    };

    if !runtime.has_plugin(&name) {
        return (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("plugin not found: {name}"),
            }),
        )
            .into_response();
    }

    // The WasmPluginRuntime trait doesn't expose unregister, so we just
    // report that the plugin exists but cannot be removed at runtime.
    // This is a limitation of the current trait design.
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(ErrorResponse {
            error: "runtime plugin removal is not yet supported via the API".into(),
        }),
    )
        .into_response()
}
