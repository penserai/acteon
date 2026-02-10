//! Recurring actions API endpoints.
//!
//! CRUD operations and lifecycle management for cron-scheduled recurring actions.

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use acteon_audit::AuditRecord;
use acteon_core::{
    DEFAULT_MIN_INTERVAL_SECONDS, RecurringAction, RecurringActionTemplate, next_occurrence,
    validate_cron_expr, validate_min_interval, validate_timezone,
};
use acteon_state::{KeyKind, StateKey};

use super::AppState;
use super::schemas::ErrorResponse;

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

/// Request body for creating a recurring action.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateRecurringRequest {
    /// Human-readable name for this recurring action.
    #[schema(example = "daily-digest")]
    pub name: Option<String>,
    /// Namespace.
    #[schema(example = "notifications")]
    pub namespace: String,
    /// Tenant.
    #[schema(example = "tenant-1")]
    pub tenant: String,
    /// Target provider.
    #[schema(example = "email")]
    pub provider: String,
    /// Action type discriminator.
    #[schema(example = "send_digest")]
    pub action_type: String,
    /// JSON payload for the provider.
    #[schema(value_type = Object)]
    pub payload: serde_json::Value,
    /// Optional metadata labels merged into each dispatched action.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// Cron expression (standard 5-field).
    #[schema(example = "0 9 * * MON-FRI")]
    pub cron_expression: String,
    /// IANA timezone for the cron expression. Defaults to `"UTC"`.
    #[schema(example = "US/Eastern")]
    #[serde(default)]
    pub timezone: Option<String>,
    /// Optional end date (ISO 8601). The recurring action is auto-disabled after this.
    #[serde(default)]
    pub end_date: Option<DateTime<Utc>>,
    /// Optional maximum number of executions before auto-disabling.
    #[serde(default)]
    pub max_executions: Option<u64>,
    /// Human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// Optional dedup key template. Supports `{{recurring_id}}` and
    /// `{{execution_time}}` placeholders.
    #[serde(default)]
    pub dedup_key: Option<String>,
    /// Arbitrary key-value labels.
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

/// Request body for updating a recurring action.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateRecurringRequest {
    /// Namespace (required for key lookup).
    #[schema(example = "notifications")]
    pub namespace: String,
    /// Tenant (required for key lookup).
    #[schema(example = "tenant-1")]
    pub tenant: String,
    /// Updated name.
    #[serde(default)]
    pub name: Option<String>,
    /// Updated payload.
    #[serde(default)]
    #[schema(value_type = Option<Object>)]
    pub payload: Option<serde_json::Value>,
    /// Updated metadata.
    #[serde(default)]
    pub metadata: Option<HashMap<String, String>>,
    /// Updated cron expression.
    #[serde(default)]
    pub cron_expression: Option<String>,
    /// Updated timezone.
    #[serde(default)]
    pub timezone: Option<String>,
    /// Updated end date.
    #[serde(default)]
    pub end_date: Option<DateTime<Utc>>,
    /// Updated maximum executions.
    #[serde(default)]
    pub max_executions: Option<u64>,
    /// Updated description.
    #[serde(default)]
    pub description: Option<String>,
    /// Updated dedup key template.
    #[serde(default)]
    pub dedup_key: Option<String>,
    /// Updated labels.
    #[serde(default)]
    pub labels: Option<HashMap<String, String>>,
}

/// Namespace/tenant query parameters for recurring action endpoints.
#[derive(Debug, Deserialize, IntoParams)]
pub struct RecurringNamespaceParams {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
}

/// Query parameters for listing recurring actions.
#[derive(Debug, Deserialize, IntoParams)]
pub struct ListRecurringParams {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Optional status filter: `"active"`, `"paused"`.
    #[serde(default)]
    pub status: Option<String>,
    /// Maximum number of results (default: 100).
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Number of results to skip (default: 0).
    #[serde(default)]
    pub offset: usize,
}

fn default_limit() -> usize {
    100
}

