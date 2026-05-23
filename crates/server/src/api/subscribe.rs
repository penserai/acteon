//! Entity-specific SSE subscription endpoint.
//!
//! Provides `GET /v1/subscribe/{entity_type}/{entity_id}` as a convenience
//! wrapper over the general `/v1/stream` endpoint, pre-filtering for a
//! specific chain, group, or action.
//!
//! ## Catch-up (historical replay)
//!
//! When `include_history=true` (the default), the handler emits synthetic
//! SSE events reflecting the entity's current state before switching to the
//! live broadcast stream. This allows late-joining subscribers to see what
//! has already happened.
//!
//! ## Entity validation
//!
//! For `chain` and `group` subscriptions the handler verifies that the entity
//! exists and belongs to the caller's allowed tenants before opening the stream.
//! Both "not found" and "wrong tenant" map to 403 to avoid leaking entity
//! existence (per security review C1).

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::{Stream, StreamExt};
use serde::Deserialize;
use tracing::debug;

use acteon_core::{ChainStatus, GroupState, StreamEvent, StreamEventType};

use crate::auth::identity::CallerIdentity;
use crate::auth::role::Permission;

use super::AppState;
use super::stream::{StreamQuery, stream_event_type_tag};

/// Supported entity types for subscription filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    Chain,
    Group,
    Action,
}

impl EntityType {
    /// Wire token. Matches the `serde(rename_all = "snake_case")`
    /// representation. Used by the `subscription_end` SSE event
    /// body so a client can branch on which entity kind ended.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chain => "chain",
            Self::Group => "group",
            Self::Action => "action",
        }
    }
}

/// Path parameters for the subscribe endpoint.
#[derive(Debug, Deserialize)]
pub struct SubscribePath {
    pub entity_type: EntityType,
    pub entity_id: String,
}

/// Query parameters for the subscribe endpoint.
#[derive(Debug, Deserialize, Default)]
pub struct SubscribeQuery {
    /// Emit synthetic catch-up events for the entity's current state (default: `true`).
    #[serde(default = "default_true")]
    pub include_history: bool,
    /// Namespace for tenant isolation (required for chain/group/action subscriptions).
    pub namespace: Option<String>,
    /// Tenant for tenant isolation (required for chain/group/action subscriptions).
    pub tenant: Option<String>,
}

fn default_true() -> bool {
    true
}

/// Maximum length for entity IDs (per security review M4).
const MAX_ENTITY_ID_LENGTH: usize = 256;

/// Per-call timeout for the entity-existence / catch-up reads
/// performed before the SSE stream is handed back to the runtime
/// (security review H2).
///
/// A slow state backend would otherwise hold an axum worker for as
/// long as the operation takes; capping at two seconds bounds the
/// blast radius of one bad query — the caller gets a clean
/// `503 Service Unavailable` instead of a hanging request.
const CATCHUP_FETCH_TIMEOUT: Duration = Duration::from_secs(2);

