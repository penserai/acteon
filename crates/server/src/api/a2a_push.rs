//! A2A push-notification config CRUD (Phase 4.1).
//!
//! Storage + handler layer for `TaskPushNotificationConfig`. The
//! delivery worker that POSTs streamed events to the registered URLs
//! ships in Phase 4.2; this module is read/write only.
//!
//! Endpoints exposed:
//!
//! - JSON-RPC (over `POST /a2a/{ns}/{tenant}`):
//!   - `tasks/pushNotificationConfig/set`
//!   - `tasks/pushNotificationConfig/get`
//!   - `tasks/pushNotificationConfig/list`
//!   - `tasks/pushNotificationConfig/delete`
//! - REST (A2A spec §11):
//!   - `POST   /a2a/{ns}/{tenant}/v1/tasks/{id}/pushNotificationConfigs`
//!   - `GET    /a2a/{ns}/{tenant}/v1/tasks/{id}/pushNotificationConfigs`
//!   - `GET    /a2a/{ns}/{tenant}/v1/tasks/{id}/pushNotificationConfigs/{cfgId}`
//!   - `DELETE /a2a/{ns}/{tenant}/v1/tasks/{id}/pushNotificationConfigs/{cfgId}`
//!
//! All endpoints share the standard A2A authorization check and reject
//! a request for a task that does not exist with `TaskNotFound`
//! (-32001) — so a probe cannot enumerate config ids for a task the
//! caller has no rights to.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde::Deserialize;

use acteon_core::{PushAuthentication, TaskPushNotificationConfig};
use acteon_gateway::TaskScope;
use acteon_state::{KeyKind, StateKey, StateStore};

use super::AppState;
use super::schemas::ErrorResponse;
use crate::auth::identity::CallerIdentity;
use crate::auth::role::Permission;

// ---------------------------------------------------------------------
// Storage helpers (also reachable from the JSON-RPC method shims)
// ---------------------------------------------------------------------

/// Failure modes shared by every CRUD entry point. The variants map
/// 1:1 onto JSON-RPC / HTTP outcomes — the handler decides the
/// presentation, the helper decides the kind.
#[derive(Debug)]
pub(crate) enum PushConfigError {
    /// The parent task does not exist (or the caller cannot see it).
    TaskNotFound(String),
    /// The named config row does not exist for this task.
    ConfigNotFound { task_id: String, config_id: String },
    /// The submitted config failed [`TaskPushNotificationConfig::validate`].
    Invalid(String),
    /// State-store I/O failed. Carries a generic message — the
    /// underlying error is logged but not surfaced to the caller
    /// (CWE-209).
    Internal,
}

/// Build the storage key for a config row.
fn config_key(scope: &TaskScope, task_id: &str, config_id: &str) -> StateKey {
    StateKey::new(
        scope.namespace.clone(),
        scope.tenant.clone(),
        KeyKind::A2aTaskPushConfig,
        format!("{task_id}:{config_id}"),
    )
}

/// Confirm the task exists under this scope before any config
/// CRUD. Without this every endpoint would happily mint config rows
/// for a non-existent task — the existence check makes config IDs
/// unguessable for tasks the caller cannot see (the same shape the
/// rest of the A2A surface uses).
async fn ensure_task_exists(
    state: &AppState,
    scope: &TaskScope,
    task_id: &str,
) -> Result<(), PushConfigError> {
    let task_key = StateKey::new(
        scope.namespace.clone(),
        scope.tenant.clone(),
        KeyKind::A2aTask,
        task_id,
    );
    let store: Arc<dyn StateStore> = {
        let gw = state.gateway.read().await;
        gw.state_store().clone()
    };
    match store.get(&task_key).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(PushConfigError::TaskNotFound(task_id.to_string())),
        Err(e) => {
            tracing::warn!(error = %e, task_id, "push-config: task existence check failed");
            Err(PushConfigError::Internal)
        }
    }
}

