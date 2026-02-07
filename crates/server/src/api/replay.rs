//! Action replay API endpoints.
//!
//! Replay actions from the audit trail by reconstructing the original action
//! from the stored payload and dispatching it through the gateway pipeline.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use acteon_audit::record::AuditQuery;
use acteon_core::Action;

/// Maximum number of concurrent replay dispatches.
const REPLAY_CONCURRENCY: usize = 32;

use crate::auth::identity::CallerIdentity;
use crate::auth::role::Permission;

use super::AppState;
use super::schemas::ErrorResponse;

/// Result of replaying a single action.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReplayResult {
    /// The original action ID from the audit record.
    #[schema(example = "550e8400-e29b-41d4-a716-446655440000")]
    pub original_action_id: String,
    /// The new action ID assigned to the replayed action.
    #[schema(example = "661f9511-f30c-52e5-b827-557766551111")]
    pub new_action_id: String,
    /// Whether the replay succeeded.
    #[schema(example = true)]
    pub success: bool,
    /// Error message if the replay failed.
    pub error: Option<String>,
}

/// Summary response for bulk replay operations.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReplaySummary {
    /// Number of actions successfully replayed.
    #[schema(example = 8)]
    pub replayed: usize,
    /// Number of actions that failed to replay.
    #[schema(example = 1)]
    pub failed: usize,
    /// Number of records skipped (no stored payload).
    #[schema(example = 2)]
    pub skipped: usize,
    /// Per-action results.
    pub results: Vec<ReplayResult>,
}

/// Reconstruct an [`Action`] from an audit record's stored fields.
///
/// Returns `None` if the record has no stored payload (privacy mode was on).
fn reconstruct_action(record: &acteon_audit::AuditRecord) -> Option<Action> {
    let payload = record.action_payload.as_ref()?;

    let mut action = Action::new(
        record.namespace.as_str(),
        record.tenant.as_str(),
        record.provider.as_str(),
        record.action_type.as_str(),
        payload.clone(),
    );

    // Restore metadata and extra Action fields from the audit record.
    if let Some(labels) = record.metadata.as_object() {
        for (k, v) in labels {
            // Skip system-prefixed keys; they are restored as typed fields below.
            if k.starts_with("__") {
                continue;
            }
            // Convert non-string values to their string representation instead
            // of silently dropping them.
            let s = if let Some(s) = v.as_str() {
                s.to_owned()
            } else {
                v.to_string()
            };
            action.metadata.labels.insert(k.clone(), s);
        }

        // Restore extra Action fields stored with __ prefix.
        if let Some(k) = labels.get("__dedup_key").and_then(|v| v.as_str()) {
            action = action.with_dedup_key(k);
        }
        if let Some(f) = labels.get("__fingerprint").and_then(|v| v.as_str()) {
            action = action.with_fingerprint(f);
        }
        if let Some(s) = labels.get("__status").and_then(|v| v.as_str()) {
            action = action.with_status(s);
        }
        if let Some(t) = labels
            .get("__starts_at")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok())
        {
            action = action.with_starts_at(t);
        }
        if let Some(t) = labels
            .get("__ends_at")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok())
        {
            action = action.with_ends_at(t);
        }
    }
    action
        .metadata
        .labels
        .insert("replayed_from".to_owned(), record.action_id.clone());

    Some(action)
}

/// `POST /v1/audit/{action_id}/replay` -- replay a single action from the audit trail.
#[utoipa::path(
    post,
    path = "/v1/audit/{action_id}/replay",
    tag = "Audit",
    summary = "Replay action by audit action ID",
    description = "Reconstructs the original action from the audit record and dispatches it through the gateway pipeline. The replayed action receives a new ID and includes `replayed_from` metadata pointing to the original.",
    params(
        ("action_id" = String, Path, description = "Original action ID to replay")
    ),
    responses(
        (status = 200, description = "Action replayed", body = ReplayResult),
        (status = 404, description = "Audit record not found or audit not enabled", body = ErrorResponse),
        (status = 422, description = "No stored payload available for replay", body = ErrorResponse)
    )
)]
pub async fn replay_action(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(action_id): Path<String>,
) -> impl IntoResponse {
    // Check role permission (replay requires dispatch permission).
    if !identity.role.has_permission(Permission::Dispatch) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!(ErrorResponse {
                error: "insufficient permissions: replay requires admin or operator role".into(),
            })),
        );
    }

    let Some(ref audit) = state.audit else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                error: "audit is not enabled".into(),
            })),
        );
    };

    // Fetch the audit record.
    let record = match audit.get_by_action_id(&action_id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!(ErrorResponse {
                    error: format!("no audit record found for action: {action_id}"),
                })),
            );
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse {
                    error: e.to_string(),
                })),
            );
        }
    };

    // Verify the caller has access to this record's tenant/namespace.
    if !identity.is_authorized(&record.tenant, &record.namespace, &record.action_type) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!(ErrorResponse {
                error: "no grant covers this audit record".into(),
            })),
        );
    }

    // Reconstruct the action from the audit record.
    let Some(action) = reconstruct_action(&record) else {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!(ErrorResponse {
                error: "audit record has no stored payload (privacy mode was enabled)".into(),
            })),
        );
    };

    let new_action_id = action.id.to_string();
    let caller = identity.to_caller();
    let gw = state.gateway.read().await;

    let result = match gw.dispatch(action, Some(&caller)).await {
        Ok(_) => ReplayResult {
            original_action_id: action_id,
            new_action_id,
            success: true,
            error: None,
        },
        Err(e) => ReplayResult {
            original_action_id: action_id,
            new_action_id,
            success: false,
            error: Some(e.to_string()),
        },
    };

    (StatusCode::OK, Json(serde_json::json!(result)))
}

