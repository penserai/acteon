//! Quota policy API endpoints.
//!
//! CRUD operations and usage queries for tenant quota policies.

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use acteon_core::{
    OverageBehavior, QuotaPolicy, QuotaUsage, QuotaWindow, compute_window_boundaries,
    quota_counter_key,
};
use acteon_state::{KeyKind, StateKey};

use super::AppState;
use super::schemas::ErrorResponse;

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

/// Request body for creating a quota policy.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateQuotaRequest {
    /// Namespace this quota applies to.
    #[schema(example = "notifications")]
    pub namespace: String,
    /// Tenant this quota applies to.
    #[schema(example = "tenant-1")]
    pub tenant: String,
    /// Maximum number of actions allowed per window.
    #[schema(example = 1000)]
    pub max_actions: u64,
    /// Time window: `"hourly"`, `"daily"`, `"weekly"`, `"monthly"`, or a custom
    /// number of seconds (as an integer).
    #[schema(example = "daily")]
    pub window: String,
    /// Behavior when the quota is exceeded.
    pub overage_behavior: OverageBehavior,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// Arbitrary key-value labels.
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

/// Request body for updating a quota policy.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateQuotaRequest {
    /// Updated maximum actions per window.
    #[serde(default)]
    pub max_actions: Option<u64>,
    /// Updated time window.
    #[serde(default)]
    pub window: Option<String>,
    /// Updated overage behavior.
    #[serde(default)]
    pub overage_behavior: Option<OverageBehavior>,
    /// Updated enabled state.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Updated description.
    #[serde(default)]
    pub description: Option<String>,
    /// Updated labels.
    #[serde(default)]
    pub labels: Option<HashMap<String, String>>,
}

/// Full quota policy response (includes optional current usage).
#[derive(Debug, Serialize, ToSchema)]
pub struct QuotaResponse {
    /// Unique quota policy ID.
    pub id: String,
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Maximum actions per window.
    pub max_actions: u64,
    /// Time window.
    pub window: QuotaWindow,
    /// Overage behavior.
    pub overage_behavior: OverageBehavior,
    /// Whether this quota is currently active.
    pub enabled: bool,
    /// When the quota was created.
    pub created_at: DateTime<Utc>,
    /// When the quota was last updated.
    pub updated_at: DateTime<Utc>,
    /// Optional description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Arbitrary labels.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
    /// Current usage (populated on single-get requests).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<QuotaUsageResponse>,
}

/// Current usage snapshot for a quota policy.
#[derive(Debug, Serialize, ToSchema)]
pub struct QuotaUsageResponse {
    /// Actions used in the current window.
    pub used: u64,
    /// Maximum actions allowed.
    pub limit: u64,
    /// Remaining actions.
    pub remaining: u64,
    /// When the current window resets.
    pub resets_at: DateTime<Utc>,
}

/// Response for listing quota policies.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListQuotasResponse {
    /// List of quota policies.
    pub quotas: Vec<QuotaResponse>,
    /// Total count of results returned.
    pub count: usize,
}