/// Mint a new config id when the caller did not pre-allocate one.
fn fresh_config_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// Persist a new or updated config row. Caller-supplied `id` is
/// honoured (so a retried set is a clean upsert); when omitted a
/// fresh `UUIDv7` is minted. Returns the saved row with its timestamps
/// stamped.
pub(crate) async fn save_config(
    state: &AppState,
    scope: &TaskScope,
    task_id: &str,
    input: SetPushConfigInput,
) -> Result<TaskPushNotificationConfig, PushConfigError> {
    ensure_task_exists(state, scope, task_id).await?;
    let id = input.id.unwrap_or_else(fresh_config_id);
    let key = config_key(scope, task_id, &id);

    let store: Arc<dyn StateStore> = {
        let gw = state.gateway.read().await;
        gw.state_store().clone()
    };

    // Preserve the original `created_at` on update by reading the
    // existing row first. A missing row is the upsert-create path.
    let now = Utc::now();
    let created_at = match store.get(&key).await {
        Ok(Some(raw)) => {
            serde_json::from_str::<TaskPushNotificationConfig>(&raw).map_or(now, |c| c.created_at)
        }
        Ok(None) => now,
        Err(e) => {
            tracing::warn!(error = %e, "push-config: existing row read failed");
            return Err(PushConfigError::Internal);
        }
    };

    let config = TaskPushNotificationConfig {
        id: id.clone(),
        task_id: task_id.to_string(),
        namespace: scope.namespace.clone(),
        tenant: scope.tenant.clone(),
        url: input.url,
        token: input.token,
        authentication: input.authentication,
        created_at,
        updated_at: now,
    };
    config
        .validate()
        .map_err(|e| PushConfigError::Invalid(e.to_string()))?;
    // SSRF guard at registration: reject a config that literally
    // names an internal target (a private/loopback IP, a
    // cloud-metadata address, a `localhost`-style hostname) before
    // it is ever stored. The delivery worker re-checks with DNS
    // resolution — that is the authoritative guard against a
    // hostname that *resolves* into a blocked range — but rejecting
    // the obvious cases here gives the caller immediate feedback.
    super::a2a_ssrf::check_url_literal(&config.url)
        .map_err(|e| PushConfigError::Invalid(format!("push url rejected: {e}")))?;

    let payload = serde_json::to_string(&config).map_err(|e| {
        tracing::error!(error = %e, "push-config: serialize failed");
        PushConfigError::Internal
    })?;
    store.set(&key, &payload, None).await.map_err(|e| {
        tracing::warn!(error = %e, "push-config: state write failed");
        PushConfigError::Internal
    })?;
    Ok(config)
}

/// Load one config row.
pub(crate) async fn load_config(
    state: &AppState,
    scope: &TaskScope,
    task_id: &str,
    config_id: &str,
) -> Result<TaskPushNotificationConfig, PushConfigError> {
    ensure_task_exists(state, scope, task_id).await?;
    let key = config_key(scope, task_id, config_id);
    let store: Arc<dyn StateStore> = {
        let gw = state.gateway.read().await;
        gw.state_store().clone()
    };
    match store.get(&key).await {
        Ok(Some(raw)) => serde_json::from_str::<TaskPushNotificationConfig>(&raw).map_err(|e| {
            tracing::warn!(error = %e, "push-config: deserialize failed");
            PushConfigError::Internal
        }),
        Ok(None) => Err(PushConfigError::ConfigNotFound {
            task_id: task_id.to_string(),
            config_id: config_id.to_string(),
        }),
        Err(e) => {
            tracing::warn!(error = %e, "push-config: state read failed");
            Err(PushConfigError::Internal)
        }
    }
}

/// List every config row bound to one task. Returns an empty Vec when
/// the task exists but has no configs — distinguished from a missing
/// task by [`PushConfigError::TaskNotFound`].
pub(crate) async fn list_configs(
    state: &AppState,
    scope: &TaskScope,
    task_id: &str,
) -> Result<Vec<TaskPushNotificationConfig>, PushConfigError> {
    ensure_task_exists(state, scope, task_id).await?;
    let store: Arc<dyn StateStore> = {
        let gw = state.gateway.read().await;
        gw.state_store().clone()
    };
    let prefix = format!("{task_id}:");
    let entries = store
        .scan_keys(
            &scope.namespace,
            &scope.tenant,
            KeyKind::A2aTaskPushConfig,
            Some(&prefix),
        )
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "push-config: scan failed");
            PushConfigError::Internal
        })?;
    let mut out = Vec::with_capacity(entries.len());
    for (_, raw) in entries {
        if let Ok(c) = serde_json::from_str::<TaskPushNotificationConfig>(&raw) {
            out.push(c);
        }
    }
    Ok(out)
}