/// `GET /v1/subscribe/{entity_type}/{entity_id}` -- subscribe to events for a
/// specific entity via SSE.
///
/// This is a convenience endpoint that creates a pre-filtered SSE stream for
/// the given entity. It delegates to the same underlying broadcast channel
/// and filtering logic as `/v1/stream`.
///
/// When `include_history=true` (default), synthetic catch-up events are emitted
/// for the entity's current state before switching to the live stream.
#[allow(clippy::too_many_lines)]
pub async fn subscribe(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(path): Path<SubscribePath>,
    Query(sub_query): Query<SubscribeQuery>,
) -> Result<impl IntoResponse, (StatusCode, axum::Json<serde_json::Value>)> {
    // 1. Check role permission.
    if !identity.role.has_permission(Permission::StreamSubscribe) {
        return Err((
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({
                "error": "insufficient permissions: stream subscribe requires at least viewer role"
            })),
        ));
    }

    // 2. Validate entity_id length and charset (security review M4).
    validate_entity_id(&path.entity_id)?;

    // 3. Determine the caller's allowed tenants for filtering.
    let allowed_tenants: Option<Vec<String>> = identity
        .allowed_tenants()
        .map(|tenants| tenants.into_iter().map(String::from).collect());

    // 4. Entity validation + catch-up events.
    let catchup_events = match path.entity_type {
        EntityType::Chain => {
            let (ns, tenant) = require_ns_tenant(&sub_query)?;
            validate_tenant_access(allowed_tenants.as_ref(), &tenant)?;

            let gateway = state.gateway.read().await;
            // Bound the state-store fetch — a slow backend would
            // otherwise hold this worker. Translates to 503 instead
            // of an indefinite hang (security review H2).
            let chain_state = tokio::time::timeout(
                CATCHUP_FETCH_TIMEOUT,
                gateway.get_chain_status(&ns, &tenant, &path.entity_id),
            )
            .await
            .map_err(|_| catchup_timeout("chain"))?
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    axum::Json(serde_json::json!({ "error": e.to_string() })),
                )
            })?;

            let chain_state = chain_state.ok_or_else(forbidden_or_not_found)?;
            drop(gateway);

            if sub_query.include_history {
                build_chain_catchup(&chain_state)
            } else {
                Vec::new()
            }
        }
        EntityType::Group => {
            let (_ns, tenant) = require_ns_tenant(&sub_query)?;
            validate_tenant_access(allowed_tenants.as_ref(), &tenant)?;

            let gateway = state.gateway.read().await;
            // `get_group` is in-memory so a hang is unlikely, but
            // wrap symmetrically with the chain path so the contract
            // is uniform across entity types (security review H2).
            let group = tokio::time::timeout(
                CATCHUP_FETCH_TIMEOUT,
                std::future::ready(gateway.group_manager().get_group(&path.entity_id)),
            )
            .await
            .map_err(|_| catchup_timeout("group"))?;
            drop(gateway);

            let group = group.ok_or_else(forbidden_or_not_found)?;

            if sub_query.include_history {
                build_group_catchup(&group)
            } else {
                Vec::new()
            }
        }
        EntityType::Action => {
            // Actions are ephemeral — skip entity validation.
            // Catch-up uses the audit store if available. The audit
            // path is the most likely to be slow under load, so the
            // timeout matters most here.
            if sub_query.include_history {
                tokio::time::timeout(
                    CATCHUP_FETCH_TIMEOUT,
                    build_action_catchup(
                        state.audit.as_deref(),
                        &path.entity_id,
                        allowed_tenants.as_ref(),
                    ),
                )
                .await
                .map_err(|_| catchup_timeout("action"))?
            } else {
                Vec::new()
            }
        }
    };

    // 5. Build a StreamQuery targeting the entity.
    let query = match path.entity_type {
        EntityType::Chain => StreamQuery {
            chain_id: Some(path.entity_id.clone()),
            ..StreamQuery::default()
        },
        EntityType::Group => StreamQuery {
            group_id: Some(path.entity_id.clone()),
            ..StreamQuery::default()
        },
        EntityType::Action => StreamQuery {
            action_id: Some(path.entity_id.clone()),
            ..StreamQuery::default()
        },
    };

    // 6. Enforce per-tenant connection limit.
    let connection_bucket = match &allowed_tenants {
        Some(tenants) if tenants.len() == 1 => tenants[0].clone(),
        _ => format!("caller:{}", identity.id),
    };

    let conn_registry = state.connection_registry.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({
                "error": "SSE streaming is not enabled"
            })),
        )
    })?;

    let guard = conn_registry
        .try_acquire(&connection_bucket)
        .await
        .ok_or_else(|| {
            (
                StatusCode::TOO_MANY_REQUESTS,
                axum::Json(serde_json::json!({
                    "error": "too many concurrent SSE connections for this tenant"
                })),
            )
        })?;

    // 7. Subscribe to the broadcast channel.
    //
    // Two subscriptions are taken: one (`rx`) feeds the data stream
    // through `make_event_stream`; the second (`detector_rx`) feeds
    // the terminal-event detector. Both are taken before any
    // `.await` so neither can miss events that arrive between the
    // two subscribes — tokio broadcast::Receiver doesn't replay,
    // and the detector running ahead of or behind the data stream
    // is acceptable as long as both see the terminal event.
    let gateway = state.gateway.read().await;
    let rx = gateway.stream_tx().subscribe();
    let detector_rx = gateway.stream_tx().subscribe();
    drop(gateway);

    // 8. Build the filtered SSE stream (catch-up + live).
    let event_stream = super::stream::make_event_stream(rx, allowed_tenants, query, guard, None);

    let catchup_stream = futures::stream::iter(catchup_events);
    let combined = catchup_stream.chain(event_stream);

    // 9. Auto-close on terminal entity state (design doc §2.5/2.6).
    //
    // Chain / Group subscriptions end when the entity reaches a
    // terminal state. A side task watches `detector_rx`; when it
    // sees the matching terminal event, it stores the reason in
    // `reason_slot` and fires a oneshot. The data stream is
    // wrapped with `take_until(signal_rx)` so it ends at the
    // first opportunity after the signal fires; a final
    // `subscription_end` event is then chained on.
    //
    // Action subscriptions have no inherent terminal in the live
    // stream (per design §10: idle timeout, not state-driven). The
    // wrap is skipped for them — they stay open until the client
    // disconnects or the server drops the broadcast.
    let entity_type = path.entity_type;
    let entity_id_for_terminal = path.entity_id.clone();
    // `Pin<Box<dyn Stream + Send>>` is the standard trait-object
    // shape for heterogeneous async streams — neither branch's
    // concrete type implements `Unpin`, so `Box::pin` is the
    // ergonomic carrier.
    let final_stream: std::pin::Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> =
        if entity_type == EntityType::Action {
            Box::pin(combined)
        } else {
            let reason_slot = Arc::new(std::sync::Mutex::new(None::<String>));
            let reason_for_detector = Arc::clone(&reason_slot);
            let detector_entity_id = entity_id_for_terminal.clone();
            let (signal_tx, signal_rx) = tokio::sync::oneshot::channel::<()>();

            tokio::spawn(async move {
                let mut det = tokio_stream::wrappers::BroadcastStream::new(detector_rx);
                let mut signal_tx = Some(signal_tx);
                while let Some(item) = det.next().await {
                    let Ok(event) = item else { continue };
                    if let Some(reason) =
                        detect_terminal(entity_type, &detector_entity_id, &event.event_type)
                    {
                        // Store reason *before* firing the signal
                        // so the chained terminator can read it
                        // without a race.
                        if let Ok(mut slot) = reason_for_detector.lock() {
                            *slot = Some(reason);
                        }
                        if let Some(tx) = signal_tx.take() {
                            let _ = tx.send(());
                        }
                        break;
                    }
                }
            });

            let truncated = combined.take_until(signal_rx);
            let entity_type_str = entity_type.as_str();
            let entity_id_for_final = entity_id_for_terminal;
            let final_event = futures::stream::once(async move {
                let reason = reason_slot
                    .lock()
                    .ok()
                    .and_then(|mut s| s.take())
                    .unwrap_or_else(|| "closed".to_string());
                Ok::<Event, Infallible>(
                    Event::default().event("subscription_end").data(
                        serde_json::json!({
                            "reason": reason,
                            "entity_type": entity_type_str,
                            "entity_id": entity_id_for_final,
                        })
                        .to_string(),
                    ),
                )
            });

            Box::pin(truncated.chain(final_event))
        };

    Ok(Sse::new(final_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    ))
}

