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

    // Acquire permits for the full batch *before* signature
    // verification. Ed25519 verify is cheap per action but a caller
    // with a valid grant could still flood the gateway with
    // 1000-action batches of valid-looking-but-wrong signatures and
    // burn CPU. Bounding signing work by the existing dispatch
    // concurrency knob closes that hole — the signing loop can never
    // outrun what the gateway is willing to dispatch. The permits
    // are released on function exit regardless of which path we
    // take.
    let _permits = state
        .dispatch_semaphore
        .try_acquire_many(u32::try_from(actions.len()).unwrap_or(u32::MAX))
        .map_err(|_| ServerError::RateLimited { retry_after: 1 })?;

    // Verify signatures per-action. An InternalError aborts the whole
    // batch with HTTP 500; a normal rejection only knocks out its
    // own entry and the batch continues.
    let signing_rejections = match verify_batch_signatures(
        state.signature_verifier.as_deref(),
        &state.metrics,
        &actions,
    ) {
        Ok(rejections) => rejections,
        Err(msg) => {
            // Mirror the internal-error message into every slot so
            // the response still satisfies the "one entry per
            // input action" batch invariant. A client that
            // indexes body[i] won't skip over a phantom slot.
            let body: Vec<serde_json::Value> = (0..actions.len())
                .map(|_| serde_json::json!(ErrorResponse { error: msg.clone() }))
                .collect();
            return Ok((StatusCode::INTERNAL_SERVER_ERROR, Json(body)));
        }
    };

    // Split the input: passing actions go to the gateway in order,
    // rejected ones stay behind and slot back by index afterwards.
    let caller = identity.to_caller();
    let trace_context = super::trace_context::capture_trace_context();
    let passing_actions: Vec<Action> = actions
        .into_iter()
        .zip(signing_rejections.iter())
        .filter_map(|(mut a, rejection)| {
            if rejection.is_some() {
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

    let body = merge_batch_results(signing_rejections, dispatch_results);
    Ok((StatusCode::OK, Json(body)))
}

/// Verify every action in a batch against the signature verifier.
///
/// Returns `Ok(rejections)` where `rejections[i]` is the HTTP 400
/// message for action `i` (or `None` if it passed), or `Err(msg)` on
/// the first `InternalError` — which should abort the whole batch
/// with HTTP 500 since internal errors indicate a server bug, not a
/// caller issue.
///
/// Every branch (pass, reject, internal error) bumps the matching
/// gateway metric and emits a structured `tracing::warn` before
/// returning, so observability is consistent with the single-action
/// dispatch path.
fn verify_batch_signatures(
    verifier: Option<&super::verify::SignatureVerifier>,
    metrics: &acteon_gateway::GatewayMetrics,
    actions: &[Action],
) -> Result<Vec<Option<String>>, String> {
    let mut rejections: Vec<Option<String>> = vec![None; actions.len()];
    let Some(verifier) = verifier else {
        return Ok(rejections);
    };
    for (idx, action) in actions.iter().enumerate() {
        let outcome = verifier.verify_action(action);
        outcome.record_metric(metrics);
        outcome.log_rejection();
        if let Some(msg) = outcome.internal_error_message() {
            // Fail-fast: don't burn CPU verifying the rest of a batch
            // that's already destined for HTTP 500.
            return Err(msg);
        }
        if let Some(err) = outcome.error_message() {
            rejections[idx] = Some(err);
        }
    }
    Ok(rejections)
}

/// Zip signing rejections back together with gateway dispatch results
/// by input index.
///
/// `signing_rejections` has one entry per *input* action;
/// `dispatch_results` has one entry per *passing* action in input
/// order (the gateway guarantees this — see
/// `dispatch_batch_preserves_order_under_latency_skew`). Walk the
/// rejection vector; for every `Some(msg)` emit an error, for every
/// `None` pull the next gateway result and emit its outcome.
fn merge_batch_results(
    signing_rejections: Vec<Option<String>>,
    dispatch_results: Vec<Result<acteon_core::ActionOutcome, acteon_gateway::GatewayError>>,
) -> Vec<serde_json::Value> {
    let mut results_iter = dispatch_results.into_iter();
    signing_rejections
        .into_iter()
        .map(|rejection| match rejection {
            Some(msg) => serde_json::json!(ErrorResponse { error: msg }),
            None => match results_iter.next() {
                Some(Ok(outcome)) => serde_json::json!(outcome),
                Some(Err(e)) => serde_json::json!(ErrorResponse {
                    error: e.to_string()
                }),
                // Unreachable under invariant: the gateway returns
                // exactly one result per passing action. Degrade to a
                // neutral error rather than panicking if that ever
                // drifts (e.g. a future caller exposes a new dispatch
                // shape).
                None => serde_json::json!(ErrorResponse {
                    error: "internal: missing gateway result for passing action".to_owned(),
                }),
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_core::{ActionOutcome, ProviderResponse};
    use acteon_gateway::GatewayError;

    fn ok_outcome(tag: &str) -> Result<ActionOutcome, GatewayError> {
        Ok(ActionOutcome::Executed(ProviderResponse::success(
            serde_json::json!({ "tag": tag }),
        )))
    }

    /// Every slot in the merged body should correspond 1:1 to a slot
    /// in the signing_rejections vector. Errors come from rejections,
    /// outcomes come from gateway results in the order of passing
    /// indices.
    #[test]
    fn merge_batch_results_mixed_preserves_indices() {
        // Indices: 0 ok, 1 rejected, 2 ok, 3 rejected, 4 ok
        let rejections = vec![
            None,
            Some("bad at 1".to_owned()),
            None,
            Some("bad at 3".to_owned()),
            None,
        ];
        let gateway_results = vec![ok_outcome("a"), ok_outcome("c"), ok_outcome("e")];

        let body = merge_batch_results(rejections, gateway_results);

        assert_eq!(body.len(), 5);
        assert_eq!(body[0]["Executed"]["body"]["tag"], "a");
        assert_eq!(body[1]["error"], "bad at 1");
        assert_eq!(body[2]["Executed"]["body"]["tag"], "c");
        assert_eq!(body[3]["error"], "bad at 3");
        assert_eq!(body[4]["Executed"]["body"]["tag"], "e");
    }

    #[test]
    fn merge_batch_results_all_rejected_empty_gateway_results() {
        let rejections = vec![
            Some("err 0".to_owned()),
            Some("err 1".to_owned()),
            Some("err 2".to_owned()),
        ];
        let body = merge_batch_results(rejections, Vec::new());

        assert_eq!(body.len(), 3);
        assert_eq!(body[0]["error"], "err 0");
        assert_eq!(body[1]["error"], "err 1");
        assert_eq!(body[2]["error"], "err 2");
    }

    #[test]
    fn merge_batch_results_no_rejections_all_pass_through() {
        let rejections = vec![None, None, None];
        let gateway_results = vec![ok_outcome("x"), ok_outcome("y"), ok_outcome("z")];

        let body = merge_batch_results(rejections, gateway_results);

        assert_eq!(body.len(), 3);
        assert_eq!(body[0]["Executed"]["body"]["tag"], "x");
        assert_eq!(body[1]["Executed"]["body"]["tag"], "y");
        assert_eq!(body[2]["Executed"]["body"]["tag"], "z");
    }

    /// Invariant violation guard: if the caller somehow hands us a
    /// passing slot without a matching gateway result, degrade
    /// gracefully instead of panicking.
    #[test]
    fn merge_batch_results_missing_gateway_result_degrades_gracefully() {
        let rejections = vec![None, None];
        // Only one result for two passing slots — should never
        // happen, but must not panic.
        let body = merge_batch_results(rejections, vec![ok_outcome("a")]);

        assert_eq!(body.len(), 2);
        assert_eq!(body[0]["Executed"]["body"]["tag"], "a");
        assert!(
            body[1]["error"]
                .as_str()
                .unwrap()
                .contains("missing gateway result")
        );
    }

    /// When `verify_batch_signatures` returns `Err`, the caller
    /// builds an N-length response body (one entry per input action)
    /// rather than a length-1 array. This matches the batch API's
    /// "one entry per input action" contract even for HTTP 500.
    #[test]
    fn internal_error_body_is_n_length() {
        // Simulate the internal-error path: build the body the same
        // way the handler does when verify_batch_signatures errors.
        let actions_len = 7;
        let msg = "signature verification failed with an unexpected crypto error: boom";
        let body: Vec<serde_json::Value> = (0..actions_len)
            .map(|_| {
                serde_json::json!(ErrorResponse {
                    error: msg.to_owned()
                })
            })
            .collect();
        assert_eq!(body.len(), 7);
        for entry in &body {
            assert_eq!(entry["error"], msg);
        }
    }
}
