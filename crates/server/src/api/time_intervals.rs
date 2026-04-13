//! Time intervals API endpoints.
//!
//! CRUD operations for tenant-scoped time intervals — recurring schedules
//! that rules reference via `mute_time_intervals` / `active_time_intervals`
//! to gate dispatch by wall-clock time. Mirrors Alertmanager's
//! `time_intervals` model so configurations imported from Alertmanager
//! map 1:1.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::info;
use utoipa::{IntoParams, ToSchema};

use acteon_core::TimeInterval;
use acteon_core::time_interval::{
    DayOfMonthRange, MonthRange, TimeOfDayRange, TimeRange, WeekdayRange, YearRange,
};

use crate::auth::identity::CallerIdentity;
use crate::auth::role::Permission;

use super::AppState;
use super::schemas::ErrorResponse;

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct TimeOfDayInput {
    /// Start time as `HH:MM` (24-hour clock).
    #[schema(example = "09:00")]
    pub start: String,
    /// End time as `HH:MM`. `24:00` is allowed as an end-of-day sentinel.
    #[schema(example = "17:00")]
    pub end: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct TimeRangeInput {
    /// Time-of-day windows in the interval's location.
    #[serde(default)]
    pub times: Vec<TimeOfDayInput>,
    /// Weekday ranges (1=Mon..7=Sun).
    #[serde(default)]
    pub weekdays: Vec<WeekdayRange>,
    /// Day-of-month ranges. Negative values count from end of month.
    #[serde(default)]
    pub days_of_month: Vec<DayOfMonthRange>,
    /// Month ranges (1..=12).
    #[serde(default)]
    pub months: Vec<MonthRange>,
    /// Year ranges.
    #[serde(default)]
    pub years: Vec<YearRange>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateTimeIntervalRequest {
    /// Stable interval name within the (namespace, tenant) scope.
    #[schema(example = "business-hours")]
    pub name: String,
    /// Namespace this interval belongs to.
    #[schema(example = "prod")]
    pub namespace: String,
    /// Tenant this interval belongs to.
    #[schema(example = "acme")]
    pub tenant: String,
    /// Time ranges; the interval matches if any range matches.
    #[serde(default)]
    pub time_ranges: Vec<TimeRangeInput>,
    /// IANA timezone (e.g. `America/New_York`). Defaults to UTC.
    #[serde(default)]
    pub location: Option<String>,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateTimeIntervalRequest {
    /// Replacement time ranges. If `None`, ranges are unchanged.
    #[serde(default)]
    pub time_ranges: Option<Vec<TimeRangeInput>>,
    /// Replacement timezone. If `None`, timezone is unchanged.
    #[serde(default)]
    pub location: Option<String>,
    /// Replacement description. If `None`, description is unchanged.
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TimeIntervalResponse {
    pub name: String,
    pub namespace: String,
    pub tenant: String,
    pub time_ranges: Vec<TimeRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub created_by: String,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: chrono::DateTime<Utc>,
    /// Whether the interval matches at the moment of the response.
    pub matches_now: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ListTimeIntervalsResponse {
    pub time_intervals: Vec<TimeIntervalResponse>,
    pub count: usize,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ListTimeIntervalsParams {
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub tenant: Option<String>,
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

fn parse_hm(value: &str) -> Result<(u32, u32), String> {
    let (h, m) = value
        .split_once(':')
        .ok_or_else(|| format!("time {value:?} must be HH:MM"))?;
    let hour: u32 = h
        .parse()
        .map_err(|_| format!("invalid hour in {value:?}"))?;
    let minute: u32 = m
        .parse()
        .map_err(|_| format!("invalid minute in {value:?}"))?;
    Ok((hour, minute))
}

fn convert_time_range(input: TimeRangeInput) -> Result<TimeRange, String> {
    let mut times = Vec::with_capacity(input.times.len());
    for t in input.times {
        let (sh, sm) = parse_hm(&t.start)?;
        let (eh, em) = parse_hm(&t.end)?;
        times.push(TimeOfDayRange::from_hm(sh, sm, eh, em)?);
    }
    Ok(TimeRange {
        times,
        weekdays: input.weekdays,
        days_of_month: input.days_of_month,
        months: input.months,
        years: input.years,
    })
}

fn time_interval_to_response(interval: &TimeInterval) -> TimeIntervalResponse {
    TimeIntervalResponse {
        name: interval.name.clone(),
        namespace: interval.namespace.clone(),
        tenant: interval.tenant.clone(),
        time_ranges: interval.time_ranges.clone(),
        location: interval.location.clone(),
        description: interval.description.clone(),
        created_by: interval.created_by.clone(),
        created_at: interval.created_at,
        updated_at: interval.updated_at,
        matches_now: interval.matches_at(Utc::now()),
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /v1/time-intervals` -- create a time interval.
#[utoipa::path(
    post,
    path = "/v1/time-intervals",
    tag = "TimeIntervals",
    summary = "Create a time interval",
    description = "Creates a new tenant-scoped time interval. Rules can reference it from `mute_time_intervals` or `active_time_intervals` to gate dispatch by wall-clock time.",
    request_body(content = CreateTimeIntervalRequest, description = "Time interval definition"),
    responses(
        (status = 201, description = "Time interval created", body = TimeIntervalResponse),
        (status = 400, description = "Validation error", body = ErrorResponse),
        (status = 403, description = "Caller not authorized", body = ErrorResponse),
        (status = 409, description = "Time interval already exists", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn create_time_interval(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Json(req): Json<CreateTimeIntervalRequest>,
) -> impl IntoResponse {
    if !identity
        .role
        .has_permission(Permission::TimeIntervalsManage)
    {
        return error_response(
            StatusCode::FORBIDDEN,
            "insufficient permissions: time intervals manage requires admin or operator role",
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

    let mut time_ranges = Vec::with_capacity(req.time_ranges.len());
    for tr in req.time_ranges {
        match convert_time_range(tr) {
            Ok(r) => time_ranges.push(r),
            Err(e) => return error_response(StatusCode::BAD_REQUEST, &e),
        }
    }

    let now = Utc::now();
    let interval = TimeInterval {
        name: req.name,
        namespace: req.namespace,
        tenant: req.tenant,
        time_ranges,
        location: req.location,
        description: req.description,
        created_by: identity.id.clone(),
        created_at: now,
        updated_at: now,
    };

    if let Err(e) = interval.validate() {
        return error_response(StatusCode::BAD_REQUEST, &e);
    }

    let gw = state.gateway.read().await;

    // Reject duplicates explicitly so create vs update stays clean.
    match gw
        .get_time_interval(&interval.namespace, &interval.tenant, &interval.name)
        .await
    {
        Ok(Some(_)) => {
            return error_response(
                StatusCode::CONFLICT,
                &format!("time interval already exists: {}", interval.name),
            );
        }
        Ok(None) => {}
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }

    if let Err(e) = gw.persist_time_interval(&interval).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    if let Err(e) = gw.upsert_time_interval_cache(interval.clone()) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e);
    }

    info!(
        name = %interval.name,
        namespace = %interval.namespace,
        tenant = %interval.tenant,
        "time interval created"
    );
    (
        StatusCode::CREATED,
        Json(serde_json::json!(time_interval_to_response(&interval))),
    )
        .into_response()
}

/// `GET /v1/time-intervals` -- list time intervals.
#[utoipa::path(
    get,
    path = "/v1/time-intervals",
    tag = "TimeIntervals",
    summary = "List time intervals",
    description = "Lists time intervals from the gateway cache, optionally filtered by namespace and tenant.",
    params(ListTimeIntervalsParams),
    responses(
        (status = 200, description = "Time interval list", body = ListTimeIntervalsResponse),
    )
)]
pub async fn list_time_intervals(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Query(params): Query<ListTimeIntervalsParams>,
) -> impl IntoResponse {
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

    let allowed_tenants: Option<Vec<String>> = identity
        .allowed_tenants()
        .map(|t| t.iter().map(|s| (*s).to_owned()).collect());

    let gw = state.gateway.read().await;
    let all = gw.list_time_intervals(params.namespace.as_deref(), tenant.as_deref());

    let filtered: Vec<TimeIntervalResponse> = all
        .into_iter()
        .filter(|i| match &allowed_tenants {
            Some(allowed) => allowed.iter().any(|t| t == &i.tenant),
            None => true,
        })
        .map(|i| time_interval_to_response(&i))
        .collect();

    let count = filtered.len();
    (
        StatusCode::OK,
        Json(serde_json::json!(ListTimeIntervalsResponse {
            time_intervals: filtered,
            count,
        })),
    )
        .into_response()
}

/// `GET /v1/time-intervals/{namespace}/{tenant}/{name}` -- fetch one.
#[utoipa::path(
    get,
    path = "/v1/time-intervals/{namespace}/{tenant}/{name}",
    tag = "TimeIntervals",
    summary = "Get a time interval",
    description = "Returns the named time interval scoped to the given namespace and tenant.",
    params(
        ("namespace" = String, Path, description = "Namespace"),
        ("tenant" = String, Path, description = "Tenant"),
        ("name" = String, Path, description = "Interval name"),
    ),
    responses(
        (status = 200, description = "Time interval", body = TimeIntervalResponse),
        (status = 403, description = "Caller not authorized", body = ErrorResponse),
        (status = 404, description = "Time interval not found", body = ErrorResponse),
    )
)]
pub async fn get_time_interval(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, name)): Path<(String, String, String)>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let interval = match gw.get_time_interval(&namespace, &tenant, &name).await {
        Ok(Some(i)) => i,
        Ok(None) => {
            return error_response(
                StatusCode::NOT_FOUND,
                &format!("time interval not found: {namespace}/{tenant}/{name}"),
            );
        }
        Err(e) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
        }
    };

    if !identity.can_manage_scope(&interval.tenant, &interval.namespace) {
        return error_response(
            StatusCode::FORBIDDEN,
            "forbidden: no grant covers this time interval",
        );
    }

    (
        StatusCode::OK,
        Json(serde_json::json!(time_interval_to_response(&interval))),
    )
        .into_response()
}

/// `PUT /v1/time-intervals/{namespace}/{tenant}/{name}` -- update.
#[utoipa::path(
    put,
    path = "/v1/time-intervals/{namespace}/{tenant}/{name}",
    tag = "TimeIntervals",
    summary = "Update a time interval",
    description = "Replaces time ranges, location, or description. The name and (namespace, tenant) tuple are immutable.",
    params(
        ("namespace" = String, Path, description = "Namespace"),
        ("tenant" = String, Path, description = "Tenant"),
        ("name" = String, Path, description = "Interval name"),
    ),
    request_body(content = UpdateTimeIntervalRequest, description = "Partial update"),
    responses(
        (status = 200, description = "Updated interval", body = TimeIntervalResponse),
        (status = 400, description = "Validation error", body = ErrorResponse),
        (status = 403, description = "Caller not authorized", body = ErrorResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
    )
)]
pub async fn update_time_interval(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, name)): Path<(String, String, String)>,
    Json(req): Json<UpdateTimeIntervalRequest>,
) -> impl IntoResponse {
    if !identity
        .role
        .has_permission(Permission::TimeIntervalsManage)
    {
        return error_response(
            StatusCode::FORBIDDEN,
            "insufficient permissions: time intervals manage requires admin or operator role",
        );
    }

    let gw = state.gateway.read().await;
    let mut interval = match gw.get_time_interval(&namespace, &tenant, &name).await {
        Ok(Some(i)) => i,
        Ok(None) => {
            return error_response(
                StatusCode::NOT_FOUND,
                &format!("time interval not found: {namespace}/{tenant}/{name}"),
            );
        }
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    if !identity.can_manage_scope(&interval.tenant, &interval.namespace) {
        return error_response(
            StatusCode::FORBIDDEN,
            "forbidden: no grant covers this time interval",
        );
    }

    if let Some(ranges) = req.time_ranges {
        let mut converted = Vec::with_capacity(ranges.len());
        for tr in ranges {
            match convert_time_range(tr) {
                Ok(r) => converted.push(r),
                Err(e) => return error_response(StatusCode::BAD_REQUEST, &e),
            }
        }
        interval.time_ranges = converted;
    }
    if let Some(loc) = req.location {
        interval.location = if loc.is_empty() { None } else { Some(loc) };
    }
    if let Some(desc) = req.description {
        interval.description = if desc.is_empty() { None } else { Some(desc) };
    }
    interval.updated_at = Utc::now();

    if let Err(e) = interval.validate() {
        return error_response(StatusCode::BAD_REQUEST, &e);
    }

    if let Err(e) = gw.persist_time_interval(&interval).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    if let Err(e) = gw.upsert_time_interval_cache(interval.clone()) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e);
    }

    info!(name = %name, "time interval updated");
    (
        StatusCode::OK,
        Json(serde_json::json!(time_interval_to_response(&interval))),
    )
        .into_response()
}

/// `DELETE /v1/time-intervals/{namespace}/{tenant}/{name}` -- delete.
#[utoipa::path(
    delete,
    path = "/v1/time-intervals/{namespace}/{tenant}/{name}",
    tag = "TimeIntervals",
    summary = "Delete a time interval",
    description = "Removes the time interval from both the cache and the state store. Rules that still reference the interval will treat it as 'not found' and proceed.",
    params(
        ("namespace" = String, Path, description = "Namespace"),
        ("tenant" = String, Path, description = "Tenant"),
        ("name" = String, Path, description = "Interval name"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Caller not authorized", body = ErrorResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
    )
)]
pub async fn delete_time_interval(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, name)): Path<(String, String, String)>,
) -> impl IntoResponse {
    if !identity
        .role
        .has_permission(Permission::TimeIntervalsManage)
    {
        return error_response(
            StatusCode::FORBIDDEN,
            "insufficient permissions: time intervals manage requires admin or operator role",
        );
    }

    let gw = state.gateway.read().await;
    let interval = match gw.get_time_interval(&namespace, &tenant, &name).await {
        Ok(Some(i)) => i,
        Ok(None) => {
            return error_response(
                StatusCode::NOT_FOUND,
                &format!("time interval not found: {namespace}/{tenant}/{name}"),
            );
        }
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    if !identity.can_manage_scope(&interval.tenant, &interval.namespace) {
        return error_response(
            StatusCode::FORBIDDEN,
            "forbidden: no grant covers this time interval",
        );
    }

    if let Err(e) = gw.delete_time_interval(&namespace, &tenant, &name).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    gw.remove_time_interval_cache(&namespace, &tenant, &name);

    info!(name = %name, "time interval deleted");
    StatusCode::NO_CONTENT.into_response()
}