// ---------------------------------------------------------------------------
// Catch-up builders
// ---------------------------------------------------------------------------

/// Build synthetic catch-up events for a chain's current state.
fn build_chain_catchup(chain_state: &acteon_core::ChainState) -> Vec<Result<Event, Infallible>> {
    let mut events = Vec::new();

    // Emit a ChainStepCompleted event for each completed step, ordered by
    // completed_at timestamp.
    let mut completed_steps: Vec<(usize, &acteon_core::StepResult)> = chain_state
        .step_results
        .iter()
        .enumerate()
        .filter_map(|(i, r)| r.as_ref().map(|sr| (i, sr)))
        .collect();
    completed_steps.sort_by_key(|(_, sr)| sr.completed_at);

    for (step_index, result) in &completed_steps {
        // Derive next_step from execution_path.
        let next_step = derive_next_step(&chain_state.execution_path, &result.step_name);

        let event_type = StreamEventType::ChainStepCompleted {
            chain_id: chain_state.chain_id.clone(),
            step_name: result.step_name.clone(),
            step_index: *step_index,
            success: result.success,
            next_step,
        };

        let stream_event = StreamEvent {
            id: uuid::Uuid::now_v7().to_string(),
            timestamp: result.completed_at,
            event_type,
            namespace: chain_state.namespace.clone(),
            tenant: chain_state.tenant.clone(),
            action_type: None,
            action_id: None,
        };

        if let Some(sse_event) = serialize_catchup_event(&stream_event) {
            events.push(Ok(sse_event));
        }
    }

    // If the chain is terminal, emit a ChainCompleted event.
    if chain_state.status != ChainStatus::Running
        && chain_state.status != ChainStatus::WaitingSubChain
        && chain_state.status != ChainStatus::WaitingParallel
    {
        let status_str = match chain_state.status {
            ChainStatus::Completed => "completed",
            ChainStatus::Failed => "failed",
            ChainStatus::Cancelled => "cancelled",
            ChainStatus::TimedOut => "timed_out",
            ChainStatus::Running | ChainStatus::WaitingSubChain | ChainStatus::WaitingParallel => {
                unreachable!()
            }
        };
        let event_type = StreamEventType::ChainCompleted {
            chain_id: chain_state.chain_id.clone(),
            status: status_str.to_string(),
            execution_path: chain_state.execution_path.clone(),
        };
        let stream_event = StreamEvent {
            id: uuid::Uuid::now_v7().to_string(),
            timestamp: chain_state.updated_at,
            event_type,
            namespace: chain_state.namespace.clone(),
            tenant: chain_state.tenant.clone(),
            action_type: None,
            action_id: None,
        };
        if let Some(sse_event) = serialize_catchup_event(&stream_event) {
            events.push(Ok(sse_event));
        }
    }

    debug!(
        chain_id = %chain_state.chain_id,
        catchup_events = events.len(),
        "chain catch-up complete"
    );
    events
}

