//! Data retention policy API endpoints.
//!
//! CRUD operations for per-tenant data retention policies.

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use acteon_core::RetentionPolicy;
use acteon_state::{KeyKind, StateKey};

use super::AppState;
use super::schemas::ErrorResponse;

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

/// Request body for creating a retention policy.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateRetentionRequest {
    /// Namespace this policy applies to.
    #[schema(example = "notifications")]
    pub namespace: String,
    /// Tenant this policy applies to.
    #[schema(example = "tenant-1")]
    pub tenant: String,
    /// Override for the global audit TTL (seconds).
    #[serde(default)]
    pub audit_ttl_seconds: Option<u64>,
    /// TTL for completed chain state records (seconds).
    #[serde(default)]
    pub state_ttl_seconds: Option<u64>,
    /// TTL for resolved event state records (seconds).
    #[serde(default)]
    pub event_ttl_seconds: Option<u64>,
    /// When `true`, audit records never expire (compliance hold).
    #[serde(default)]
    pub compliance_hold: bool,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// Arbitrary key-value labels.
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

/// Request body for updating a retention policy.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateRetentionRequest {
    /// Updated enabled state.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Updated audit TTL (seconds).
    #[serde(default)]
    pub audit_ttl_seconds: Option<u64>,
    /// Updated state TTL (seconds).
    #[serde(default)]
    pub state_ttl_seconds: Option<u64>,
    /// Updated event TTL (seconds).
    #[serde(default)]
    pub event_ttl_seconds: Option<u64>,
    /// Updated compliance hold flag.
    #[serde(default)]
    pub compliance_hold: Option<bool>,
    /// Updated description.
    #[serde(default)]
    pub description: Option<String>,
    /// Updated labels.
    #[serde(default)]
    pub labels: Option<HashMap<String, String>>,
}

/// Full retention policy response.
#[derive(Debug, Serialize, ToSchema)]
pub struct RetentionResponse {
    /// Unique policy ID.
    pub id: String,
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Whether this policy is active.
    pub enabled: bool,
    /// Audit TTL override (seconds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audit_ttl_seconds: Option<u64>,
    /// State TTL (seconds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_ttl_seconds: Option<u64>,
    /// Event TTL (seconds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_ttl_seconds: Option<u64>,
    /// Compliance hold flag.
    pub compliance_hold: bool,
    /// When the policy was created.
    pub created_at: DateTime<Utc>,
    /// When the policy was last updated.
    pub updated_at: DateTime<Utc>,
    /// Optional description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Arbitrary labels.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
}

/// Response for listing retention policies.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListRetentionResponse {
    /// List of retention policies.
    pub policies: Vec<RetentionResponse>,
    /// Total count of results returned.
    pub count: usize,
}

