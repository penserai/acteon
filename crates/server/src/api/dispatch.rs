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

/// Maximum number of actions allowed in a single batch dispatch request.
///
/// Prevents resource exhaustion from a single oversized request that could
/// consume unbounded memory and CPU. Callers that need to dispatch more
/// actions should split them across multiple requests.
const MAX_BATCH_SIZE: usize = 1_000;

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
#[allow(clippy::too_many_lines)]
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
    if !identity.is_authorized(
        &action.tenant,
        &action.namespace,
        &action.provider,
        &action.action_type,
    ) {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!(ErrorResponse {
                error: format!(
                    "forbidden: no grant covers tenant={}, namespace={}, provider={}, action={}",
                    action.tenant, action.namespace, action.provider, action.action_type
                ),
            })),
        ));
    }

    // Verify action signature if signing is enabled. Every branch
    // (pass, reject, allow-unsigned) bumps a gateway metric so
    // operators can alert on a spike in invalid/unknown/scope-denied
    // signatures post-rotation. Metric bumps hit `state.metrics`
    // directly (atomic counters) so the signing check never has to
    // take the gateway RwLock.
    if let Some(ref verifier) = state.signature_verifier {
        let outcome = verifier.verify_action(&action);
        outcome.record_metric(&state.metrics);
        // Rejection paths (and InternalError) emit a structured
        // tracing::warn with the full variant + signer + kid + scope
        // context. The HTTP body, by contrast, deliberately returns a
        // uniform "signature verification failed for signer X"
        // message so a probing caller can't tell UnknownSigner apart
        // from InvalidSignature.
        outcome.log_rejection();
        if let Some(msg) = outcome.internal_error_message() {
            return Ok((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse { error: msg })),
            ));
        }
        if let Some(err) = outcome.error_message() {
            return Ok((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!(ErrorResponse { error: err })),
            ));
        }
    }

    // Replay protection: reject if this action ID has already been dispatched.
    // Uses check_and_set (atomic set-if-not-exists) to close the race window
    // between checking and marking. Returns false if the key already existed.
    // Replay protection is independent of signing — this path increments
    // `replay_rejected`, not any `signing_*` counter.
    if let Some((true, ttl)) = state.replay_protection {
        let gw = state.gateway.read().await;
        let replay_key = acteon_state::StateKey::new(
            action.namespace.clone(),
            action.tenant.clone(),
            acteon_state::KeyKind::Custom("action_replay".into()),
            action.id.to_string(),
        );
        if let Ok(false) = gw
            .state_store()
            .check_and_set(&replay_key, "1", Some(std::time::Duration::from_secs(ttl)))
            .await
        {
            // Already existed — replay detected.
            drop(gw);
            state.metrics.increment_replay_rejected();
            return Ok((
                StatusCode::CONFLICT,
                Json(serde_json::json!(ErrorResponse {
                    error: format!(
                        "replay rejected: action ID '{}' has already been dispatched",
                        action.id
                    ),
                })),
            ));
        }
        // Ok(true) = fresh ID claimed; Err(_) = state store unavailable, fail open.
        drop(gw);
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

    // Acquire a permit from the global dispatch semaphore to limit concurrency.
    let _permit = state.dispatch_semaphore.try_acquire().map_err(|_| {
        ServerError::RateLimited {
            retry_after: 1, // Suggest retry after 1 second
        }
    })?;

    let caller = identity.to_caller();
    let mut action = action;
    action.trace_context = super::trace_context::capture_trace_context();

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
/// When signing is enabled, each action is verified independently and a
/// failed signature rejects only that one entry — the rest of the batch
/// continues through the pipeline. The response preserves the input
/// ordering so callers can match errors to their submitted actions by
/// index. An internal crypto error on any action fails the whole batch
/// with HTTP 500 since that indicates a server bug, not a caller issue.
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
#[allow(clippy::too_many_lines)]
pub async fn dispatch_batch(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Query(query): Query<DispatchQuery>,
    Json(actions): Json<Vec<Action>>,
) -> Result<impl IntoResponse, ServerError> {
    // Enforce maximum batch size to prevent resource exhaustion.
    if actions.len() > MAX_BATCH_SIZE {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(vec![serde_json::json!(ErrorResponse {
                error: format!(
                    "batch size {} exceeds maximum of {MAX_BATCH_SIZE}",
                    actions.len()
                ),
            })]),
        ));
    }

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
        if !identity.is_authorized(
            &action.tenant,
            &action.namespace,
            &action.provider,
            &action.action_type,
        ) {
            return Ok((
                StatusCode::FORBIDDEN,
                Json(vec![serde_json::json!(ErrorResponse {
                    error: format!(
                        "forbidden: no grant covers tenant={}, namespace={}, provider={}, action={}",
                        action.tenant, action.namespace, action.provider, action.action_type,
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

    // Verify action signatures if signing is enabled. Verification
    // runs per-action so a single bad signature rejects only its own
    // entry — the rest of the batch continues through the pipeline.
    // Record per-action metrics and emit a structured log line on
    // every rejection (mirrors the single-dispatch path).
    //
    // `signing_rejections[i] == Some(msg)` means action `i` failed
    // verification and should surface `msg` at that index in the
    // response; `None` means it passes to the gateway.
    let mut signing_rejections: Vec<Option<String>> = vec![None; actions.len()];
    if let Some(ref verifier) = state.signature_verifier {
        for (idx, action) in actions.iter().enumerate() {
            let outcome = verifier.verify_action(action);
            outcome.record_metric(&state.metrics);
            outcome.log_rejection();
            // An unexpected crypto error is a server-side bug; fail
            // the whole batch with 500 rather than leaking a partial
            // success for something the operator should investigate.
            if let Some(msg) = outcome.internal_error_message() {
                return Ok((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(vec![serde_json::json!(ErrorResponse { error: msg })]),
                ));
            }
            if let Some(err) = outcome.error_message() {
                signing_rejections[idx] = Some(err);
            }
        }
    }

    // Acquire permits from the global dispatch semaphore only for the
    // actions we'll actually dispatch — rejected entries don't tie up
    // a permit. When every action is rejected the semaphore is not
    // touched at all.
    let passing_count = signing_rejections.iter().filter(|r| r.is_none()).count();
    let _permits = if passing_count > 0 {
        Some(
            state
                .dispatch_semaphore
                .try_acquire_many(u32::try_from(passing_count).unwrap_or(u32::MAX))
                .map_err(|_| ServerError::RateLimited { retry_after: 1 })?,
        )
    } else {
        None
    };

    let caller = identity.to_caller();
    let trace_context = super::trace_context::capture_trace_context();

    // Split the input: passing actions go to the gateway in order,
    // rejected ones stay behind and slot back by index afterwards.
    let passing_actions: Vec<Action> = actions
        .into_iter()
        .enumerate()
        .filter_map(|(idx, mut a)| {
            if signing_rejections[idx].is_some() {
                None
            } else {
                a.trace_context.clone_from(&trace_context);
                Some(a)
            }
        })
        .collect();

    let dispatch_results = if passing_actions.is_empty() {
        Vec::new()
    } else {
        let gw = state.gateway.read().await;
        if query.dry_run {
            gw.dispatch_batch_dry_run(passing_actions, Some(&caller))
                .await
        } else {
            gw.dispatch_batch(passing_actions, Some(&caller)).await
        }
    };

    // Merge gateway results back with signing rejections. The
    // gateway results are in `passing_actions` order; walk the
    // original index space and pull from either source.
    let mut results_iter = dispatch_results.into_iter();
    let body: Vec<serde_json::Value> = signing_rejections
        .into_iter()
        .map(|rejection| match rejection {
            Some(msg) => serde_json::json!(ErrorResponse { error: msg }),
            None => match results_iter.next() {
                Some(Ok(outcome)) => serde_json::json!(outcome),
                Some(Err(e)) => serde_json::json!(ErrorResponse {
                    error: e.to_string()
                }),
                // Unreachable: passing_count == results_iter.len()
                // by construction. Fall back to a neutral error
                // rather than panicking if invariants ever drift.
                None => serde_json::json!(ErrorResponse {
                    error: "internal: missing gateway result for passing action".to_owned(),
                }),
            },
        })
        .collect();

    Ok((StatusCode::OK, Json(body)))
}
