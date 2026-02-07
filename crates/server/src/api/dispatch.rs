use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;

use acteon_core::{Action, ActionOutcome};

use crate::auth::identity::CallerIdentity;
use crate::auth::role::Permission;
use crate::error::ServerError;

use super::AppState;
use super::schemas::ErrorResponse;

/// Query parameters for dispatch endpoints.
#[derive(Debug, Deserialize, Default)]
pub struct DispatchQuery {
    /// When `true`, evaluates rules and returns the verdict without executing
    /// the action, recording state, or emitting audit records.
    #[serde(default)]
    pub dry_run: bool,
}

/// `POST /v1/dispatch` -- dispatch a single action through the gateway pipeline.
///
/// Expects a JSON body that deserializes to an [`Action`]. Returns the
/// resulting [`ActionOutcome`] as JSON.
///
/// Pass `?dry_run=true` to evaluate rules without executing the action.
#[utoipa::path(
    post,
    path = "/v1/dispatch",
    tag = "Dispatch",
    summary = "Dispatch action",
    description = "Sends a single action through the gateway pipeline (lock, rules, execute) and returns the outcome. Pass ?dry_run=true to evaluate rules without executing.",
    request_body(content = Action, description = "Action to dispatch"),
    params(
        ("dry_run" = Option<bool>, Query, description = "Evaluate rules without executing the action")
    ),
    responses(
        (status = 200, description = "Action dispatched successfully", body = ActionOutcome),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub async fn dispatch(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Query(query): Query<DispatchQuery>,
    Json(action): Json<Action>,
) -> Result<impl IntoResponse, ServerError> {
    // Check role permission.
    if !identity.role.has_permission(Permission::Dispatch) {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!(ErrorResponse {
                error: "insufficient permissions: dispatch requires admin or operator role".into(),
            })),
        ));
    }

    // Check grant-level authorization.
    if !identity.is_authorized(&action.tenant, &action.namespace, &action.action_type) {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!(ErrorResponse {
                error: format!(
                    "forbidden: no grant covers tenant={}, namespace={}, action={}",
                    action.tenant, action.namespace, action.action_type
                ),
            })),
        ));
    }

    // Check per-tenant rate limit if enabled (skip for dry-run).
    if !query.dry_run
        && let Some(ref limiter) = state.rate_limiter
        && limiter.config().tenants.enabled
        && let Err(e) = limiter.check_tenant_limit(&action.tenant).await
    {
        return Err(ServerError::RateLimited {
            retry_after: e.retry_after,
        });
    }

    let caller = identity.to_caller();
    let gw = state.gateway.read().await;
    let result = if query.dry_run {
        gw.dispatch_dry_run(action, Some(&caller)).await
    } else {
        gw.dispatch(action, Some(&caller)).await
    };

    match result {
        Ok(outcome) => Ok((StatusCode::OK, Json(serde_json::json!(outcome)))),
        Err(e) => Ok((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse {
                error: e.to_string()
            })),
        )),
    }
}

/// `POST /v1/dispatch/batch` -- dispatch multiple actions and collect results.
///
/// Expects a JSON array of [`Action`] objects. Returns an array of results,
/// where each element is either an `ActionOutcome` or an error object.
///
/// Pass `?dry_run=true` to evaluate rules without executing any actions.
#[utoipa::path(
    post,
    path = "/v1/dispatch/batch",
    tag = "Dispatch",
    summary = "Batch dispatch",
    description = "Dispatches multiple actions through the gateway pipeline and returns an array of outcomes or errors. Pass ?dry_run=true to evaluate rules without executing.",
    request_body(content = Vec<Action>, description = "Actions to dispatch"),
    params(
        ("dry_run" = Option<bool>, Query, description = "Evaluate rules without executing any actions")
    ),
    responses(
        (status = 200, description = "Array of dispatch outcomes", body = Vec<serde_json::Value>)
    )
)]
pub async fn dispatch_batch(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Query(query): Query<DispatchQuery>,
    Json(actions): Json<Vec<Action>>,
) -> Result<impl IntoResponse, ServerError> {
    // Check role permission.
    if !identity.role.has_permission(Permission::Dispatch) {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(vec![serde_json::json!(ErrorResponse {
                error: "insufficient permissions: dispatch requires admin or operator role".into(),
            })]),
        ));
    }

    // Check each action individually for grant authorization.
    for action in &actions {
        if !identity.is_authorized(&action.tenant, &action.namespace, &action.action_type) {
            return Ok((
                StatusCode::FORBIDDEN,
                Json(vec![serde_json::json!(ErrorResponse {
                    error: format!(
                        "forbidden: no grant covers tenant={}, namespace={}, action={}",
                        action.tenant, action.namespace, action.action_type
                    ),
                })]),
            ));
        }
    }

    // Check per-tenant rate limits for all tenants in the batch if enabled (skip for dry-run).
    if !query.dry_run
        && let Some(ref limiter) = state.rate_limiter
        && limiter.config().tenants.enabled
    {
        // Collect unique tenants to avoid duplicate checks.
        let mut checked_tenants = std::collections::HashSet::new();
        for action in &actions {
            if checked_tenants.insert(&action.tenant)
                && let Err(e) = limiter.check_tenant_limit(&action.tenant).await
            {
                return Err(ServerError::RateLimited {
                    retry_after: e.retry_after,
                });
            }
        }
    }

    let caller = identity.to_caller();
    let gw = state.gateway.read().await;
    let results = if query.dry_run {
        gw.dispatch_batch_dry_run(actions, Some(&caller)).await
    } else {
        gw.dispatch_batch(actions, Some(&caller)).await
    };

    let body: Vec<serde_json::Value> = results
        .into_iter()
        .map(|r| match r {
            Ok(outcome) => serde_json::json!(outcome),
            Err(e) => serde_json::json!(ErrorResponse {
                error: e.to_string()
            }),
        })
        .collect();

    Ok((StatusCode::OK, Json(body)))
}