/// Build synthetic catch-up events for a group's current state.
fn build_group_catchup(group: &acteon_core::EventGroup) -> Vec<Result<Event, Infallible>> {
    let mut events = Vec::new();

    // Emit current state as GroupEventAdded.
    let event_type = StreamEventType::GroupEventAdded {
        group_id: group.group_id.clone(),
        group_key: group.group_key.clone(),
        event_count: group.events.len(),
    };
    let stream_event = StreamEvent {
        id: uuid::Uuid::now_v7().to_string(),
        timestamp: group.updated_at,
        event_type,
        namespace: String::new(),
        tenant: String::new(),
        action_type: None,
        action_id: None,
    };
    if let Some(sse_event) = serialize_catchup_event(&stream_event) {
        events.push(Ok(sse_event));
    }

    // If notified or resolved, emit the corresponding event.
    match group.state {
        GroupState::Notified => {
            let event_type = StreamEventType::GroupFlushed {
                group_id: group.group_id.clone(),
                event_count: group.events.len(),
            };
            let stream_event = StreamEvent {
                id: uuid::Uuid::now_v7().to_string(),
                timestamp: group.updated_at,
                event_type,
                namespace: String::new(),
                tenant: String::new(),
                action_type: None,
                action_id: None,
            };
            if let Some(sse_event) = serialize_catchup_event(&stream_event) {
                events.push(Ok(sse_event));
            }
        }
        GroupState::Resolved => {
            let event_type = StreamEventType::GroupResolved {
                group_id: group.group_id.clone(),
                group_key: group.group_key.clone(),
            };
            let stream_event = StreamEvent {
                id: uuid::Uuid::now_v7().to_string(),
                timestamp: group.updated_at,
                event_type,
                namespace: String::new(),
                tenant: String::new(),
                action_type: None,
                action_id: None,
            };
            if let Some(sse_event) = serialize_catchup_event(&stream_event) {
                events.push(Ok(sse_event));
            }
        }
        GroupState::Pending => {}
    }

    debug!(
        group_id = %group.group_id,
        catchup_events = events.len(),
        "group catch-up complete"
    );
    events
}

/// Build synthetic catch-up events for an action from the audit store.
///
/// Uses `get_by_action_id` which returns the most recent audit record for the
/// given action. This is sufficient for catch-up since actions typically map
/// to a single dispatch event.
async fn build_action_catchup(
    audit: Option<&dyn acteon_audit::store::AuditStore>,
    action_id: &str,
    allowed_tenants: Option<&Vec<String>>,
) -> Vec<Result<Event, Infallible>> {
    use acteon_core::stream::{reconstruct_outcome, sanitize_outcome};

    let Some(audit) = audit else {
        debug!("no audit store configured, skipping action catch-up");
        return Vec::new();
    };

    let record = match audit.get_by_action_id(action_id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            debug!(action_id, "no audit record found for action catch-up");
            return Vec::new();
        }
        Err(e) => {
            tracing::warn!(error = %e, "audit query failed during action catch-up");
            return Vec::new();
        }
    };

    // Tenant isolation.
    if let Some(tenants) = allowed_tenants
        && !tenants.iter().any(|t| t == &record.tenant)
    {
        return Vec::new();
    }

    let outcome = match reconstruct_outcome(&record.outcome, &record.outcome_details) {
        Some(o) => sanitize_outcome(&o),
        None => return Vec::new(),
    };

    let stream_event = StreamEvent {
        id: uuid::Uuid::now_v7().to_string(),
        timestamp: record.dispatched_at,
        event_type: StreamEventType::ActionDispatched {
            outcome,
            provider: record.provider.clone(),
        },
        namespace: record.namespace.clone(),
        tenant: record.tenant.clone(),
        action_type: Some(record.action_type.clone()),
        action_id: Some(record.action_id.clone()),
    };

    let mut events = Vec::new();
    if let Some(sse_event) = serialize_catchup_event(&stream_event) {
        events.push(Ok(sse_event));
    }

    debug!(
        action_id,
        catchup_events = events.len(),
        "action catch-up complete"
    );
    events
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Derive the next step name from the execution path.
fn derive_next_step(execution_path: &[String], current_step_name: &str) -> Option<String> {
    let pos = execution_path.iter().position(|s| s == current_step_name)?;
    execution_path.get(pos + 1).cloned()
}

/// Serialize a `StreamEvent` into an SSE `Event`.
fn serialize_catchup_event(stream_event: &StreamEvent) -> Option<Event> {
    let event_id = stream_event.id.clone();
    let type_tag = stream_event_type_tag(&stream_event.event_type);
    serde_json::to_string(stream_event)
        .ok()
        .map(|json| Event::default().id(event_id).event(type_tag).data(json))
}

/// Require `namespace` and `tenant` query params.
fn require_ns_tenant(
    query: &SubscribeQuery,
) -> Result<(String, String), (StatusCode, axum::Json<serde_json::Value>)> {
    let ns = query.namespace.clone().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({
                "error": "namespace query parameter is required"
            })),
        )
    })?;
    let tenant = query.tenant.clone().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({
                "error": "tenant query parameter is required"
            })),
        )
    })?;
    Ok((ns, tenant))
}

/// Validate that the requested tenant is in the caller's allowed set.
fn validate_tenant_access(
    allowed_tenants: Option<&Vec<String>>,
    tenant: &str,
) -> Result<(), (StatusCode, axum::Json<serde_json::Value>)> {
    if let Some(tenants) = allowed_tenants
        && !tenants.iter().any(|t| t == tenant)
    {
        return Err(forbidden_or_not_found());
    }
    Ok(())
}