/// Query parameters for listing quota policies.
#[derive(Debug, Deserialize, IntoParams)]
pub struct ListQuotasParams {
    /// Filter by namespace (optional).
    #[serde(default)]
    pub namespace: Option<String>,
    /// Filter by tenant (optional).
    #[serde(default)]
    pub tenant: Option<String>,
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a window string into a [`QuotaWindow`].
fn parse_window(s: &str) -> Result<QuotaWindow, String> {
    match s {
        "hourly" => Ok(QuotaWindow::Hourly),
        "daily" => Ok(QuotaWindow::Daily),
        "weekly" => Ok(QuotaWindow::Weekly),
        "monthly" => Ok(QuotaWindow::Monthly),
        other => other
            .parse::<u64>()
            .map_err(|_| format!("invalid window: {other} (expected hourly/daily/weekly/monthly or seconds as integer)"))
            .and_then(|seconds| {
                if seconds == 0 {
                    Err("invalid window: custom seconds must be greater than 0".to_string())
                } else {
                    Ok(QuotaWindow::Custom { seconds })
                }
            }),
    }
}

/// Build a [`QuotaResponse`] from a [`QuotaPolicy`] with optional usage.
fn policy_to_response(policy: &QuotaPolicy, usage: Option<QuotaUsageResponse>) -> QuotaResponse {
    QuotaResponse {
        id: policy.id.clone(),
        namespace: policy.namespace.clone(),
        tenant: policy.tenant.clone(),
        max_actions: policy.max_actions,
        window: policy.window.clone(),
        overage_behavior: policy.overage_behavior.clone(),
        enabled: policy.enabled,
        created_at: policy.created_at,
        updated_at: policy.updated_at,
        description: policy.description.clone(),
        labels: policy.labels.clone(),
        usage,
    }
}

/// Well-known namespace used for quota policy storage keys.
const QUOTA_STORE_NS: &str = "_system";
/// Well-known tenant used for quota policy storage keys.
const QUOTA_STORE_TENANT: &str = "_quotas";

/// Build a [`StateKey`] for a quota policy by its ID.
fn quota_state_key(id: &str) -> StateKey {
    StateKey::new(QUOTA_STORE_NS, QUOTA_STORE_TENANT, KeyKind::Quota, id)
}

/// Load a [`QuotaPolicy`] from the state store by ID via direct key lookup.
async fn load_quota(
    state_store: &dyn acteon_state::StateStore,
    id: &str,
) -> Result<Option<QuotaPolicy>, String> {
    let key = quota_state_key(id);
    let value = state_store.get(&key).await.map_err(|e| e.to_string())?;

    match value {
        Some(data) => {
            let policy = serde_json::from_str::<QuotaPolicy>(&data).map_err(|e| e.to_string())?;
            Ok(Some(policy))
        }
        None => Ok(None),
    }
}

/// Read the current quota usage counter for a policy.
async fn read_usage(
    state_store: &dyn acteon_state::StateStore,
    policy: &QuotaPolicy,
) -> Result<QuotaUsageResponse, String> {
    let now = Utc::now();
    let counter_id = quota_counter_key(&policy.namespace, &policy.tenant, &policy.window, &now);
    let counter_key = StateKey::new(
        policy.namespace.as_str(),
        policy.tenant.as_str(),
        KeyKind::QuotaUsage,
        &counter_id,
    );

    let current_str = state_store
        .get(&counter_key)
        .await
        .map_err(|e| e.to_string())?;
    let used: u64 = current_str.and_then(|s| s.parse().ok()).unwrap_or(0);
    let (_, resets_at) = compute_window_boundaries(&policy.window, &now);

    Ok(QuotaUsageResponse {
        used,
        limit: policy.max_actions,
        remaining: policy.max_actions.saturating_sub(used),
        resets_at,
    })
}

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
// Handlers
// ---------------------------------------------------------------------------

/// `POST /v1/quotas` -- create a quota policy.
#[utoipa::path(
    post,
    path = "/v1/quotas",
    tag = "Quotas",
    summary = "Create a quota policy",
    description = "Creates a new tenant quota policy. Validates the window and stores the policy in both the state store and the gateway.",
    request_body(content = CreateQuotaRequest, description = "Quota policy definition"),
    responses(
        (status = 201, description = "Quota policy created", body = QuotaResponse),
        (status = 400, description = "Validation error", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn create_quota(
    State(state): State<AppState>,
    Json(req): Json<CreateQuotaRequest>,
) -> impl IntoResponse {
    // Validate window.
    let window = match parse_window(&req.window) {
        Ok(w) => w,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &e),
    };

    let now = Utc::now();
    let id = uuid::Uuid::new_v4().to_string();

    let policy = QuotaPolicy {
        id: id.clone(),
        namespace: req.namespace.clone(),
        tenant: req.tenant.clone(),
        max_actions: req.max_actions,
        window,
        overage_behavior: req.overage_behavior,
        enabled: true,
        created_at: now,
        updated_at: now,
        description: req.description,
        labels: req.labels,
    };

    // Persist to state store.
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();
    let key = quota_state_key(&id);
    let data = match serde_json::to_string(&policy) {
        Ok(d) => d,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("serialization error: {e}"),
            );
        }
    };
    if let Err(e) = state_store.set(&key, &data, None).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    drop(gw);

