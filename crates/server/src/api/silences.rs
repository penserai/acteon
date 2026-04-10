//! Silences API endpoints.
//!
//! CRUD operations for tenant-scoped silences. Silences suppress dispatched
//! actions whose labels match all of the silence's matchers during the
//! silence's active time window. See the feature documentation at
//! `docs/book/features/silences.md` and the master plan in
//! `docs/design-alertmanager-parity.md`.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use tracing::info;
use utoipa::{IntoParams, ToSchema};

use acteon_core::{MatchOp, Silence, SilenceMatcher};

use crate::auth::identity::CallerIdentity;
use crate::auth::role::Permission;

use super::AppState;
use super::schemas::ErrorResponse;

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

/// Shape of a matcher in create/update request bodies.
#[derive(Debug, Deserialize, ToSchema)]
pub struct MatcherInput {
    /// Label name to match against `action.metadata.labels`.
    #[schema(example = "severity")]
    pub name: String,
    /// Literal value (for `Equal` / `NotEqual`) or regex pattern (for `Regex` / `NotRegex`).
    #[schema(example = "warning")]
    pub value: String,
    /// Match operator: `equal`, `not_equal`, `regex`, or `not_regex`.
    #[serde(default = "default_op")]
    pub op: MatchOp,
}

fn default_op() -> MatchOp {
    MatchOp::Equal
}

/// Request body for `POST /v1/silences`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateSilenceRequest {
    /// Namespace this silence applies to.
    #[schema(example = "prod")]
    pub namespace: String,
    /// Tenant this silence applies to. Hierarchical matching applies:
    /// a silence on tenant `acme` also covers `acme.us-east`.
    #[schema(example = "acme")]
    pub tenant: String,
    /// Matchers — all must match for the silence to apply (AND semantics).
    pub matchers: Vec<MatcherInput>,
    /// Explicit start time (RFC 3339). Defaults to now if omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starts_at: Option<DateTime<Utc>>,
    /// Explicit end time (RFC 3339). Either `ends_at` or `duration_seconds`
    /// must be supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ends_at: Option<DateTime<Utc>>,
    /// Convenience: duration from `starts_at` in seconds. Ignored if
    /// `ends_at` is also supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u64>,
    /// Human-readable comment.
    #[schema(example = "deploying canary")]
    pub comment: String,
}

/// Request body for `PUT /v1/silences/{id}`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateSilenceRequest {
    /// New end time (RFC 3339). If omitted, the existing end time is kept.
    #[serde(default)]
    pub ends_at: Option<DateTime<Utc>>,
    /// New comment. If omitted, the existing comment is kept.
    #[serde(default)]
    pub comment: Option<String>,
}

/// Full silence response.
#[derive(Debug, Serialize, ToSchema)]
pub struct SilenceResponse {
    /// Unique silence ID.
    pub id: String,
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Matchers (AND semantics).
    pub matchers: Vec<SilenceMatcher>,
    /// When the silence becomes active.
    pub starts_at: DateTime<Utc>,
    /// When the silence expires.
    pub ends_at: DateTime<Utc>,
    /// Caller who created the silence.
    pub created_by: String,
    /// Human-readable comment.
    pub comment: String,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last-update time.
    pub updated_at: DateTime<Utc>,
    /// Whether the silence is currently active (within its time window).
    pub active: bool,
}

/// Response for listing silences.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListSilencesResponse {
    /// Silences matching the query.
    pub silences: Vec<SilenceResponse>,
    /// Total number of results returned.
    pub count: usize,
}

/// Query parameters for `GET /v1/silences`.
#[derive(Debug, Deserialize, IntoParams)]
pub struct ListSilencesParams {
    /// Filter by namespace.
    #[serde(default)]
    pub namespace: Option<String>,
    /// Filter by tenant.
    #[serde(default)]
    pub tenant: Option<String>,
    /// Include expired silences whose `ends_at` is in the past. Default false.
    #[serde(default)]
    pub include_expired: bool,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn error_response(status: StatusCode, message: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!(ErrorResponse {
            error: message.to_owned(),
        })),
    )
        .into_response()
}