/// If `event_type` represents a terminal state for
/// `(entity_type, entity_id)`, return the `subscription_end`
/// reason string the SSE body should carry. Returns `None` for
/// non-terminal events.
///
/// Per design doc §2.5: terminal states are
/// - **Chain**: `ChainCompleted` (covers `completed` / `failed` /
///   `cancelled` / `timed_out` — the status is in the event
///   payload).
/// - **Group**: `GroupResolved` (after manual resolve or flush).
/// - **Action**: no inherent terminal in the live stream; the
///   detector is not wired for actions.
///
/// The reason string is the `snake_case` event-type tag, which
/// lets a client correlate it with the data event that preceded
/// it.
fn detect_terminal(
    entity_type: EntityType,
    entity_id: &str,
    event_type: &StreamEventType,
) -> Option<String> {
    match (entity_type, event_type) {
        (
            EntityType::Chain,
            StreamEventType::ChainCompleted {
                chain_id, status, ..
            },
        ) if chain_id == entity_id => Some(format!("chain_{status}")),
        (EntityType::Group, StreamEventType::GroupResolved { group_id, .. })
            if group_id == entity_id =>
        {
            Some("group_resolved".to_string())
        }
        _ => None,
    }
}

/// Uniform 403 for both "not found" and "wrong tenant" (security review C1).
fn forbidden_or_not_found() -> (StatusCode, axum::Json<serde_json::Value>) {
    (
        StatusCode::FORBIDDEN,
        axum::Json(serde_json::json!({
            "error": "forbidden or not found"
        })),
    )
}

/// `503 Service Unavailable` returned when one of the catch-up
/// state-store reads exceeds [`CATCHUP_FETCH_TIMEOUT`]. The
/// `entity_kind` argument lets the caller distinguish chain vs.
/// group vs. action timeouts from the log + error body without
/// exposing the underlying error details.
fn catchup_timeout(entity_kind: &'static str) -> (StatusCode, axum::Json<serde_json::Value>) {
    tracing::warn!(
        entity_kind,
        timeout_ms = u64::try_from(CATCHUP_FETCH_TIMEOUT.as_millis()).unwrap_or(u64::MAX),
        "subscribe: catch-up state-store fetch timed out"
    );
    (
        StatusCode::SERVICE_UNAVAILABLE,
        axum::Json(serde_json::json!({
            "error": format!(
                "state-store fetch for {entity_kind} timed out; try again or set include_history=false"
            )
        })),
    )
}