    // Register in gateway (uses interior mutability via RwLock).
    let gw = state.gateway.read().await;
    gw.set_quota_policy(policy.clone());

    let resp = policy_to_response(&policy, None);
    (StatusCode::CREATED, Json(serde_json::json!(resp))).into_response()
}

/// `GET /v1/quotas` -- list quota policies.
#[utoipa::path(
    get,
    path = "/v1/quotas",
    tag = "Quotas",
    summary = "List quota policies",
    description = "Returns quota policies, optionally filtered by namespace and tenant.",
    params(ListQuotasParams),
    responses(
        (status = 200, description = "Quota policy list", body = ListQuotasResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn list_quotas(
    State(state): State<AppState>,
    Query(params): Query<ListQuotasParams>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    let results = match state_store.scan_keys_by_kind(KeyKind::Quota).await {
        Ok(r) => r,
        Err(e) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
        }
    };

    let mut quotas: Vec<QuotaResponse> = Vec::new();
    let mut skipped = 0usize;

    for (_key, value) in results {
        let Ok(policy) = serde_json::from_str::<QuotaPolicy>(&value) else {
            continue;
        };

        // Apply namespace filter.
        if let Some(ref ns) = params.namespace
            && policy.namespace != *ns
        {
            continue;
        }

        // Apply tenant filter.
        if let Some(ref t) = params.tenant
            && policy.tenant != *t
        {
            continue;
        }

        // Offset-based pagination.
        if skipped < params.offset {
            skipped += 1;
            continue;
        }

        if quotas.len() >= params.limit {
            break;
        }

        quotas.push(policy_to_response(&policy, None));
    }

    let count = quotas.len();
    (
        StatusCode::OK,
        Json(serde_json::json!(ListQuotasResponse { quotas, count })),
    )
        .into_response()
}

/// `GET /v1/quotas/{id}` -- get a single quota policy with usage.
#[utoipa::path(
    get,
    path = "/v1/quotas/{id}",
    tag = "Quotas",
    summary = "Get quota policy details",
    description = "Returns the full details of a quota policy including current usage.",
    params(("id" = String, Path, description = "Quota policy ID")),
    responses(
        (status = 200, description = "Quota policy details", body = QuotaResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn get_quota(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    match load_quota(state_store.as_ref(), &id).await {
        Ok(Some(policy)) => {
            let usage = read_usage(state_store.as_ref(), &policy).await.ok();
            (
                StatusCode::OK,
                Json(serde_json::json!(policy_to_response(&policy, usage))),
            )
                .into_response()
        }
        Ok(None) => error_response(
            StatusCode::NOT_FOUND,
            &format!("quota policy not found: {id}"),
        ),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

/// `PUT /v1/quotas/{id}` -- update a quota policy.
#[utoipa::path(
    put,
    path = "/v1/quotas/{id}",
    tag = "Quotas",
    summary = "Update a quota policy",
    description = "Updates fields of an existing quota policy.",
    params(("id" = String, Path, description = "Quota policy ID")),
    request_body(content = UpdateQuotaRequest, description = "Fields to update"),
    responses(
        (status = 200, description = "Updated quota policy", body = QuotaResponse),
        (status = 400, description = "Validation error", body = ErrorResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn update_quota(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateQuotaRequest>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    let mut policy = match load_quota(state_store.as_ref(), &id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return error_response(
                StatusCode::NOT_FOUND,
                &format!("quota policy not found: {id}"),
            );
        }
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };

    // Apply updates.
    if let Some(max) = req.max_actions {
        policy.max_actions = max;
    }
    if let Some(ref window_str) = req.window {
        match parse_window(window_str) {
            Ok(w) => policy.window = w,
            Err(e) => return error_response(StatusCode::BAD_REQUEST, &e),
        }
    }
    if let Some(behavior) = req.overage_behavior {
        policy.overage_behavior = behavior;
    }
    if let Some(enabled) = req.enabled {
        policy.enabled = enabled;
    }
    if let Some(desc) = req.description {
        policy.description = Some(desc);
    }
    if let Some(labels) = req.labels {
        policy.labels = labels;
    }

    policy.updated_at = Utc::now();

    // Persist.
    let key = quota_state_key(&id);
    let data = match serde_json::to_string(&policy) {
        Ok(d) => d,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("serialization error: {e}"),
            );
        }
    };
    if let Err(e) = state_store.set(&key, &data, None).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    drop(gw);

    // Update gateway (uses interior mutability via RwLock).
    let gw = state.gateway.read().await;
    gw.set_quota_policy(policy.clone());

    let resp = policy_to_response(&policy, None);
    (StatusCode::OK, Json(serde_json::json!(resp))).into_response()
}

/// `DELETE /v1/quotas/{id}` -- delete a quota policy.
#[utoipa::path(
    delete,
    path = "/v1/quotas/{id}",
    tag = "Quotas",
    summary = "Delete a quota policy",
    description = "Removes a quota policy from both the state store and the gateway.",
    params(("id" = String, Path, description = "Quota policy ID")),
    responses(
        (status = 204, description = "Quota policy deleted"),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn delete_quota(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    let policy = match load_quota(state_store.as_ref(), &id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return error_response(
                StatusCode::NOT_FOUND,
                &format!("quota policy not found: {id}"),
            );
        }
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };

    // Remove from state store.
    let key = quota_state_key(&id);
    if let Err(e) = state_store.delete(&key).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    drop(gw);

    // Remove from gateway (uses interior mutability via RwLock).
    let gw = state.gateway.read().await;
    gw.remove_quota_policy(&policy.namespace, &policy.tenant);

    StatusCode::NO_CONTENT.into_response()
}

/// `GET /v1/quotas/{id}/usage` -- get current usage for a quota.
#[utoipa::path(
    get,
    path = "/v1/quotas/{id}/usage",
    tag = "Quotas",
    summary = "Get quota usage",
    description = "Returns the current usage counters for a quota policy in the active window.",
    params(("id" = String, Path, description = "Quota policy ID")),
    responses(
        (status = 200, description = "Quota usage", body = QuotaUsage),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn get_quota_usage(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    let policy = match load_quota(state_store.as_ref(), &id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return error_response(
                StatusCode::NOT_FOUND,
                &format!("quota policy not found: {id}"),
            );
        }
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };

    let now = Utc::now();
    let counter_id = quota_counter_key(&policy.namespace, &policy.tenant, &policy.window, &now);
    let counter_key = StateKey::new(
        policy.namespace.as_str(),
        policy.tenant.as_str(),
        KeyKind::QuotaUsage,
        &counter_id,
    );

    let current_str = match state_store.get(&counter_key).await {
        Ok(v) => v,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let used: u64 = current_str.and_then(|s| s.parse().ok()).unwrap_or(0);
    let (_, resets_at) = compute_window_boundaries(&policy.window, &now);

    let usage = QuotaUsage {
        tenant: policy.tenant.clone(),
        namespace: policy.namespace.clone(),
        used,
        limit: policy.max_actions,
        remaining: policy.max_actions.saturating_sub(used),
        window: policy.window.clone(),
        resets_at,
        overage_behavior: policy.overage_behavior.clone(),
    };

    (StatusCode::OK, Json(serde_json::json!(usage))).into_response()
}