/// Query parameters for bulk replay.
#[derive(Debug, Default, Deserialize)]
pub struct ReplayQuery {
    /// Filter by namespace.
    pub namespace: Option<String>,
    /// Filter by tenant.
    pub tenant: Option<String>,
    /// Filter by provider.
    pub provider: Option<String>,
    /// Filter by action type.
    pub action_type: Option<String>,
    /// Filter by outcome (e.g., `"failed"`, `"suppressed"`).
    pub outcome: Option<String>,
    /// Filter by verdict.
    pub verdict: Option<String>,
    /// Filter by matched rule name.
    pub matched_rule: Option<String>,
    /// Only records dispatched at or after this time (RFC 3339).
    pub from: Option<chrono::DateTime<chrono::Utc>>,
    /// Only records dispatched at or before this time (RFC 3339).
    pub to: Option<chrono::DateTime<chrono::Utc>>,
    /// Maximum number of records to replay (default 50, max 1000).
    pub limit: Option<u32>,
}

/// `POST /v1/audit/replay` -- bulk replay actions from the audit trail.
#[utoipa::path(
    post,
    path = "/v1/audit/replay",
    tag = "Audit",
    summary = "Bulk replay actions from audit trail",
    description = "Queries the audit trail with the given filters and replays each action that has a stored payload. Actions are dispatched through the full gateway pipeline with new IDs.",
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
        ("limit" = Option<u32>, Query, description = "Max records to replay (default 50, max 1000)"),
    ),
    responses(
        (status = 200, description = "Replay summary", body = ReplaySummary),
        (status = 404, description = "Audit not enabled", body = ErrorResponse)
    )
)]
#[allow(clippy::too_many_lines)]
pub async fn replay_audit(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Query(query): Query<ReplayQuery>,
) -> impl IntoResponse {
    // Check role permission.
    if !identity.role.has_permission(Permission::Dispatch) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!(ErrorResponse {
                error: "insufficient permissions: replay requires admin or operator role".into(),
            })),
        );
    }

    let Some(ref audit) = state.audit else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                error: "audit is not enabled".into(),
            })),
        );
    };

    // Build the audit query from replay parameters.
    let audit_query = AuditQuery {
        namespace: query.namespace,
        tenant: query.tenant,
        provider: query.provider,
        action_type: query.action_type,
        outcome: query.outcome,
        verdict: query.verdict,
        matched_rule: query.matched_rule,
        from: query.from,
        to: query.to,
        limit: Some(query.limit.unwrap_or(50).clamp(1, 1000)),
        ..AuditQuery::default()
    };

    let page = match audit.query(&audit_query).await {
        Ok(p) => p,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse {
                    error: e.to_string(),
                })),
            );
        }
    };

    let caller = identity.to_caller();

    // Process replays concurrently with bounded parallelism. The gateway read
    // lock is acquired per-dispatch (and released after each one) so rule
    // reloads are not blocked for the entire batch.
    let results: Vec<Option<ReplayResult>> = stream::iter(page.records)
        .map(|record| {
            let state = &state;
            let caller = &caller;
            let identity = &identity;
            async move {
                if !identity.is_authorized(&record.tenant, &record.namespace, &record.action_type) {
                    return None; // skipped
                }

                let Some(action) = reconstruct_action(&record) else {
                    return None; // skipped
                };

                let new_action_id = action.id.to_string();
                let gw = state.gateway.read().await;
                let result = match gw.dispatch(action, Some(caller)).await {
                    Ok(_) => ReplayResult {
                        original_action_id: record.action_id.clone(),
                        new_action_id,
                        success: true,
                        error: None,
                    },
                    Err(e) => ReplayResult {
                        original_action_id: record.action_id.clone(),
                        new_action_id,
                        success: false,
                        error: Some(e.to_string()),
                    },
                };
                Some(result)
            }
        })
        .buffer_unordered(REPLAY_CONCURRENCY)
        .collect()
        .await;

    let mut replayed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut replay_results = Vec::new();

    for item in results {
        match item {
            Some(r) if r.success => {
                replayed += 1;
                replay_results.push(r);
            }
            Some(r) => {
                failed += 1;
                replay_results.push(r);
            }
            None => {
                skipped += 1;
            }
        }
    }

    let summary = ReplaySummary {
        replayed,
        failed,
        skipped,
        results: replay_results,
    };

    (StatusCode::OK, Json(serde_json::json!(summary)))
}