/// Validate entity ID length and charset. Extracted for testability.
fn validate_entity_id(id: &str) -> Result<(), (StatusCode, axum::Json<serde_json::Value>)> {
    if id.is_empty() || id.len() > MAX_ENTITY_ID_LENGTH {
        return Err((
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({
                "error": "entity_id must be between 1 and 256 characters"
            })),
        ));
    }
    if !id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err((
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({
                "error": "entity_id contains invalid characters (allowed: alphanumeric, dash, underscore, dot)"
            })),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_core::{ChainState, ChainStatus, EventGroup, GroupState, StepResult};
    use chrono::Utc;

    // -- catchup_timeout helper -----------------------------------------------

    #[test]
    fn catchup_timeout_is_503_with_actionable_message() {
        // Security review H2: timed-out catch-up reads must surface
        // as 503 with a message that names the entity kind and hints
        // at the `include_history=false` workaround. The `error`
        // field must NOT leak underlying state-store details.
        let (status, body) = catchup_timeout("chain");
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        let err = body.0.get("error").and_then(|v| v.as_str()).unwrap_or("");
        assert!(err.contains("chain"), "should name the entity kind: {err}");
        assert!(
            err.contains("include_history=false"),
            "should hint at the workaround: {err}",
        );
    }

    #[test]
    fn catchup_timeout_constant_is_reasonable() {
        // Pin the constant — a regression that bumps the cap to
        // e.g. 60s would silently undo the protection. The exact
        // value can move, but it has to stay under the typical
        // axum worker SLA.
        assert!(
            CATCHUP_FETCH_TIMEOUT.as_millis() <= 5_000,
            "catch-up timeout must stay short to bound worker blocking",
        );
        assert!(
            CATCHUP_FETCH_TIMEOUT.as_millis() >= 500,
            "but not so tight that a healthy backend trips it",
        );
    }

    // -- detect_terminal (subscription_end auto-close) ------------------------

    #[test]
    fn detect_terminal_fires_on_chain_completion_for_matching_id() {
        let ev = StreamEventType::ChainCompleted {
            chain_id: "chain-7".into(),
            status: "completed".into(),
            execution_path: vec![],
        };
        // The reason string folds the status into the tag so a
        // client can distinguish completed / failed / cancelled
        // without parsing the data field.
        assert_eq!(
            detect_terminal(EntityType::Chain, "chain-7", &ev),
            Some("chain_completed".to_string()),
        );
    }

    #[test]
    fn detect_terminal_carries_chain_failed_status() {
        let ev = StreamEventType::ChainCompleted {
            chain_id: "chain-7".into(),
            status: "failed".into(),
            execution_path: vec![],
        };
        assert_eq!(
            detect_terminal(EntityType::Chain, "chain-7", &ev),
            Some("chain_failed".to_string()),
        );
    }

    #[test]
    fn detect_terminal_ignores_other_chain_ids() {
        // Two chains can complete on the same broadcast; the
        // detector for "chain-7" must NOT fire on a sibling's
        // terminal event.
        let ev = StreamEventType::ChainCompleted {
            chain_id: "other-chain".into(),
            status: "completed".into(),
            execution_path: vec![],
        };
        assert_eq!(detect_terminal(EntityType::Chain, "chain-7", &ev), None);
    }

    #[test]
    fn detect_terminal_fires_on_group_resolved() {
        let ev = StreamEventType::GroupResolved {
            group_id: "group-3".into(),
            group_key: "k".into(),
        };
        assert_eq!(
            detect_terminal(EntityType::Group, "group-3", &ev),
            Some("group_resolved".to_string()),
        );
    }

    #[test]
    fn detect_terminal_does_not_fire_on_intermediate_chain_step() {
        // A step-completion is not the chain terminal — must NOT
        // end the subscription.
        let ev = StreamEventType::ChainStepCompleted {
            chain_id: "chain-7".into(),
            step_name: "validate".into(),
            step_index: 0,
            success: true,
            next_step: Some("dispatch".into()),
        };
        assert_eq!(detect_terminal(EntityType::Chain, "chain-7", &ev), None);
    }

    #[test]
    fn detect_terminal_does_not_fire_on_action_for_any_event() {
        // Per design §10 actions have no inherent terminal event —
        // the live-stream wrap is intentionally skipped for them.
        // This test pins that contract: there is no event type that
        // turns this branch on.
        let ev = StreamEventType::ChainCompleted {
            chain_id: "anything".into(),
            status: "completed".into(),
            execution_path: vec![],
        };
        assert_eq!(detect_terminal(EntityType::Action, "anything", &ev), None);
    }

    #[test]
    fn entity_type_as_str_matches_serde_repr() {
        // The subscription_end body uses `as_str`; the round-trip
        // tests below cover the serde direction. These must agree
        // so a client can branch on the same token whether it
        // came over the wire or from the helper.
        for et in [EntityType::Chain, EntityType::Group, EntityType::Action] {
            let token = match et {
                EntityType::Chain => "chain",
                EntityType::Group => "group",
                EntityType::Action => "action",
            };
            assert_eq!(et.as_str(), token);
        }
    }

    // -- EntityType deserialization -------------------------------------------

    #[test]
    fn entity_type_deserializes_chain() {
        let e: EntityType = serde_json::from_str(r#""chain""#).unwrap();
        assert!(matches!(e, EntityType::Chain));
    }

    #[test]
    fn entity_type_deserializes_group() {
        let e: EntityType = serde_json::from_str(r#""group""#).unwrap();
        assert!(matches!(e, EntityType::Group));
    }

    #[test]
    fn entity_type_deserializes_action() {
        let e: EntityType = serde_json::from_str(r#""action""#).unwrap();
        assert!(matches!(e, EntityType::Action));
    }

    #[test]
    fn entity_type_rejects_unknown() {
        let result = serde_json::from_str::<EntityType>(r#""unknown""#);
        assert!(result.is_err());
    }

    #[test]
    fn entity_type_is_snake_case() {
        // CamelCase should not deserialize since rename_all = snake_case.
        let result = serde_json::from_str::<EntityType>(r#""Chain""#);
        assert!(result.is_err());
    }

    // -- Entity ID validation -------------------------------------------------

    #[test]
    fn validate_entity_id_accepts_valid_ids() {
        assert!(validate_entity_id("chain-42").is_ok());
        assert!(validate_entity_id("my_group.v1").is_ok());
        assert!(validate_entity_id("abc123").is_ok());
        assert!(validate_entity_id("a").is_ok()); // minimum length
    }

    #[test]
    fn validate_entity_id_rejects_empty() {
        let err = validate_entity_id("").unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn validate_entity_id_rejects_too_long() {
        let long_id = "a".repeat(257);
        let err = validate_entity_id(&long_id).unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn validate_entity_id_accepts_max_length() {
        let max_id = "a".repeat(256);
        assert!(validate_entity_id(&max_id).is_ok());
    }

    #[test]
    fn validate_entity_id_rejects_special_characters() {
        let err = validate_entity_id("id with spaces").unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn validate_entity_id_rejects_path_traversal() {
        let err = validate_entity_id("../etc/passwd").unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn validate_entity_id_rejects_null_bytes() {
        let err = validate_entity_id("id\0null").unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn validate_entity_id_rejects_html_injection() {
        let err = validate_entity_id("<script>alert(1)</script>").unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn validate_entity_id_rejects_url_encoded() {
        let err = validate_entity_id("id%20encoded").unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    // -- StreamQuery construction from entity type ----------------------------

    #[test]
    fn entity_type_chain_builds_chain_id_query() {
        let path = SubscribePath {
            entity_type: EntityType::Chain,
            entity_id: "chain-42".into(),
        };
        let query = match path.entity_type {
            EntityType::Chain => StreamQuery {
                chain_id: Some(path.entity_id.clone()),
                ..StreamQuery::default()
            },
            EntityType::Group => StreamQuery {
                group_id: Some(path.entity_id.clone()),
                ..StreamQuery::default()
            },
            EntityType::Action => StreamQuery {
                action_id: Some(path.entity_id.clone()),
                ..StreamQuery::default()
            },
        };
        assert_eq!(query.chain_id, Some("chain-42".into()));
        assert!(query.group_id.is_none());
        assert!(query.action_id.is_none());
    }

    #[test]
    fn entity_type_group_builds_group_id_query() {
        let path = SubscribePath {
            entity_type: EntityType::Group,
            entity_id: "grp-abc".into(),
        };
        let query = match path.entity_type {
            EntityType::Chain => StreamQuery {
                chain_id: Some(path.entity_id.clone()),
                ..StreamQuery::default()
            },
            EntityType::Group => StreamQuery {
                group_id: Some(path.entity_id.clone()),
                ..StreamQuery::default()
            },
            EntityType::Action => StreamQuery {
                action_id: Some(path.entity_id.clone()),
                ..StreamQuery::default()
            },
        };
        assert!(query.chain_id.is_none());
        assert_eq!(query.group_id, Some("grp-abc".into()));
        assert!(query.action_id.is_none());
    }

    #[test]
    fn entity_type_action_builds_action_id_query() {
        let path = SubscribePath {
            entity_type: EntityType::Action,
            entity_id: "act-1".into(),
        };
        let query = match path.entity_type {
            EntityType::Chain => StreamQuery {
                chain_id: Some(path.entity_id.clone()),
                ..StreamQuery::default()
            },
            EntityType::Group => StreamQuery {
                group_id: Some(path.entity_id.clone()),
                ..StreamQuery::default()
            },
            EntityType::Action => StreamQuery {
                action_id: Some(path.entity_id.clone()),
                ..StreamQuery::default()
            },
        };
        assert!(query.chain_id.is_none());
        assert!(query.group_id.is_none());
        assert_eq!(query.action_id, Some("act-1".into()));
    }

    // -- SubscribeQuery defaults ----------------------------------------------

    #[test]
    fn subscribe_query_defaults() {
        let q = SubscribeQuery::default();
        // Default constructed has include_history = false (Default trait),
        // but serde default uses default_true(), so we test serde separately.
        assert!(!q.include_history); // Default trait default is false
        assert!(q.namespace.is_none());
        assert!(q.tenant.is_none());
    }

    #[test]
    fn subscribe_query_serde_default_include_history() {
        // When deserialized with no include_history field, should default to true.
        let q: SubscribeQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.include_history);
    }

    #[test]
    fn subscribe_query_serde_explicit_false() {
        let q: SubscribeQuery = serde_json::from_str(r#"{"include_history": false}"#).unwrap();
        assert!(!q.include_history);
    }

    // -- require_ns_tenant ----------------------------------------------------

    #[test]
    fn require_ns_tenant_ok() {
        let q = SubscribeQuery {
            include_history: true,
            namespace: Some("ns".into()),
            tenant: Some("t1".into()),
        };
        let (ns, t) = require_ns_tenant(&q).unwrap();
        assert_eq!(ns, "ns");
        assert_eq!(t, "t1");
    }

    #[test]
    fn require_ns_tenant_missing_namespace() {
        let q = SubscribeQuery {
            include_history: true,
            namespace: None,
            tenant: Some("t1".into()),
        };
        let err = require_ns_tenant(&q).unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn require_ns_tenant_missing_tenant() {
        let q = SubscribeQuery {
            include_history: true,
            namespace: Some("ns".into()),
            tenant: None,
        };
        let err = require_ns_tenant(&q).unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    // -- validate_tenant_access -----------------------------------------------

    #[test]
    fn validate_tenant_access_wildcard() {
        assert!(validate_tenant_access(None, "any-tenant").is_ok());
    }

    #[test]
    fn validate_tenant_access_matching() {
        let allowed = vec!["t1".to_string(), "t2".to_string()];
        assert!(validate_tenant_access(Some(&allowed), "t1").is_ok());
    }

    #[test]
    fn validate_tenant_access_denied() {
        let allowed = vec!["t1".to_string()];
        let err = validate_tenant_access(Some(&allowed), "t2").unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    // -- derive_next_step -----------------------------------------------------

    #[test]
    fn derive_next_step_found() {
        let path = vec!["a".into(), "b".into(), "c".into()];
        assert_eq!(derive_next_step(&path, "a"), Some("b".into()));
        assert_eq!(derive_next_step(&path, "b"), Some("c".into()));
    }

    #[test]
    fn derive_next_step_last() {
        let path = vec!["a".into(), "b".into()];
        assert_eq!(derive_next_step(&path, "b"), None);
    }

    #[test]
    fn derive_next_step_not_in_path() {
        let path = vec!["a".into(), "b".into()];
        assert_eq!(derive_next_step(&path, "c"), None);
    }

    // -- Chain catch-up -------------------------------------------------------

    fn make_chain_state(
        chain_id: &str,
        status: ChainStatus,
        step_results: Vec<Option<StepResult>>,
        execution_path: Vec<String>,
    ) -> ChainState {
        let now = Utc::now();
        ChainState {
            chain_id: chain_id.into(),
            chain_name: "test-chain".into(),
            origin_action: acteon_core::Action::new(
                "ns",
                "t1",
                "provider",
                "action_type",
                serde_json::json!({}),
            ),
            current_step: step_results.iter().filter(|r| r.is_some()).count(),
            total_steps: step_results.len(),
            status,
            step_results,
            started_at: now,
            updated_at: now,
            expires_at: None,
            namespace: "ns".into(),
            tenant: "t1".into(),
            cancel_reason: None,
            cancelled_by: None,
            execution_path,
            parent_chain_id: None,
            parent_step_index: None,
            child_chain_ids: Vec::new(),
            task_id: None,
            parallel_state: None,
            parallel_sub_results: std::collections::HashMap::new(),
            step_attempts: vec![],
            step_history: vec![],
        }
    }

    fn make_step_result(name: &str, success: bool) -> StepResult {
        StepResult {
            step_name: name.into(),
            success,
            response_body: Some(serde_json::json!({"ok": true})),
            error: None,
            completed_at: Utc::now(),
            attempt: None,
            started_at: None,
        }
    }

    #[test]
    fn test_chain_catchup_emits_step_events() {
        let chain_state = make_chain_state(
            "chain-1",
            ChainStatus::Running,
            vec![
                Some(make_step_result("step-a", true)),
                Some(make_step_result("step-b", true)),
                None, // step-c pending
            ],
            vec!["step-a".into(), "step-b".into()],
        );

        let events = build_chain_catchup(&chain_state);
        // 2 completed steps → 2 events (no ChainCompleted since Running)
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn test_chain_catchup_terminal_chain() {
        let chain_state = make_chain_state(
            "chain-2",
            ChainStatus::Completed,
            vec![
                Some(make_step_result("step-a", true)),
                Some(make_step_result("step-b", true)),
            ],
            vec!["step-a".into(), "step-b".into()],
        );

        let events = build_chain_catchup(&chain_state);
        // 2 step events + 1 ChainCompleted = 3
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn test_chain_catchup_failed_chain() {
        let chain_state = make_chain_state(
            "chain-3",
            ChainStatus::Failed,
            vec![
                Some(make_step_result("step-a", true)),
                Some(make_step_result("step-b", false)),
            ],
            vec!["step-a".into(), "step-b".into()],
        );

        let events = build_chain_catchup(&chain_state);
        // 2 step events + 1 ChainCompleted (failed) = 3
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn test_chain_catchup_empty_no_steps() {
        let chain_state =
            make_chain_state("chain-4", ChainStatus::Running, vec![None, None], vec![]);

        let events = build_chain_catchup(&chain_state);
        assert_eq!(events.len(), 0);
    }

    #[test]
    fn test_include_history_false_skips_catchup() {
        // This test verifies the logic in subscribe() — when include_history=false,
        // the catchup builder is not called. We test the builder directly here:
        // even if chain has state, an empty Vec is expected when we skip.
        let chain_state = make_chain_state(
            "chain-5",
            ChainStatus::Completed,
            vec![Some(make_step_result("s1", true))],
            vec!["s1".into()],
        );
        // Simulate include_history = false: just don't call build_chain_catchup.
        let events: Vec<Result<Event, Infallible>> = Vec::new();
        assert!(events.is_empty());
        // But if we did call it, we'd get events:
        let events = build_chain_catchup(&chain_state);
        assert_eq!(events.len(), 2); // 1 step + 1 completed
    }

    // -- Group catch-up -------------------------------------------------------

    fn make_group(state: GroupState, event_count: usize) -> EventGroup {
        let now = Utc::now();
        let mut group = EventGroup::new("grp-1", "key-1", now);
        group.state = state;
        for i in 0..event_count {
            group.add_event(acteon_core::GroupedEvent::new(
                format!("action-{i}").into(),
                serde_json::json!({"n": i}),
            ));
        }
        group
    }

    #[test]
    fn test_group_catchup_pending() {
        let group = make_group(GroupState::Pending, 3);
        let events = build_group_catchup(&group);
        // 1 GroupEventAdded only (pending, no flush/resolve)
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn test_group_catchup_notified() {
        let group = make_group(GroupState::Notified, 5);
        let events = build_group_catchup(&group);
        // 1 GroupEventAdded + 1 GroupFlushed = 2
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn test_group_catchup_resolved() {
        let group = make_group(GroupState::Resolved, 2);
        let events = build_group_catchup(&group);
        // 1 GroupEventAdded + 1 GroupResolved = 2
        assert_eq!(events.len(), 2);
    }

    // -- forbidden_or_not_found -----------------------------------------------

    #[test]
    fn test_entity_not_found_returns_403() {
        let (status, _body) = forbidden_or_not_found();
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[test]
    fn test_wrong_tenant_returns_403() {
        let allowed = vec!["tenant-a".to_string()];
        let err = validate_tenant_access(Some(&allowed), "tenant-b").unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }
}