/// Delete one config row. Returns `Ok(())` only when the row existed
/// — a missing row is a `ConfigNotFound`, never a silent no-op.
pub(crate) async fn delete_config(
    state: &AppState,
    scope: &TaskScope,
    task_id: &str,
    config_id: &str,
) -> Result<(), PushConfigError> {
    ensure_task_exists(state, scope, task_id).await?;
    let key = config_key(scope, task_id, config_id);
    let store: Arc<dyn StateStore> = {
        let gw = state.gateway.read().await;
        gw.state_store().clone()
    };
    // Probe first — `delete` on a missing key is a no-op in the
    // memory store; we want an explicit 404 instead.
    let present = match store.get(&key).await {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(e) => {
            tracing::warn!(error = %e, "push-config: pre-delete probe failed");
            return Err(PushConfigError::Internal);
        }
    };
    if !present {
        return Err(PushConfigError::ConfigNotFound {
            task_id: task_id.to_string(),
            config_id: config_id.to_string(),
        });
    }
    store.delete(&key).await.map_err(|e| {
        tracing::warn!(error = %e, "push-config: delete failed");
        PushConfigError::Internal
    })?;
    Ok(())
}

// ---------------------------------------------------------------------
// Wire DTOs (shared by JSON-RPC params and REST bodies)
// ---------------------------------------------------------------------

/// Inbound shape for `set`. `id` is optional: present for an explicit
/// update, omitted to mint a new config. The DTO mirrors the spec's
/// `PushNotificationConfig` minus identity fields the URL / scope
/// already carry.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetPushConfigInput {
    /// Optional pre-allocated config id (`UUIDv7` by convention).
    #[serde(default)]
    pub id: Option<String>,
    /// Destination URL (`http`/`https`).
    pub url: String,
    /// Optional bearer token sent in `Authorization: Bearer …`.
    #[serde(default)]
    pub token: Option<String>,
    /// Optional richer authentication metadata.
    #[serde(default)]
    pub authentication: Option<PushAuthentication>,
}

// ---------------------------------------------------------------------
// REST handlers
// ---------------------------------------------------------------------

/// Body of `POST .../v1/tasks/{id}/pushNotificationConfigs`. Matches
/// [`SetPushConfigInput`] verbatim — the spec's REST and JSON-RPC
/// bodies share the same shape.
pub type SetPushConfigBody = SetPushConfigInput;

/// `POST /a2a/{ns}/{tenant}/v1/tasks/{id}/pushNotificationConfigs`.
pub async fn rest_set_push_config(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, task_id)): Path<(String, String, String)>,
    Json(body): Json<SetPushConfigBody>,
) -> Response {
    if let Some(resp) = guard(&identity, &namespace, &tenant) {
        return resp;
    }
    let scope = TaskScope::new(&namespace, &tenant);
    match save_config(&state, &scope, &task_id, body).await {
        Ok(cfg) => (StatusCode::OK, Json(cfg)).into_response(),
        Err(e) => render_rest_error(&e),
    }
}

/// `GET /a2a/{ns}/{tenant}/v1/tasks/{id}/pushNotificationConfigs`.
pub async fn rest_list_push_configs(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, task_id)): Path<(String, String, String)>,
) -> Response {
    if let Some(resp) = guard(&identity, &namespace, &tenant) {
        return resp;
    }
    let scope = TaskScope::new(&namespace, &tenant);
    match list_configs(&state, &scope, &task_id).await {
        Ok(configs) => (StatusCode::OK, Json(configs)).into_response(),
        Err(e) => render_rest_error(&e),
    }
}

/// `GET /a2a/{ns}/{tenant}/v1/tasks/{id}/pushNotificationConfigs/{cfgId}`.
pub async fn rest_get_push_config(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, task_id, config_id)): Path<(String, String, String, String)>,
) -> Response {
    if let Some(resp) = guard(&identity, &namespace, &tenant) {
        return resp;
    }
    let scope = TaskScope::new(&namespace, &tenant);
    match load_config(&state, &scope, &task_id, &config_id).await {
        Ok(cfg) => (StatusCode::OK, Json(cfg)).into_response(),
        Err(e) => render_rest_error(&e),
    }
}

/// `DELETE /a2a/{ns}/{tenant}/v1/tasks/{id}/pushNotificationConfigs/{cfgId}`.
pub async fn rest_delete_push_config(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, task_id, config_id)): Path<(String, String, String, String)>,
) -> Response {
    if let Some(resp) = guard(&identity, &namespace, &tenant) {
        return resp;
    }
    let scope = TaskScope::new(&namespace, &tenant);
    match delete_config(&state, &scope, &task_id, &config_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => render_rest_error(&e),
    }
}

