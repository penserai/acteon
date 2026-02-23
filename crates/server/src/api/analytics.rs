use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use utoipa::IntoParams;

use acteon_core::analytics::{AnalyticsInterval, AnalyticsMetric, AnalyticsQuery};

use crate::auth::identity::CallerIdentity;

use super::AppState;
use super::schemas::ErrorResponse;

/// Query parameters for the analytics endpoint.
#[derive(Debug, Deserialize, IntoParams)]
pub struct AnalyticsParams {
    /// Metric to compute: `volume`, `outcome_breakdown`, `top_action_types`, `latency`, `error_rate`.
    pub metric: AnalyticsMetric,
    /// Filter by namespace.
    pub namespace: Option<String>,
    /// Filter by tenant.
    pub tenant: Option<String>,
    /// Filter by provider.
    pub provider: Option<String>,
    /// Filter by action type.
    pub action_type: Option<String>,
    /// Filter by outcome.
    pub outcome: Option<String>,
    /// Time bucket interval (default: daily).
    pub interval: Option<AnalyticsInterval>,
    /// Start of time range (RFC 3339).
    pub from: Option<DateTime<Utc>>,
    /// End of time range (RFC 3339).
    pub to: Option<DateTime<Utc>>,
    /// Dimension to group by (e.g. `provider`, `action_type`, `outcome`).
    pub group_by: Option<String>,
    /// Number of top entries (default 10).
    pub top_n: Option<usize>,
}

/// `GET /v1/analytics` -- query aggregated action analytics.
#[utoipa::path(
    get,
    path = "/v1/analytics",
    tag = "Analytics",
    summary = "Query action analytics",
    description = "Returns time-bucketed aggregated metrics over the audit trail. Supports volume, outcome breakdown, top action types, latency percentiles, and error rate metrics.",
    params(
        ("metric" = AnalyticsMetric, Query, description = "Metric to compute"),
        ("namespace" = Option<String>, Query, description = "Filter by namespace"),
        ("tenant" = Option<String>, Query, description = "Filter by tenant"),
        ("provider" = Option<String>, Query, description = "Filter by provider"),
        ("action_type" = Option<String>, Query, description = "Filter by action type"),
        ("outcome" = Option<String>, Query, description = "Filter by outcome"),
        ("interval" = Option<AnalyticsInterval>, Query, description = "Time bucket interval (default: daily)"),
        ("from" = Option<String>, Query, description = "Start of time range (RFC 3339)"),
        ("to" = Option<String>, Query, description = "End of time range (RFC 3339)"),
        ("group_by" = Option<String>, Query, description = "Dimension to group by"),
        ("top_n" = Option<usize>, Query, description = "Number of top entries (default 10)"),
    ),
    responses(
        (status = 200, description = "Analytics results", body = acteon_core::AnalyticsResponse),
        (status = 404, description = "Analytics not available", body = ErrorResponse)
    )
)]
pub async fn query_analytics(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Query(params): Query<AnalyticsParams>,
) -> impl IntoResponse {
    let Some(ref analytics) = state.analytics else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                error: "analytics is not available (audit may be disabled)".into(),
            })),
        );
    };

    // Enforce tenant access.
    if let Some(ref requested_tenant) = params.tenant
        && let Some(allowed) = identity.allowed_tenants()
        && !allowed.contains(&requested_tenant.as_str())
    {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!(ErrorResponse {
                error: format!("no grant covers tenant={requested_tenant}"),
            })),
        );
    }

    let mut query = AnalyticsQuery {
        metric: params.metric,
        namespace: params.namespace,
        tenant: params.tenant,
        provider: params.provider,
        action_type: params.action_type,
        outcome: params.outcome,
        interval: params.interval.unwrap_or(AnalyticsInterval::Daily),
        from: params.from,
        to: params.to,
        group_by: params.group_by,
        top_n: params.top_n,
    };

    // Inject single-tenant caller's tenant if not specified.
    if query.tenant.is_none()
        && let Some(allowed) = identity.allowed_tenants()
        && allowed.len() == 1
    {
        query.tenant = Some(allowed[0].to_owned());
    }

    match analytics.query_analytics(&query).await {
        Ok(response) => (StatusCode::OK, Json(serde_json::json!(response))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse {
                error: e.to_string(),
            })),
        ),
    }
}