fn silence_to_response(silence: &Silence, now: DateTime<Utc>) -> SilenceResponse {
    SilenceResponse {
        id: silence.id.clone(),
        namespace: silence.namespace.clone(),
        tenant: silence.tenant.clone(),
        matchers: silence.matchers.clone(),
        starts_at: silence.starts_at,
        ends_at: silence.ends_at,
        created_by: silence.created_by.clone(),
        comment: silence.comment.clone(),
        created_at: silence.created_at,
        updated_at: silence.updated_at,
        active: silence.is_active_at(now),
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /v1/silences` -- create a silence.
#[utoipa::path(
    post,
    path = "/v1/silences",
    tag = "Silences",
    summary = "Create a silence",
    description = "Creates a new tenant-scoped silence that suppresses matching dispatched actions during the active window. Regex matchers are capped at 256 characters and a 64KB compiled DFA to prevent ReDoS.",
    request_body(content = CreateSilenceRequest, description = "Silence definition"),
    responses(
        (status = 201, description = "Silence created", body = SilenceResponse),
        (status = 400, description = "Validation error", body = ErrorResponse),
        (status = 403, description = "Caller not authorized for this tenant", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn create_silence(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Json(req): Json<CreateSilenceRequest>,
) -> impl IntoResponse {
    // Permission + tenant-scope check.
    if !identity.role.has_permission(Permission::SilencesManage) {
        return error_response(
            StatusCode::FORBIDDEN,
            "insufficient permissions: silences manage requires admin or operator role",
        );
    }
    if !identity.can_manage_scope(&req.tenant, &req.namespace) {
        return error_response(
            StatusCode::FORBIDDEN,
            &format!(
                "forbidden: no grant covers tenant={} namespace={}",
                req.tenant, req.namespace
            ),
        );
    }

    // Resolve time window.
    let now = Utc::now();
    let starts_at = req.starts_at.unwrap_or(now);
    let ends_at = match (req.ends_at, req.duration_seconds) {
        (Some(end), _) => end,
        (None, Some(secs)) => {
            #[allow(clippy::cast_possible_wrap)]
            let secs = secs as i64;
            starts_at + Duration::seconds(secs)
        }
        (None, None) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "either ends_at or duration_seconds must be supplied",
            );
        }
    };

    // Build matchers with per-matcher validation.
    let mut matchers = Vec::with_capacity(req.matchers.len());
    for m in req.matchers {
        match SilenceMatcher::new(&m.name, &m.value, m.op) {
            Ok(matcher) => matchers.push(matcher),
            Err(e) => return error_response(StatusCode::BAD_REQUEST, &e),
        }
    }

    let silence = Silence {
        id: uuid::Uuid::now_v7().to_string(),
        namespace: req.namespace,
        tenant: req.tenant,
        matchers,
        starts_at,
        ends_at,
        created_by: identity.id.clone(),
        comment: req.comment,
        created_at: now,
        updated_at: now,
    };

    if let Err(e) = silence.validate() {
        return error_response(StatusCode::BAD_REQUEST, &e);
    }

    let gw = state.gateway.read().await;
    if let Err(e) = gw.persist_silence(&silence).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    if let Err(e) = gw.upsert_silence_cache(silence.clone()) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e);
    }

    info!(silence_id = %silence.id, namespace = %silence.namespace, tenant = %silence.tenant, "silence created");
    (
        StatusCode::CREATED,
        Json(serde_json::json!(silence_to_response(&silence, now))),
    )
        .into_response()
}

/// `GET /v1/silences` -- list silences.
#[utoipa::path(
    get,
    path = "/v1/silences",
    tag = "Silences",
    summary = "List silences",
    description = "Returns silences from the gateway cache, optionally filtered by namespace and tenant. Expired silences are hidden unless `include_expired=true`. The tenant filter is auto-injected for single-tenant callers.",
    params(ListSilencesParams),
    responses(
        (status = 200, description = "List of silences", body = ListSilencesResponse),
    )
)]
pub async fn list_silences(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Query(params): Query<ListSilencesParams>,
) -> impl IntoResponse {
    // Enforce requested tenant against grants.
    if let Some(ref requested_tenant) = params.tenant
        && let Some(allowed) = identity.allowed_tenants()
        && !allowed.contains(&requested_tenant.as_str())
    {
        return error_response(
            StatusCode::FORBIDDEN,
            &format!("no grant covers tenant={requested_tenant}"),
        );
    }

    let mut tenant = params.tenant.clone();
    if tenant.is_none()
        && let Some(allowed) = identity.allowed_tenants()
        && allowed.len() == 1
    {
        tenant = Some(allowed[0].to_owned());
    }

    let gw = state.gateway.read().await;
    let now = Utc::now();
    let all = gw.list_silences(params.namespace.as_deref(), tenant.as_deref());

    let filtered: Vec<SilenceResponse> = all
        .into_iter()
        .filter(|s| params.include_expired || s.is_active_at(now))
        .map(|s| silence_to_response(&s, now))
        .collect();

    let count = filtered.len();
    (
        StatusCode::OK,
        Json(serde_json::json!(ListSilencesResponse {
            silences: filtered,
            count,
        })),
    )
        .into_response()
}

/// `GET /v1/silences/{id}` -- fetch a single silence by ID.
#[utoipa::path(
    get,
    path = "/v1/silences/{id}",
    tag = "Silences",
    summary = "Get a silence by ID",
    description = "Returns the silence record, whether active or expired. Caller must have a grant covering the silence's namespace/tenant.",
    params(("id" = String, Path, description = "Silence ID")),
    responses(
        (status = 200, description = "Silence", body = SilenceResponse),
        (status = 403, description = "Caller not authorized for this silence", body = ErrorResponse),
        (status = 404, description = "Silence not found", body = ErrorResponse),
    )
)]
pub async fn get_silence(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let silence = match gw.get_silence(&id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return error_response(StatusCode::NOT_FOUND, &format!("silence not found: {id}"));
        }
        Err(e) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
        }
    };

    if !identity.can_manage_scope(&silence.tenant, &silence.namespace) {
        return error_response(
            StatusCode::FORBIDDEN,
            "forbidden: no grant covers this silence",
        );
    }

    (
        StatusCode::OK,
        Json(serde_json::json!(silence_to_response(&silence, Utc::now()))),
    )
        .into_response()
}

/// `PUT /v1/silences/{id}` -- extend or edit a silence.
#[utoipa::path(
    put,
    path = "/v1/silences/{id}",
    tag = "Silences",
    summary = "Update a silence",
    description = "Extends `ends_at` or edits the comment. Matchers are immutable — to change matchers, delete the silence and create a new one.",
    params(("id" = String, Path, description = "Silence ID")),
    request_body(content = UpdateSilenceRequest, description = "Partial update"),
    responses(
        (status = 200, description = "Updated silence", body = SilenceResponse),
        (status = 400, description = "Validation error", body = ErrorResponse),
        (status = 403, description = "Caller not authorized", body = ErrorResponse),
        (status = 404, description = "Silence not found", body = ErrorResponse),
    )
)]
pub async fn update_silence(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(id): Path<String>,
    Json(req): Json<UpdateSilenceRequest>,
) -> impl IntoResponse {
    if !identity.role.has_permission(Permission::SilencesManage) {
        return error_response(
            StatusCode::FORBIDDEN,
            "insufficient permissions: silences manage requires admin or operator role",
        );
    }

    let gw = state.gateway.read().await;
    let mut silence = match gw.get_silence(&id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return error_response(StatusCode::NOT_FOUND, &format!("silence not found: {id}"));
        }
        Err(e) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
        }
    };

    if !identity.can_manage_scope(&silence.tenant, &silence.namespace) {
        return error_response(
            StatusCode::FORBIDDEN,
            "forbidden: no grant covers this silence",
        );
    }

    if let Some(end) = req.ends_at {
        silence.ends_at = end;
    }
    if let Some(comment) = req.comment {
        silence.comment = comment;
    }
    silence.updated_at = Utc::now();

    if let Err(e) = silence.validate() {
        return error_response(StatusCode::BAD_REQUEST, &e);
    }

    if let Err(e) = gw.persist_silence(&silence).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    if let Err(e) = gw.upsert_silence_cache(silence.clone()) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e);
    }

    info!(silence_id = %id, "silence updated");
    (
        StatusCode::OK,
        Json(serde_json::json!(silence_to_response(&silence, Utc::now()))),
    )
        .into_response()
}

/// `DELETE /v1/silences/{id}` -- expire a silence immediately (soft).
#[utoipa::path(
    delete,
    path = "/v1/silences/{id}",
    tag = "Silences",
    summary = "Expire a silence",
    description = "Soft-expires the silence by setting `ends_at = now` and persisting. The record itself is NOT deleted, so audit references to the silence ID remain resolvable via `GET /v1/silences/{id}` and `GET /v1/silences?include_expired=true`. Matching dispatches immediately stop being silenced because the cache entry is removed and `is_active_at` now returns false. A background reaper (Phase 1.5) will eventually purge tombstoned records after their retention window.",
    params(("id" = String, Path, description = "Silence ID")),
    responses(
        (status = 204, description = "Silence expired"),
        (status = 403, description = "Caller not authorized", body = ErrorResponse),
        (status = 404, description = "Silence not found", body = ErrorResponse),
    )
)]
pub async fn delete_silence(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if !identity.role.has_permission(Permission::SilencesManage) {
        return error_response(
            StatusCode::FORBIDDEN,
            "insufficient permissions: silences manage requires admin or operator role",
        );
    }

    let gw = state.gateway.read().await;
    let mut silence = match gw.get_silence(&id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return error_response(StatusCode::NOT_FOUND, &format!("silence not found: {id}"));
        }
        Err(e) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
        }
    };

    if !identity.can_manage_scope(&silence.tenant, &silence.namespace) {
        return error_response(
            StatusCode::FORBIDDEN,
            "forbidden: no grant covers this silence",
        );
    }

    // Soft-expire: set ends_at = now, persist the updated record, and
    // refresh the cache entry in place. The dispatch path ignores the
    // entry naturally via `is_active_at`, but `list_silences?include_expired=true`
    // and `get_silence/{id}` can still see it so audit references remain
    // resolvable.
    let now = Utc::now();
    silence.ends_at = now;
    silence.updated_at = now;
    if let Err(e) = gw.persist_silence(&silence).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    if let Err(e) = gw.upsert_silence_cache(silence.clone()) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e);
    }

    info!(silence_id = %id, "silence expired (soft)");
    StatusCode::NO_CONTENT.into_response()
}
