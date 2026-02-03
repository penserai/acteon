use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;

use acteon_audit::record::AuditQuery;

use crate::auth::identity::CallerIdentity;

use super::schemas::ErrorResponse;
use super::AppState;

/// `GET /v1/audit` -- query audit records with filters and pagination.
#[utoipa::path(
    get,
    path = "/v1/audit",
    tag = "Audit",
    summary = "Query audit records",
    description = "Search audit records with optional filters for namespace, tenant, provider, outcome, verdict, time range, and pagination.",
    params(
        ("namespace" = Option<String>, Query, description = "Filter by namespace"),
        ("tenant" = Option<String>, Query, description = "Filter by tenant"),
        ("provider" = Option<String>, Query, description = "Filter by provider"),
        ("action_type" = Option<String>, Query, description = "Filter by action type"),
        ("outcome" = Option<String>, Query, description = "Filter by outcome"),
        ("verdict" = Option<String>, Query, description = "Filter by verdict"),
        ("matched_rule" = Option<String>, Query, description = "Filter by matched rule name"),
        ("from" = Option<String>, Query, description = "Start of time range (RFC 3339)"),
        ("to" = Option<String>, Query, description = "End of time range (RFC 3339)"),
        ("limit" = Option<u32>, Query, description = "Max records to return (default 50, max 1000)"),
        ("offset" = Option<u32>, Query, description = "Number of records to skip"),
    ),
    responses(
        (status = 200, description = "Audit records matching query", body = acteon_audit::AuditPage),
        (status = 404, description = "Audit not enabled", body = ErrorResponse)
    )
)]
pub async fn query_audit(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Query(mut query): Query<AuditQuery>,
) -> impl IntoResponse {
    let Some(ref audit) = state.audit else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                error: "audit is not enabled".into(),
            })),
        );
    };

    // Restrict query to only tenants the caller has access to.
    // If the caller has a specific tenant filter, verify it's covered by grants.
    if let Some(ref requested_tenant) = query.tenant {
        if let Some(allowed) = identity.allowed_tenants() {
            if !allowed.contains(&requested_tenant.as_str()) {
                return (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!(ErrorResponse {
                        error: format!("no grant covers tenant={requested_tenant}"),
                    })),
                );
            }
        }
    } else if let Some(allowed) = identity.allowed_tenants() {
        // No tenant filter requested but caller is scoped — inject first allowed tenant.
        // For multi-tenant callers, the API requires an explicit tenant filter.
        if allowed.len() == 1 {
            query.tenant = Some(allowed[0].to_owned());
        }
        // If multiple tenants allowed and no filter, we let the query through
        // and results will contain records from all their granted tenants.
    }

    match audit.query(&query).await {
        Ok(page) => (StatusCode::OK, Json(serde_json::json!(page))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse {
                error: e.to_string(),
            })),
        ),
    }
}

/// `GET /v1/audit/{action_id}` -- get the most recent audit record for an action.
#[utoipa::path(
    get,
    path = "/v1/audit/{action_id}",
    tag = "Audit",
    summary = "Get audit record by action ID",
    description = "Returns the most recent audit record for the given action ID.",
    params(
        ("action_id" = String, Path, description = "Action ID to look up")
    ),
    responses(
        (status = 200, description = "Audit record found", body = acteon_audit::AuditRecord),
        (status = 404, description = "Not found or audit not enabled", body = ErrorResponse)
    )
)]
pub async fn get_audit_by_action(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(action_id): Path<String>,
) -> impl IntoResponse {
    let Some(ref audit) = state.audit else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                error: "audit is not enabled".into(),
            })),
        );
    };

    match audit.get_by_action_id(&action_id).await {
        Ok(Some(record)) => {
            // Verify the caller has access to this record's tenant/namespace.
            if !identity.is_authorized(&record.tenant, &record.namespace, &record.action_type) {
                return (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!(ErrorResponse {
                        error: "no grant covers this audit record".into(),
                    })),
                );
            }
            (StatusCode::OK, Json(serde_json::json!(record)))
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                error: format!("no audit record found for action: {action_id}"),
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse {
                error: e.to_string(),
            })),
        ),
    }
}