/// Summary of a recurring action for list responses.
#[derive(Debug, Serialize, ToSchema)]
pub struct RecurringSummary {
    /// Unique recurring action ID.
    pub id: String,
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Cron expression.
    pub cron_expr: String,
    /// IANA timezone.
    pub timezone: String,
    /// Whether the action is currently enabled.
    pub enabled: bool,
    /// Target provider.
    pub provider: String,
    /// Action type.
    pub action_type: String,
    /// Next scheduled execution time.
    pub next_execution_at: Option<DateTime<Utc>>,
    /// Total execution count.
    pub execution_count: u64,
    /// Optional description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// When the recurring action was created.
    pub created_at: DateTime<Utc>,
}

/// Response for listing recurring actions.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListRecurringResponse {
    /// List of recurring action summaries.
    pub recurring_actions: Vec<RecurringSummary>,
    /// Total count of results returned.
    pub count: usize,
}

/// Full detail response for a single recurring action.
#[derive(Debug, Serialize, ToSchema)]
pub struct RecurringDetailResponse {
    /// Unique recurring action ID.
    pub id: String,
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Cron expression.
    pub cron_expr: String,
    /// IANA timezone.
    pub timezone: String,
    /// Whether the action is currently enabled.
    pub enabled: bool,
    /// Target provider.
    pub provider: String,
    /// Action type.
    pub action_type: String,
    /// JSON payload template.
    #[schema(value_type = Object)]
    pub payload: serde_json::Value,
    /// Metadata labels.
    pub metadata: HashMap<String, String>,
    /// Optional dedup key template.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dedup_key: Option<String>,
    /// Next scheduled execution time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_execution_at: Option<DateTime<Utc>>,
    /// Most recent execution time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_executed_at: Option<DateTime<Utc>>,
    /// Optional end date.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ends_at: Option<DateTime<Utc>>,
    /// Optional maximum number of executions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_executions: Option<u64>,
    /// Total execution count.
    pub execution_count: u64,
    /// Optional description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Arbitrary labels.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
    /// When the recurring action was created.
    pub created_at: DateTime<Utc>,
    /// When the recurring action was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Response returned after creating a recurring action.
#[derive(Debug, Serialize, ToSchema)]
pub struct CreateRecurringResponse {
    /// Assigned recurring action ID.
    pub id: String,
    /// Name (if provided).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// First scheduled execution time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_execution_at: Option<DateTime<Utc>>,
    /// Status: `"active"`.
    pub status: String,
}

/// Request body for pause/resume lifecycle operations.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RecurringLifecycleRequest {
    /// Namespace.
    #[schema(example = "notifications")]
    pub namespace: String,
    /// Tenant.
    #[schema(example = "tenant-1")]
    pub tenant: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn recurring_to_detail(rec: &RecurringAction) -> RecurringDetailResponse {
    RecurringDetailResponse {
        id: rec.id.clone(),
        namespace: rec.namespace.clone(),
        tenant: rec.tenant.clone(),
        cron_expr: rec.cron_expr.clone(),
        timezone: rec.timezone.clone(),
        enabled: rec.enabled,
        provider: rec.action_template.provider.clone(),
        action_type: rec.action_template.action_type.clone(),
        payload: rec.action_template.payload.clone(),
        metadata: rec.action_template.metadata.clone(),
        dedup_key: rec.action_template.dedup_key.clone(),
        next_execution_at: rec.next_execution_at,
        last_executed_at: rec.last_executed_at,
        ends_at: rec.ends_at,
        max_executions: rec.max_executions,
        execution_count: rec.execution_count,
        description: rec.description.clone(),
        labels: rec.labels.clone(),
        created_at: rec.created_at,
        updated_at: rec.updated_at,
    }
}

fn recurring_to_summary(rec: &RecurringAction) -> RecurringSummary {
    RecurringSummary {
        id: rec.id.clone(),
        namespace: rec.namespace.clone(),
        tenant: rec.tenant.clone(),
        cron_expr: rec.cron_expr.clone(),
        timezone: rec.timezone.clone(),
        enabled: rec.enabled,
        provider: rec.action_template.provider.clone(),
        action_type: rec.action_template.action_type.clone(),
        next_execution_at: rec.next_execution_at,
        execution_count: rec.execution_count,
        description: rec.description.clone(),
        created_at: rec.created_at,
    }
}

/// Load a [`RecurringAction`] from the state store by ID.
async fn load_recurring(
    state_store: &dyn acteon_state::StateStore,
    namespace: &str,
    tenant: &str,
    id: &str,
) -> Result<Option<RecurringAction>, String> {
    let key = StateKey::new(namespace, tenant, KeyKind::RecurringAction, id);
    match state_store.get(&key).await {
        Ok(Some(data)) => serde_json::from_str::<RecurringAction>(&data)
            .map(Some)
            .map_err(|e| format!("corrupt recurring action data: {e}")),
        Ok(None) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Persist a [`RecurringAction`] to the state store.
async fn save_recurring(
    state_store: &dyn acteon_state::StateStore,
    rec: &RecurringAction,
) -> Result<(), String> {
    let key = StateKey::new(
        rec.namespace.as_str(),
        rec.tenant.as_str(),
        KeyKind::RecurringAction,
        &rec.id,
    );
    let data = serde_json::to_string(rec).map_err(|e| format!("serialization error: {e}"))?;
    state_store
        .set(&key, &data, None)
        .await
        .map_err(|e| e.to_string())
}

/// Index a recurring action in the pending timeout index so the background
/// processor picks it up at the right time.
async fn index_pending(
    state_store: &dyn acteon_state::StateStore,
    rec: &RecurringAction,
) -> Result<(), String> {
    if let Some(next) = rec.next_execution_at {
        let pending_key = StateKey::new(
            rec.namespace.as_str(),
            rec.tenant.as_str(),
            KeyKind::PendingRecurring,
            &rec.id,
        );
        // Store the next execution timestamp as the value.
        state_store
            .set(&pending_key, &next.timestamp_millis().to_string(), None)
            .await
            .map_err(|e| e.to_string())?;
        state_store
            .index_timeout(&pending_key, next.timestamp_millis())
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Remove a recurring action from the pending timeout index.
async fn remove_pending(
    state_store: &dyn acteon_state::StateStore,
    namespace: &str,
    tenant: &str,
    id: &str,
) -> Result<(), String> {
    let pending_key = StateKey::new(namespace, tenant, KeyKind::PendingRecurring, id);
    // Best-effort removal; ignore errors if key doesn't exist.
    let _ = state_store.remove_timeout_index(&pending_key).await;
    let _ = state_store.delete(&pending_key).await;
    Ok(())
}

fn build_audit_record(
    recurring: &RecurringAction,
    operation: &str,
    caller_id: &str,
) -> AuditRecord {
    let now = Utc::now();
    let metadata = serde_json::json!({
        "recurring_id": recurring.id
    });

    AuditRecord {
        id: Uuid::now_v7().to_string(),
        action_id: Uuid::new_v4().to_string(), // New unique ID for this audit event
        chain_id: None,
        namespace: recurring.namespace.clone(),
        tenant: recurring.tenant.clone(),
        provider: "system".to_string(),
        action_type: format!("recurring.{operation}"),
        verdict: "allow".to_string(),
        matched_rule: None,
        outcome: "executed".to_string(),
        action_payload: Some(serde_json::to_value(recurring).unwrap_or_default()),
        verdict_details: serde_json::json!({}),
        outcome_details: serde_json::json!({}),
        metadata,
        dispatched_at: now,
        completed_at: now,
        duration_ms: 0,
        expires_at: None, // Use default TTL from config if needed, or let backend handle it
        caller_id: caller_id.to_string(),
        auth_method: "unknown".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Response helpers
// ---------------------------------------------------------------------------

/// Build a JSON error response with the given status code.
fn error_response(status: StatusCode, message: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!(ErrorResponse {
            error: message.to_owned(),
        })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

/// Parsed and validated cron input ready for use.
struct ValidatedCronInput {
    cron: croner::Cron,
    tz: chrono_tz::Tz,
    tz_str: String,
}

/// Validates the per-tenant recurring action limit. Returns an error response
/// if the limit is reached or if the check itself fails.
async fn check_tenant_limit(
    state_store: &dyn acteon_state::StateStore,
    namespace: &str,
    tenant: &str,
    max_actions: usize,
) -> Result<(), axum::response::Response> {
    match state_store
        .scan_keys(namespace, tenant, KeyKind::RecurringAction, None)
        .await
    {
        Ok(keys) if keys.len() >= max_actions => Err(error_response(
            StatusCode::TOO_MANY_REQUESTS,
            &format!("recurring action limit reached for tenant ({max_actions})"),
        )),
        Err(e) => Err(error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("failed to check recurring action limit: {e}"),
        )),
        _ => Ok(()),
    }
}

/// Validates cron expression, timezone, and minimum interval. Returns the
/// parsed cron and timezone on success.
fn validate_cron_input(
    cron_expression: &str,
    timezone: Option<&str>,
) -> Result<ValidatedCronInput, Box<axum::response::Response>> {
    let cron = validate_cron_expr(cron_expression)
        .map_err(|e| Box::new(error_response(StatusCode::BAD_REQUEST, &e.to_string())))?;

    let tz_str = timezone.unwrap_or("UTC");
    let tz = validate_timezone(tz_str)
        .map_err(|e| Box::new(error_response(StatusCode::BAD_REQUEST, &e.to_string())))?;

    validate_min_interval(&cron, tz, DEFAULT_MIN_INTERVAL_SECONDS)
        .map_err(|e| Box::new(error_response(StatusCode::BAD_REQUEST, &e.to_string())))?;

    Ok(ValidatedCronInput {
        cron,
        tz,
        tz_str: tz_str.to_owned(),
    })
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /v1/recurring` -- create a recurring action.
#[utoipa::path(
    post,
    path = "/v1/recurring",
    tag = "Recurring Actions",
    summary = "Create a recurring action",
    description = "Creates a new cron-scheduled recurring action. Validates the cron expression and timezone, computes the first execution time, and stores it.",
    request_body(content = CreateRecurringRequest, description = "Recurring action definition"),
    responses(
        (status = 201, description = "Recurring action created", body = CreateRecurringResponse),
        (status = 400, description = "Validation error", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn create_recurring(
    State(state): State<AppState>,
    Json(req): Json<CreateRecurringRequest>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    // Enforce per-tenant limit.
    let max_actions = state.config.background.max_recurring_actions_per_tenant;
    if let Err(resp) = check_tenant_limit(
        state_store.as_ref(),
        &req.namespace,
        &req.tenant,
        max_actions,
    )
    .await
    {
        return resp;
    }

    // Validate cron, timezone, and minimum interval.
    let validated = match validate_cron_input(&req.cron_expression, req.timezone.as_deref()) {
        Ok(v) => v,
        Err(resp) => return *resp,
    };

    let now = Utc::now();
    let first_execution = next_occurrence(&validated.cron, validated.tz, &now);
    let id = uuid::Uuid::new_v4().to_string();

    let recurring = RecurringAction {
        id: id.clone(),
        namespace: req.namespace.clone(),
        tenant: req.tenant.clone(),
        cron_expr: req.cron_expression,
        timezone: validated.tz_str,
        enabled: true,
        action_template: RecurringActionTemplate {
            provider: req.provider,
            action_type: req.action_type,
            payload: req.payload,
            metadata: req.metadata,
            dedup_key: req.dedup_key,
        },
        created_at: now,
        updated_at: now,
        last_executed_at: None,
        next_execution_at: first_execution,
        ends_at: req.end_date,
        max_executions: req.max_executions,
        execution_count: 0,
        description: req.description.or(req.name.clone()),
        labels: req.labels,
    };

    // Save and index.
    if let Err(e) = save_recurring(state_store.as_ref(), &recurring).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e);
    }
    if let Err(e) = index_pending(state_store.as_ref(), &recurring).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e);
    }

    // Record audit event.
    if let Some(audit) = &state.audit {
        let record = build_audit_record(&recurring, "create", "system");
        if let Err(e) = audit.record(record).await {
            tracing::warn!(error = %e, "audit recording failed");
        }
    }

    (
        StatusCode::CREATED,
        Json(serde_json::json!(CreateRecurringResponse {
            id,
            name: req.name,
            next_execution_at: first_execution,
            status: "active".to_owned(),
        })),
    )
        .into_response()
}

/// `GET /v1/recurring` -- list recurring actions.
#[utoipa::path(
    get,
    path = "/v1/recurring",
    tag = "Recurring Actions",
    summary = "List recurring actions",
    description = "Returns recurring actions filtered by namespace, tenant, and optional status.",
    params(ListRecurringParams),
    responses(
        (status = 200, description = "Recurring action list", body = ListRecurringResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn list_recurring(
    State(state): State<AppState>,
    Query(params): Query<ListRecurringParams>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    let results = match state_store
        .scan_keys(
            &params.namespace,
            &params.tenant,
            KeyKind::RecurringAction,
            None,
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse {
                    error: e.to_string(),
                })),
            )
                .into_response();
        }
    };

    let status_filter = params.status.as_deref();
    let mut actions: Vec<RecurringSummary> = Vec::new();
    let mut skipped = 0usize;

    for (_key, value) in results {
        let Ok(rec) = serde_json::from_str::<RecurringAction>(&value) else {
            continue;
        };

        // Apply status filter.
        if let Some(filter) = status_filter {
            let matches = match filter {
                "active" => rec.enabled,
                "paused" => !rec.enabled,
                _ => true,
            };
            if !matches {
                continue;
            }
        }

        // Offset-based pagination.
        if skipped < params.offset {
            skipped += 1;
            continue;
        }

        if actions.len() >= params.limit {
            break;
        }

        actions.push(recurring_to_summary(&rec));
    }

    let count = actions.len();
    (
        StatusCode::OK,
        Json(serde_json::json!(ListRecurringResponse {
            recurring_actions: actions,
            count,
        })),
    )
        .into_response()
}

/// `GET /v1/recurring/{id}` -- get recurring action details.
#[utoipa::path(
    get,
    path = "/v1/recurring/{id}",
    tag = "Recurring Actions",
    summary = "Get recurring action details",
    description = "Returns the full details of a recurring action.",
    params(
        ("id" = String, Path, description = "Recurring action ID"),
        RecurringNamespaceParams,
    ),
    responses(
        (status = 200, description = "Recurring action details", body = RecurringDetailResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn get_recurring(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<RecurringNamespaceParams>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    match load_recurring(state_store.as_ref(), &params.namespace, &params.tenant, &id).await {
        Ok(Some(rec)) => (
            StatusCode::OK,
            Json(serde_json::json!(recurring_to_detail(&rec))),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                error: format!("recurring action not found: {id}"),
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { error: e })),
        )
            .into_response(),
    }
}

/// `PUT /v1/recurring/{id}` -- update a recurring action.
#[allow(clippy::too_many_lines)]
#[utoipa::path(
    put,
    path = "/v1/recurring/{id}",
    tag = "Recurring Actions",
    summary = "Update a recurring action",
    description = "Updates fields of an existing recurring action. Recalculates the next execution time if the cron expression or timezone changes.",
    params(("id" = String, Path, description = "Recurring action ID")),
    request_body(content = UpdateRecurringRequest, description = "Fields to update"),
    responses(
        (status = 200, description = "Updated recurring action", body = RecurringDetailResponse),
        (status = 400, description = "Validation error", body = ErrorResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
#[allow(clippy::similar_names)]
pub async fn update_recurring(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateRecurringRequest>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    let mut rec = match load_recurring(state_store.as_ref(), &req.namespace, &req.tenant, &id).await
    {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!(ErrorResponse {
                    error: format!("recurring action not found: {id}"),
                })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse { error: e })),
            )
                .into_response();
        }
    };

    let mut cron_changed = false;

    // Apply updates.
    if let Some(name) = req.name {
        rec.description = Some(name);
    }
    if let Some(desc) = req.description {
        rec.description = Some(desc);
    }
    if let Some(payload) = req.payload {
        rec.action_template.payload = payload;
    }
    if let Some(metadata) = req.metadata {
        rec.action_template.metadata = metadata;
    }
    if let Some(cron_expr) = req.cron_expression {
        // Validate the new cron expression.
        if let Err(e) = validate_cron_expr(&cron_expr) {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!(ErrorResponse {
                    error: e.to_string(),
                })),
            )
                .into_response();
        }
        rec.cron_expr = cron_expr;
        cron_changed = true;
    }
    if let Some(tz) = req.timezone {
        if let Err(e) = validate_timezone(&tz) {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!(ErrorResponse {
                    error: e.to_string(),
                })),
            )
                .into_response();
        }
        rec.timezone = tz;
        cron_changed = true;
    }
    if let Some(end_date) = req.end_date {
        rec.ends_at = Some(end_date);
    }
    if let Some(max) = req.max_executions {
        rec.max_executions = Some(max);
    }
    if let Some(dedup_key) = req.dedup_key {
        rec.action_template.dedup_key = Some(dedup_key);
    }
    if let Some(labels) = req.labels {
        rec.labels = labels;
    }

    // Recalculate next execution if cron/timezone changed.
    if cron_changed && rec.enabled {
        let cron = match validate_cron_expr(&rec.cron_expr) {
            Ok(c) => c,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!(ErrorResponse {
                        error: e.to_string(),
                    })),
                )
                    .into_response();
            }
        };
        let tz = match validate_timezone(&rec.timezone) {
            Ok(t) => t,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!(ErrorResponse {
                        error: e.to_string(),
                    })),
                )
                    .into_response();
            }
        };

        if let Err(e) = validate_min_interval(&cron, tz, DEFAULT_MIN_INTERVAL_SECONDS) {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!(ErrorResponse {
                    error: e.to_string(),
                })),
            )
                .into_response();
        }

        rec.next_execution_at = next_occurrence(&cron, tz, &Utc::now());
    }

    rec.updated_at = Utc::now();

    // Save and re-index.
    if let Err(e) = save_recurring(state_store.as_ref(), &rec).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { error: e })),
        )
            .into_response();
    }

    if cron_changed && rec.enabled {
        let _ = remove_pending(state_store.as_ref(), &rec.namespace, &rec.tenant, &rec.id).await;
        if let Err(e) = index_pending(state_store.as_ref(), &rec).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse { error: e })),
            )
                .into_response();
        }
    }

    // Record audit event.
    if let Some(audit) = &state.audit {
        let record = build_audit_record(&rec, "update", "system");
        if let Err(e) = audit.record(record).await {
            tracing::warn!(error = %e, "audit recording failed");
        }
    }

    (
        StatusCode::OK,
        Json(serde_json::json!(recurring_to_detail(&rec))),
    )
        .into_response()
}

/// `DELETE /v1/recurring/{id}` -- delete a recurring action.
#[utoipa::path(
    delete,
    path = "/v1/recurring/{id}",
    tag = "Recurring Actions",
    summary = "Delete a recurring action",
    description = "Removes a recurring action from the state store and timeout index.",
    params(
        ("id" = String, Path, description = "Recurring action ID"),
        RecurringNamespaceParams,
    ),
    responses(
        (status = 204, description = "Recurring action deleted"),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn delete_recurring(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<RecurringNamespaceParams>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    // Verify the recurring action exists.
    let rec =
        match load_recurring(state_store.as_ref(), &params.namespace, &params.tenant, &id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!(ErrorResponse {
                        error: format!("recurring action not found: {id}"),
                    })),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!(ErrorResponse { error: e })),
                )
                    .into_response();
            }
        };

    // Remove from timeout index.
    let _ = remove_pending(state_store.as_ref(), &params.namespace, &params.tenant, &id).await;

    // Remove from state store.
    let key = StateKey::new(
        params.namespace.as_str(),
        params.tenant.as_str(),
        KeyKind::RecurringAction,
        &id,
    );
    if let Err(e) = state_store.delete(&key).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse {
                error: e.to_string(),
            })),
        )
            .into_response();
    }

    // Record audit event.
    if let Some(audit) = &state.audit {
        let record = build_audit_record(&rec, "delete", "system");
        if let Err(e) = audit.record(record).await {
            tracing::warn!(error = %e, "audit recording failed");
        }
    }

    StatusCode::NO_CONTENT.into_response()
}

/// `POST /v1/recurring/{id}/pause` -- pause a recurring action.
#[utoipa::path(
    post,
    path = "/v1/recurring/{id}/pause",
    tag = "Recurring Actions",
    summary = "Pause a recurring action",
    description = "Pauses a recurring action, removing it from the timeout index.",
    params(("id" = String, Path, description = "Recurring action ID")),
    request_body(content = RecurringLifecycleRequest, description = "Namespace and tenant"),
    responses(
        (status = 200, description = "Recurring action paused", body = RecurringDetailResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 409, description = "Already paused", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
#[allow(clippy::similar_names)]
pub async fn pause_recurring(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RecurringLifecycleRequest>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    let mut rec = match load_recurring(state_store.as_ref(), &req.namespace, &req.tenant, &id).await
    {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!(ErrorResponse {
                    error: format!("recurring action not found: {id}"),
                })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse { error: e })),
            )
                .into_response();
        }
    };

    if !rec.enabled {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!(ErrorResponse {
                error: "recurring action is already paused".to_owned(),
            })),
        )
            .into_response();
    }

    rec.enabled = false;
    rec.next_execution_at = None;
    rec.updated_at = Utc::now();

    if let Err(e) = save_recurring(state_store.as_ref(), &rec).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { error: e })),
        )
            .into_response();
    }

    // Remove from timeout index.
    let _ = remove_pending(state_store.as_ref(), &rec.namespace, &rec.tenant, &rec.id).await;

    // Record audit event.
    if let Some(audit) = &state.audit {
        let record = build_audit_record(&rec, "pause", "system");
        if let Err(e) = audit.record(record).await {
            tracing::warn!(error = %e, "audit recording failed");
        }
    }

    (
        StatusCode::OK,
        Json(serde_json::json!(recurring_to_detail(&rec))),
    )
        .into_response()
}

/// `POST /v1/recurring/{id}/resume` -- resume a paused recurring action.
#[utoipa::path(
    post,
    path = "/v1/recurring/{id}/resume",
    tag = "Recurring Actions",
    summary = "Resume a paused recurring action",
    description = "Resumes a paused recurring action, recalculating the next execution time and re-indexing in the timeout store.",
    params(("id" = String, Path, description = "Recurring action ID")),
    request_body(content = RecurringLifecycleRequest, description = "Namespace and tenant"),
    responses(
        (status = 200, description = "Recurring action resumed", body = RecurringDetailResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 409, description = "Already active", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
#[allow(clippy::similar_names)]
pub async fn resume_recurring(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RecurringLifecycleRequest>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    let mut rec = match load_recurring(state_store.as_ref(), &req.namespace, &req.tenant, &id).await
    {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!(ErrorResponse {
                    error: format!("recurring action not found: {id}"),
                })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse { error: e })),
            )
                .into_response();
        }
    };

    if rec.enabled {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!(ErrorResponse {
                error: "recurring action is already active".to_owned(),
            })),
        )
            .into_response();
    }

    // Recalculate next execution.
    let cron = match validate_cron_expr(&rec.cron_expr) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse {
                    error: format!("stored cron expression is invalid: {e}"),
                })),
            )
                .into_response();
        }
    };
    let tz = match validate_timezone(&rec.timezone) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse {
                    error: format!("stored timezone is invalid: {e}"),
                })),
            )
                .into_response();
        }
    };

    let now = Utc::now();
    rec.enabled = true;
    rec.next_execution_at = next_occurrence(&cron, tz, &now);
    rec.updated_at = now;

    if let Err(e) = save_recurring(state_store.as_ref(), &rec).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { error: e })),
        )
            .into_response();
    }

    // Re-index in timeout store.
    if let Err(e) = index_pending(state_store.as_ref(), &rec).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { error: e })),
        )
            .into_response();
    }

    // Record audit event.
    if let Some(audit) = &state.audit {
        let record = build_audit_record(&rec, "resume", "system");
        if let Err(e) = audit.record(record).await {
            tracing::warn!(error = %e, "audit recording failed");
        }
    }

    (
        StatusCode::OK,
        Json(serde_json::json!(recurring_to_detail(&rec))),
    )
        .into_response()
}