/// Authorization shared by every REST handler. Mirrors the auth shape
/// of `a2a_rest_message_send` etc. — Dispatch permission + grant
/// covering `(tenant, namespace, "a2a", "rpc")`.
///
/// Returns `Some(response)` on rejection and `None` on success — the
/// reject value is built only when needed, keeping the hot path off
/// the large-`Err`-variant `Result` shape `clippy::result_large_err`
/// would flag.
fn guard(identity: &CallerIdentity, namespace: &str, tenant: &str) -> Option<Response> {
    if !identity.role.has_permission(Permission::Dispatch) {
        return Some(
            (
                StatusCode::FORBIDDEN,
                Json(ErrorResponse {
                    error: "a2a requires the dispatch permission (admin or operator role)"
                        .to_string(),
                }),
            )
                .into_response(),
        );
    }
    if !identity.is_authorized(tenant, namespace, "a2a", "rpc") {
        return Some(
            (
                StatusCode::FORBIDDEN,
                Json(ErrorResponse {
                    error: format!(
                        "forbidden: no grant covers tenant={tenant}, namespace={namespace}, provider=a2a"
                    ),
                }),
            )
                .into_response(),
        );
    }
    None
}

fn render_rest_error(err: &PushConfigError) -> Response {
    match err {
        PushConfigError::TaskNotFound(id) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("task '{id}' not found"),
            }),
        )
            .into_response(),
        PushConfigError::ConfigNotFound { task_id, config_id } => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("push config '{config_id}' not found on task '{task_id}'"),
            }),
        )
            .into_response(),
        PushConfigError::Invalid(msg) => (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse { error: msg.clone() }),
        )
            .into_response(),
        PushConfigError::Internal => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "internal error".to_string(),
            }),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------
// Dead-Letter Queue (operator surface)
//
// The DLQ is *not* part of the A2A protocol — it lives at
// `/v1/a2a/{ns}/{tenant}/push-dlq[/{id}]` under Acteon's operator
// namespace and uses the same Dispatch-permission + `(ns, tenant,
// a2a, dlq)` grant check as the rest of the operator surface. The
// REST list returns the most recent entries sorted descending by
// `last_failed_at` so a fresh failure surfaces first.
// ---------------------------------------------------------------------

/// Hard cap on the number of DLQ entries one list call returns.
/// Operator tooling that needs more should narrow the scope with
/// per-task listing once that's wired (follow-up).
const MAX_DLQ_LIST: usize = 500;

/// Storage key for one DLQ row.
fn dlq_key(scope: &TaskScope, task_id: &str, entry_id: &str) -> StateKey {
    StateKey::new(
        scope.namespace.clone(),
        scope.tenant.clone(),
        KeyKind::A2aPushDlq,
        format!("{task_id}:{entry_id}"),
    )
}

/// List every DLQ entry under (`namespace`, `tenant`), capped at
/// [`MAX_DLQ_LIST`]. The cap is enforced *after* the scan but
/// *before* sort so a tenant with > [`MAX_DLQ_LIST`] entries gets a
/// deterministic suffix rather than a random sample.
pub(crate) async fn list_dlq(
    state: &AppState,
    scope: &TaskScope,
) -> Result<Vec<acteon_core::PushDeliveryDlqEntry>, PushConfigError> {
    let store: Arc<dyn StateStore> = {
        let gw = state.gateway.read().await;
        gw.state_store().clone()
    };
    let entries = store
        .scan_keys(&scope.namespace, &scope.tenant, KeyKind::A2aPushDlq, None)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "push-dlq: scan failed");
            PushConfigError::Internal
        })?;
    let mut out: Vec<acteon_core::PushDeliveryDlqEntry> = Vec::with_capacity(entries.len());
    for (_, raw) in entries {
        if let Ok(e) = serde_json::from_str::<acteon_core::PushDeliveryDlqEntry>(&raw) {
            out.push(e);
        }
    }
    // Most-recent-failure first — the operator-friendly default.
    // `sort_by_key` with a reverse-ordered key keeps clippy happy
    // without an explicit comparator closure.
    out.sort_by_key(|e| std::cmp::Reverse(e.last_failed_at));
    out.truncate(MAX_DLQ_LIST);
    Ok(out)
}

