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

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use acteon_core::{PushAuthentication, TaskPushNotificationConfig};
use acteon_gateway::{PushDeadLetterResolution, TaskEngine, TaskEngineError, TaskScope};
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

fn validate_stored_config(
    config: TaskPushNotificationConfig,
    scope: &TaskScope,
    task_id: &str,
    config_id: Option<&str>,
) -> Result<TaskPushNotificationConfig, PushConfigError> {
    if config.namespace != scope.namespace
        || config.tenant != scope.tenant
        || config.task_id != task_id
        || config_id.is_some_and(|id| config.id != id)
        || config.validate().is_err()
        || super::a2a_ssrf::check_url_literal(&config.url).is_err()
    {
        tracing::warn!(
            task_id,
            "push-config: stored row failed identity or validation"
        );
        return Err(PushConfigError::Internal);
    }
    Ok(config)
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
    match acteon_gateway::agent_runtime::is_governed_agent_task(
        store.as_ref(),
        &scope.namespace,
        &scope.tenant,
        task_id,
    )
    .await
    {
        Ok(true) => return Err(PushConfigError::TaskNotFound(task_id.into())),
        Err(_) => return Err(PushConfigError::Internal),
        Ok(false) => {}
    }
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

fn map_engine_error(error: TaskEngineError, task_id: &str) -> PushConfigError {
    match error {
        TaskEngineError::NotFound(_) => PushConfigError::TaskNotFound(task_id.into()),
        TaskEngineError::PushDeadLetterNotFound(entry_id) => PushConfigError::ConfigNotFound {
            task_id: task_id.into(),
            config_id: entry_id,
        },
        TaskEngineError::PushConfigCapacityExceeded
        | TaskEngineError::PushConfigDeliveryInFlight => {
            PushConfigError::Invalid(error.to_string())
        }
        other => {
            tracing::warn!(error = %other, task_id, "push-config: task envelope operation failed");
            PushConfigError::Internal
        }
    }
}

async fn envelope_engine(state: &AppState) -> (TaskEngine, Arc<dyn StateStore>) {
    let gateway = state.gateway.read().await;
    let store = gateway.state_store().clone();
    let mut engine = TaskEngine::new(store.clone()).with_clock(gateway.clock());
    if let Some(audit) = gateway.audit_store() {
        engine = engine.with_audit(audit);
    }
    (engine, store)
}

/// Import standalone rows into the task envelope exactly once. Rows remain in
/// place during the mixed-version window so reviewed cutover can distinguish
/// pre-envelope updates and deletes from configurations created natively in the
/// envelope. The phase-5 cutover deletes them only after reconciliation.
async fn migrate_legacy_configs(
    state: &AppState,
    scope: &TaskScope,
    task_id: &str,
) -> Result<TaskEngine, PushConfigError> {
    let (engine, store) = envelope_engine(state).await;
    let cutover_complete = crate::a2a_push_cutover::a2a_push_cutover_complete(store.as_ref())
        .await
        .map_err(|error| {
            tracing::warn!(%error, task_id, "push-config: cutover marker check failed");
            PushConfigError::Internal
        })?;
    if cutover_complete {
        let prefix = format!("{task_id}:");
        let legacy = store
            .scan_keys(
                &scope.namespace,
                &scope.tenant,
                KeyKind::A2aTaskPushConfig,
                Some(&prefix),
            )
            .await
            .map_err(|error| {
                tracing::warn!(%error, task_id, "push-config: post-cutover legacy scan failed");
                PushConfigError::Internal
            })?;
        if !legacy.is_empty() {
            tracing::error!(
                task_id,
                rows = legacy.len(),
                "push-config: post-cutover legacy rows detected"
            );
            return Err(PushConfigError::Internal);
        }
        return Ok(engine);
    }
    if engine
        .push_config_migration_complete(scope, task_id)
        .await
        .map_err(|error| map_engine_error(error, task_id))?
    {
        return Ok(engine);
    }
    let prefix = format!("{task_id}:");
    let entries = store
        .scan_keys(
            &scope.namespace,
            &scope.tenant,
            KeyKind::A2aTaskPushConfig,
            Some(&prefix),
        )
        .await
        .map_err(|error| {
            tracing::warn!(%error, task_id, "push-config: legacy scan failed");
            PushConfigError::Internal
        })?;
    let mut configs = Vec::with_capacity(entries.len());
    for (key, raw) in &entries {
        let config = serde_json::from_str::<TaskPushNotificationConfig>(raw).map_err(|error| {
            tracing::warn!(%error, task_id, "push-config: malformed legacy row");
            PushConfigError::Internal
        })?;
        let config = validate_stored_config(config, scope, task_id, None)?;
        if *key != config_key(scope, task_id, &config.id).canonical() {
            tracing::warn!(%key, task_id, config_id = %config.id, "push-config: legacy row identity mismatch");
            return Err(PushConfigError::Internal);
        }
        configs.push(config);
    }
    engine
        .import_legacy_push_configs(scope, task_id, configs)
        .await
        .map_err(|error| map_engine_error(error, task_id))?;
    Ok(engine)
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
    save_authorized_config(state, scope, task_id, input).await
}

/// Persist a push configuration after the caller has independently proved
/// access to the exact task. Governed agent services use this entry point
/// because the tenant task adapter deliberately hides governed tasks.
pub(crate) async fn save_authorized_config(
    state: &AppState,
    scope: &TaskScope,
    task_id: &str,
    input: SetPushConfigInput,
) -> Result<TaskPushNotificationConfig, PushConfigError> {
    let id = input.id.unwrap_or_else(fresh_config_id);
    let engine = migrate_legacy_configs(state, scope, task_id).await?;

    // Preserve the original `created_at` on update. The TaskEngine repeats
    // this rule inside CAS so a concurrent updater cannot reset it.
    let now = engine.clock().now();
    let created_at = engine
        .get_push_config(scope, task_id, &id)
        .await
        .map_err(|error| map_engine_error(error, task_id))?
        .map_or(now, |config| config.created_at);

    let config = TaskPushNotificationConfig {
        id: id.clone(),
        task_id: task_id.to_string(),
        namespace: scope.namespace.clone(),
        tenant: scope.tenant.clone(),
        url: input.url,
        token: input.token,
        token_ref: input.token_ref,
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

    engine
        .upsert_push_config(scope, task_id, config)
        .await
        .map_err(|error| map_engine_error(error, task_id))
}

/// Load one config row.
pub(crate) async fn load_config(
    state: &AppState,
    scope: &TaskScope,
    task_id: &str,
    config_id: &str,
) -> Result<TaskPushNotificationConfig, PushConfigError> {
    ensure_task_exists(state, scope, task_id).await?;
    load_authorized_config(state, scope, task_id, config_id).await
}

/// Load a push configuration after exact-task authorization.
pub(crate) async fn load_authorized_config(
    state: &AppState,
    scope: &TaskScope,
    task_id: &str,
    config_id: &str,
) -> Result<TaskPushNotificationConfig, PushConfigError> {
    let engine = migrate_legacy_configs(state, scope, task_id).await?;
    engine
        .get_push_config(scope, task_id, config_id)
        .await
        .map_err(|error| map_engine_error(error, task_id))?
        .ok_or_else(|| PushConfigError::ConfigNotFound {
            task_id: task_id.to_string(),
            config_id: config_id.to_string(),
        })
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
    list_authorized_configs(state, scope, task_id).await
}

/// List push configurations after exact-task authorization.
pub(crate) async fn list_authorized_configs(
    state: &AppState,
    scope: &TaskScope,
    task_id: &str,
) -> Result<Vec<TaskPushNotificationConfig>, PushConfigError> {
    let engine = migrate_legacy_configs(state, scope, task_id).await?;
    engine
        .list_push_configs(scope, task_id)
        .await
        .map_err(|error| map_engine_error(error, task_id))
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
    delete_authorized_config(state, scope, task_id, config_id).await
}

/// Delete a push configuration after exact-task authorization.
pub(crate) async fn delete_authorized_config(
    state: &AppState,
    scope: &TaskScope,
    task_id: &str,
    config_id: &str,
) -> Result<(), PushConfigError> {
    let engine = migrate_legacy_configs(state, scope, task_id).await?;
    if !engine
        .delete_push_config(scope, task_id, config_id)
        .await
        .map_err(|error| map_engine_error(error, task_id))?
    {
        return Err(PushConfigError::ConfigNotFound {
            task_id: task_id.to_string(),
            config_id: config_id.to_string(),
        });
    }
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
    /// Optional host-owned token reference. Mutually exclusive with `token`.
    #[serde(default)]
    pub token_ref: Option<acteon_core::SecretRef>,
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
    crate::auth::context::ScopedPath((namespace, tenant, task_id)): crate::auth::context::ScopedPath<(String, String, String)>,
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
    crate::auth::context::ScopedPath((namespace, tenant, task_id)): crate::auth::context::ScopedPath<(String, String, String)>,
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
    crate::auth::context::ScopedPath((namespace, tenant, task_id, config_id)): crate::auth::context::ScopedPath<(String, String, String, String)>,
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
    crate::auth::context::ScopedPath((namespace, tenant, task_id, config_id)): crate::auth::context::ScopedPath<(String, String, String, String)>,
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

/// List every Task-embedded dead letter under (`namespace`, `tenant`), plus
/// legacy standalone rows that have not yet been drained. The cap is enforced
/// after a newest-first sort.
async fn collect_dlq(
    state: &AppState,
    scope: &TaskScope,
) -> Result<Vec<acteon_core::PushDeliveryDlqEntry>, PushConfigError> {
    let (engine, store) = envelope_engine(state).await;
    let task_entries = store
        .scan_keys(&scope.namespace, &scope.tenant, KeyKind::A2aTask, None)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "push-dlq: task scan failed");
            PushConfigError::Internal
        })?;
    let task_prefix = format!("{}:{}:{}:", scope.namespace, scope.tenant, KeyKind::A2aTask);
    let mut out = Vec::new();
    for (key, _) in task_entries {
        let Some(task_id) = key.strip_prefix(&task_prefix) else {
            tracing::warn!(%key, "push-dlq: task scan returned an invalid key");
            return Err(PushConfigError::Internal);
        };
        let mut rows = engine
            .list_push_dead_letters(scope, task_id)
            .await
            .map_err(|error| map_engine_error(error, task_id))?;
        out.append(&mut rows);
    }

    // Mixed-version upgrade compatibility: old workers may have written
    // standalone rows before the Task-envelope worker became authoritative.
    let legacy_entries = store
        .scan_keys(&scope.namespace, &scope.tenant, KeyKind::A2aPushDlq, None)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "push-dlq: scan failed");
            PushConfigError::Internal
        })?;
    if !legacy_entries.is_empty()
        && crate::a2a_push_cutover::a2a_push_cutover_complete(store.as_ref())
            .await
            .map_err(|error| {
                tracing::warn!(%error, "push-dlq: cutover marker check failed");
                PushConfigError::Internal
            })?
    {
        tracing::error!(
            rows = legacy_entries.len(),
            "push-dlq: post-cutover legacy rows detected"
        );
        return Err(PushConfigError::Internal);
    }
    let mut legacy_by_task = BTreeMap::<String, Vec<acteon_core::PushDeliveryDlqEntry>>::new();
    for (key, raw) in legacy_entries {
        let entry =
            serde_json::from_str::<acteon_core::PushDeliveryDlqEntry>(&raw).map_err(|error| {
                tracing::warn!(%key, %error, "push-dlq: malformed legacy row");
                PushConfigError::Internal
            })?;
        let expected = dlq_key(scope, &entry.task_id, &entry.id).canonical();
        if key != expected
            || entry.namespace != scope.namespace
            || entry.tenant != scope.tenant
            || entry.sequence.is_some()
            || entry.event_id.is_some()
        {
            tracing::warn!(%key, "push-dlq: legacy row identity mismatch");
            return Err(PushConfigError::Internal);
        }
        legacy_by_task
            .entry(entry.task_id.clone())
            .or_default()
            .push(entry);
    }
    for (task_id, entries) in legacy_by_task {
        engine
            .import_legacy_push_dead_letters(scope, &task_id, entries.clone())
            .await
            .map_err(|error| map_engine_error(error, &task_id))?;
        for entry in &entries {
            let key = dlq_key(scope, &task_id, &entry.id);
            if let Err(error) = store.delete(&key).await {
                tracing::warn!(%error, task_id, entry_id = %entry.id, "push-dlq: legacy cleanup deferred");
            }
        }
        out.extend(entries);
    }
    let mut unique = BTreeMap::<String, acteon_core::PushDeliveryDlqEntry>::new();
    for entry in out {
        match unique.entry(entry.id.clone()) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(entry);
            }
            std::collections::btree_map::Entry::Occupied(slot) if slot.get() == &entry => {}
            std::collections::btree_map::Entry::Occupied(_) => {
                tracing::warn!(entry_id = %entry.id, "push-dlq: duplicate ID has conflicting evidence");
                return Err(PushConfigError::Internal);
            }
        }
    }
    let mut out = unique.into_values().collect::<Vec<_>>();
    // Most-recent-failure first — the operator-friendly default.
    // `sort_by_key` with a reverse-ordered key keeps clippy happy
    // without an explicit comparator closure.
    out.sort_by_key(|e| std::cmp::Reverse(e.last_failed_at));
    Ok(out)
}

