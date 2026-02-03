use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;

use acteon_core::{Action, ActionOutcome};

use crate::auth::identity::CallerIdentity;
use crate::auth::role::Permission;
use crate::error::ServerError;

use super::schemas::ErrorResponse;
use super::AppState;

/// `POST /v1/dispatch` -- dispatch a single action through the gateway pipeline.
///
/// Expects a JSON body that deserializes to an [`Action`]. Returns the
/// resulting [`ActionOutcome`](acteon_core::ActionOutcome) as JSON.
#[utoipa::path(
    post,
    path = "/v1/dispatch",
    tag = "Dispatch",
    summary = "Dispatch action",
    description = "Sends a single action through the gateway pipeline (lock, rules, execute) and returns the outcome.",
    request_body(content = Action, description = "Action to dispatch"),
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

    // Check per-tenant rate limit if enabled.
    if let Some(ref limiter) = state.rate_limiter
        && limiter.config().tenants.enabled
            && let Err(e) = limiter.check_tenant_limit(&action.tenant).await {
                return Err(ServerError::RateLimited {
                    retry_after: e.retry_after,
                });
            }

    let caller = identity.to_caller();
    let gw = state.gateway.read().await;
    match gw.dispatch(action, Some(&caller)).await {
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
#[utoipa::path(
    post,
    path = "/v1/dispatch/batch",
    tag = "Dispatch",
    summary = "Batch dispatch",
    description = "Dispatches multiple actions through the gateway pipeline and returns an array of outcomes or errors.",
    request_body(content = Vec<Action>, description = "Actions to dispatch"),
    responses(
        (status = 200, description = "Array of dispatch outcomes", body = Vec<serde_json::Value>)
    )
)]
pub async fn dispatch_batch(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
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

    // Check per-tenant rate limits for all tenants in the batch if enabled.
    if let Some(ref limiter) = state.rate_limiter
        && limiter.config().tenants.enabled {
            // Collect unique tenants to avoid duplicate checks.
            let mut checked_tenants = std::collections::HashSet::new();
            for action in &actions {
                if checked_tenants.insert(&action.tenant)
                    && let Err(e) = limiter.check_tenant_limit(&action.tenant).await {
                        return Err(ServerError::RateLimited {
                            retry_after: e.retry_after,
                        });
                    }
            }
        }

    let caller = identity.to_caller();
    let gw = state.gateway.read().await;
    let results = gw.dispatch_batch(actions, Some(&caller)).await;

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