/// Load one DLQ entry by id. Walks the full prefix-scan because the
/// `{task_id}:{entry_id}` key shape requires the `task_id` to address
/// the row directly; the operator endpoint only takes `entry_id` so
/// it can identify rows by UUID alone.
pub(crate) async fn load_dlq_entry(
    state: &AppState,
    scope: &TaskScope,
    entry_id: &str,
) -> Result<acteon_core::PushDeliveryDlqEntry, PushConfigError> {
    let store: Arc<dyn StateStore> = {
        let gw = state.gateway.read().await;
        gw.state_store().clone()
    };
    let entries = store
        .scan_keys(&scope.namespace, &scope.tenant, KeyKind::A2aPushDlq, None)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "push-dlq: scan failed");
            PushConfigError::Internal
        })?;
    let suffix = format!(":{entry_id}");
    for (key_id, raw) in entries {
        // Keys are stored as `{task_id}:{entry_id}`, so a suffix
        // match on `:entry_id` is unambiguous (entry ids are
        // UUIDv7 which don't contain `:`).
        if key_id.ends_with(&suffix)
            && let Ok(e) = serde_json::from_str::<acteon_core::PushDeliveryDlqEntry>(&raw)
        {
            return Ok(e);
        }
    }
    Err(PushConfigError::ConfigNotFound {
        task_id: String::new(),
        config_id: entry_id.to_string(),
    })
}

/// Delete one DLQ entry. Loads first to learn the `task_id` (the
/// caller only gave us `entry_id`); a missing entry yields
/// `ConfigNotFound` rather than a silent no-op.
pub(crate) async fn delete_dlq_entry(
    state: &AppState,
    scope: &TaskScope,
    entry_id: &str,
) -> Result<(), PushConfigError> {
    let row = load_dlq_entry(state, scope, entry_id).await?;
    let key = dlq_key(scope, &row.task_id, entry_id);
    let store: Arc<dyn StateStore> = {
        let gw = state.gateway.read().await;
        gw.state_store().clone()
    };
    store.delete(&key).await.map_err(|e| {
        tracing::warn!(error = %e, "push-dlq: delete failed");
        PushConfigError::Internal
    })?;
    Ok(())
}

/// `GET /v1/a2a/{namespace}/{tenant}/push-dlq` — list DLQ entries.
pub async fn rest_list_push_dlq(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant)): Path<(String, String)>,
) -> Response {
    if let Some(resp) = guard(&identity, &namespace, &tenant) {
        return resp;
    }
    let scope = TaskScope::new(&namespace, &tenant);
    match list_dlq(&state, &scope).await {
        Ok(rows) => (StatusCode::OK, Json(rows)).into_response(),
        Err(e) => render_rest_error(&e),
    }
}

/// `GET /v1/a2a/{namespace}/{tenant}/push-dlq/{entryId}` — read one.
pub async fn rest_get_push_dlq(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, entry_id)): Path<(String, String, String)>,
) -> Response {
    if let Some(resp) = guard(&identity, &namespace, &tenant) {
        return resp;
    }
    let scope = TaskScope::new(&namespace, &tenant);
    match load_dlq_entry(&state, &scope, &entry_id).await {
        Ok(row) => (StatusCode::OK, Json(row)).into_response(),
        Err(e) => render_rest_error(&e),
    }
}

/// `DELETE /v1/a2a/{namespace}/{tenant}/push-dlq/{entryId}`.
pub async fn rest_delete_push_dlq(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, entry_id)): Path<(String, String, String)>,
) -> Response {
    if let Some(resp) = guard(&identity, &namespace, &tenant) {
        return resp;
    }
    let scope = TaskScope::new(&namespace, &tenant);
    match delete_dlq_entry(&state, &scope, &entry_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => render_rest_error(&e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_key_uses_task_prefix() {
        let s = TaskScope::new("agents", "demo");
        let k = config_key(&s, "task-1", "cfg-a");
        assert_eq!(k.id, "task-1:cfg-a");
        assert_eq!(k.kind, KeyKind::A2aTaskPushConfig);
    }

    #[test]
    fn fresh_config_id_is_uuid_v7() {
        let id = fresh_config_id();
        // UUIDv7s parse and have the version-7 nibble in the right place.
        let parsed = uuid::Uuid::parse_str(&id).expect("uuid parse");
        assert_eq!(parsed.get_version_num(), 7, "UUIDv7 expected");
    }

    #[test]
    fn dlq_key_uses_task_then_entry_id_format() {
        let s = TaskScope::new("agents", "demo");
        let k = dlq_key(&s, "task-1", "entry-a");
        assert_eq!(k.id, "task-1:entry-a");
        assert_eq!(k.kind, KeyKind::A2aPushDlq);
    }
}