pub(crate) async fn list_dlq(
    state: &AppState,
    scope: &TaskScope,
) -> Result<Vec<acteon_core::PushDeliveryDlqEntry>, PushConfigError> {
    let mut out = collect_dlq(state, scope).await?;
    out.truncate(MAX_DLQ_LIST);
    Ok(out)
}

/// Load one dead letter by globally unique UUID within the tenant scope.
pub(crate) async fn load_dlq_entry(
    state: &AppState,
    scope: &TaskScope,
    entry_id: &str,
) -> Result<acteon_core::PushDeliveryDlqEntry, PushConfigError> {
    if let Some(entry) = collect_dlq(state, scope)
        .await?
        .into_iter()
        .find(|entry| entry.id == entry_id)
    {
        return Ok(entry);
    }
    Err(PushConfigError::ConfigNotFound {
        task_id: String::new(),
        config_id: entry_id.to_string(),
    })
}

/// Discard one failed durable delivery and atomically unblock its callback.
/// Legacy standalone rows retain their historical row-delete behavior.
pub(crate) async fn delete_dlq_entry(
    state: &AppState,
    scope: &TaskScope,
    entry_id: &str,
) -> Result<(), PushConfigError> {
    let row = load_dlq_entry(state, scope, entry_id).await?;
    let (engine, store) = envelope_engine(state).await;
    match engine
        .resolve_push_dead_letter(
            scope,
            &row.task_id,
            entry_id,
            PushDeadLetterResolution::Discard,
        )
        .await
    {
        Ok(_) => return Ok(()),
        Err(TaskEngineError::PushDeadLetterNotFound(_)) if row.sequence.is_none() => {}
        Err(error) => return Err(map_engine_error(error, &row.task_id)),
    }
    let key = dlq_key(scope, &row.task_id, entry_id);
    store.delete(&key).await.map_err(|e| {
        tracing::warn!(error = %e, "push-dlq: delete failed");
        PushConfigError::Internal
    })?;
    Ok(())
}

/// `GET /v1/a2a/push-dlq` — list DLQ entries.
pub async fn rest_list_push_dlq(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    crate::auth::context::ScopedPath((namespace, tenant)): crate::auth::context::ScopedPath<(
        String,
        String,
    )>,
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

/// `GET /v1/a2a/push-dlq/{entryId}` — read one.
pub async fn rest_get_push_dlq(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    crate::auth::context::ScopedPath((namespace, tenant, entry_id)): crate::auth::context::ScopedPath<(String, String, String)>,
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

/// `DELETE /v1/a2a/push-dlq/{entryId}`.
pub async fn rest_delete_push_dlq(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    crate::auth::context::ScopedPath((namespace, tenant, entry_id)): crate::auth::context::ScopedPath<(String, String, String)>,
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