/// Query parameters for listing retention policies.
#[derive(Debug, Deserialize, IntoParams)]
pub struct ListRetentionParams {
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

/// Build a [`RetentionResponse`] from a [`RetentionPolicy`].
fn policy_to_response(policy: &RetentionPolicy) -> RetentionResponse {
    RetentionResponse {
        id: policy.id.clone(),
        namespace: policy.namespace.clone(),
        tenant: policy.tenant.clone(),
        enabled: policy.enabled,
        audit_ttl_seconds: policy.audit_ttl_seconds,
        state_ttl_seconds: policy.state_ttl_seconds,
        event_ttl_seconds: policy.event_ttl_seconds,
        compliance_hold: policy.compliance_hold,
        created_at: policy.created_at,
        updated_at: policy.updated_at,
        description: policy.description.clone(),
        labels: policy.labels.clone(),
    }
}

/// Well-known namespace used for retention policy storage keys.
const RETENTION_STORE_NS: &str = "_system";
/// Well-known tenant used for retention policy storage keys.
const RETENTION_STORE_TENANT: &str = "_retention";

/// Build a [`StateKey`] for a retention policy by its ID.
fn retention_state_key(id: &str) -> StateKey {
    StateKey::new(
        RETENTION_STORE_NS,
        RETENTION_STORE_TENANT,
        KeyKind::Retention,
        id,
    )
}

/// Build a [`StateKey`] for the `namespace:tenant` → policy-ID index.
fn retention_index_key(namespace: &str, tenant: &str) -> StateKey {
    let suffix = format!("idx:{namespace}:{tenant}");
    StateKey::new(
        RETENTION_STORE_NS,
        RETENTION_STORE_TENANT,
        KeyKind::Retention,
        &suffix,
    )
}

/// Load a [`RetentionPolicy`] from the state store by ID.
async fn load_retention(
    state_store: &dyn acteon_state::StateStore,
    id: &str,
) -> Result<Option<RetentionPolicy>, String> {
    let key = retention_state_key(id);
    let value = state_store.get(&key).await.map_err(|e| e.to_string())?;

    match value {
        Some(data) => {
            let policy =
                serde_json::from_str::<RetentionPolicy>(&data).map_err(|e| e.to_string())?;
            Ok(Some(policy))
        }
        None => Ok(None),
    }
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

/// `POST /v1/retention` -- create a retention policy.
#[utoipa::path(
    post,
    path = "/v1/retention",
    tag = "Retention",
    summary = "Create a retention policy",
    description = "Creates a new per-tenant data retention policy.",
    request_body(content = CreateRetentionRequest, description = "Retention policy definition"),
    responses(
        (status = 201, description = "Retention policy created", body = RetentionResponse),
        (status = 409, description = "A retention policy already exists for this namespace:tenant", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn create_retention(
    State(state): State<AppState>,
    Json(req): Json<CreateRetentionRequest>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    // Check for duplicate: only one retention policy per namespace:tenant.
    let idx_key = retention_index_key(&req.namespace, &req.tenant);
    match state_store.get(&idx_key).await {
        Ok(Some(_)) => {
            return error_response(
                StatusCode::CONFLICT,
                &format!(
                    "a retention policy already exists for namespace={} tenant={}",
                    req.namespace, req.tenant
                ),
            );
        }
        Err(e) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
        }
        Ok(None) => {} // No existing policy — proceed.
    }

    let now = Utc::now();
    let id = uuid::Uuid::new_v4().to_string();

    let policy = RetentionPolicy {
        id: id.clone(),
        namespace: req.namespace.clone(),
        tenant: req.tenant.clone(),
        enabled: true,
        audit_ttl_seconds: req.audit_ttl_seconds,
        state_ttl_seconds: req.state_ttl_seconds,
        event_ttl_seconds: req.event_ttl_seconds,
        compliance_hold: req.compliance_hold,
        created_at: now,
        updated_at: now,
        description: req.description,
        labels: req.labels,
    };

    // Persist policy to state store.
    let key = retention_state_key(&id);
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

    // Write the namespace:tenant → policy-ID index key.
    if let Err(e) = state_store.set(&idx_key, &id, None).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    drop(gw);

    // Register in gateway (uses interior mutability via RwLock).
    let gw = state.gateway.read().await;
    gw.set_retention_policy(policy.clone());

    let resp = policy_to_response(&policy);
    (StatusCode::CREATED, Json(serde_json::json!(resp))).into_response()
}

/// `GET /v1/retention` -- list retention policies.
#[utoipa::path(
    get,
    path = "/v1/retention",
    tag = "Retention",
    summary = "List retention policies",
    description = "Returns retention policies, optionally filtered by namespace and tenant.",
    params(ListRetentionParams),
    responses(
        (status = 200, description = "Retention policy list", body = ListRetentionResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn list_retention(
    State(state): State<AppState>,
    Query(params): Query<ListRetentionParams>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    let results = match state_store.scan_keys_by_kind(KeyKind::Retention).await {
        Ok(r) => r,
        Err(e) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
        }
    };

    let mut policies: Vec<RetentionResponse> = Vec::new();
    let mut skipped = 0usize;

    for (_key, value) in results {
        let Ok(policy) = serde_json::from_str::<RetentionPolicy>(&value) else {
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

        if policies.len() >= params.limit {
            break;
        }

        policies.push(policy_to_response(&policy));
    }

    let count = policies.len();
    (
        StatusCode::OK,
        Json(serde_json::json!(ListRetentionResponse { policies, count })),
    )
        .into_response()
}

/// `GET /v1/retention/{id}` -- get a single retention policy.
#[utoipa::path(
    get,
    path = "/v1/retention/{id}",
    tag = "Retention",
    summary = "Get retention policy details",
    description = "Returns the full details of a retention policy.",
    params(("id" = String, Path, description = "Retention policy ID")),
    responses(
        (status = 200, description = "Retention policy details", body = RetentionResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn get_retention(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    match load_retention(state_store.as_ref(), &id).await {
        Ok(Some(policy)) => (
            StatusCode::OK,
            Json(serde_json::json!(policy_to_response(&policy))),
        )
            .into_response(),
        Ok(None) => error_response(
            StatusCode::NOT_FOUND,
            &format!("retention policy not found: {id}"),
        ),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

/// `PUT /v1/retention/{id}` -- update a retention policy.
#[utoipa::path(
    put,
    path = "/v1/retention/{id}",
    tag = "Retention",
    summary = "Update a retention policy",
    description = "Updates fields of an existing retention policy.",
    params(("id" = String, Path, description = "Retention policy ID")),
    request_body(content = UpdateRetentionRequest, description = "Fields to update"),
    responses(
        (status = 200, description = "Updated retention policy", body = RetentionResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn update_retention(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateRetentionRequest>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    let mut policy = match load_retention(state_store.as_ref(), &id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return error_response(
                StatusCode::NOT_FOUND,
                &format!("retention policy not found: {id}"),
            );
        }
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };

    // Apply updates.
    if let Some(enabled) = req.enabled {
        policy.enabled = enabled;
    }
    if req.audit_ttl_seconds.is_some() {
        policy.audit_ttl_seconds = req.audit_ttl_seconds;
    }
    if req.state_ttl_seconds.is_some() {
        policy.state_ttl_seconds = req.state_ttl_seconds;
    }
    if req.event_ttl_seconds.is_some() {
        policy.event_ttl_seconds = req.event_ttl_seconds;
    }
    if let Some(hold) = req.compliance_hold {
        policy.compliance_hold = hold;
    }
    if let Some(desc) = req.description {
        policy.description = Some(desc);
    }
    if let Some(labels) = req.labels {
        policy.labels = labels;
    }

    policy.updated_at = Utc::now();

    // Persist.
    let key = retention_state_key(&id);
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

    // Update gateway.
    let gw = state.gateway.read().await;
    gw.set_retention_policy(policy.clone());

    let resp = policy_to_response(&policy);
    (StatusCode::OK, Json(serde_json::json!(resp))).into_response()
}

/// `DELETE /v1/retention/{id}` -- delete a retention policy.
#[utoipa::path(
    delete,
    path = "/v1/retention/{id}",
    tag = "Retention",
    summary = "Delete a retention policy",
    description = "Removes a retention policy from both the state store and the gateway.",
    params(("id" = String, Path, description = "Retention policy ID")),
    responses(
        (status = 204, description = "Retention policy deleted"),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn delete_retention(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    let policy = match load_retention(state_store.as_ref(), &id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return error_response(
                StatusCode::NOT_FOUND,
                &format!("retention policy not found: {id}"),
            );
        }
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };

    // Remove from state store.
    let key = retention_state_key(&id);
    if let Err(e) = state_store.delete(&key).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }

    // Remove the namespace:tenant → policy-ID index key.
    let idx_key = retention_index_key(&policy.namespace, &policy.tenant);
    let _ = state_store.delete(&idx_key).await;
    drop(gw);

    // Remove from gateway.
    let gw = state.gateway.read().await;
    gw.remove_retention_policy(&policy.namespace, &policy.tenant);

    StatusCode::NO_CONTENT.into_response()
}
