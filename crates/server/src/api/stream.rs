//! SSE event streaming endpoint.
//!
//! Provides a `GET /v1/stream` endpoint that lets authenticated clients
//! subscribe to real-time action outcomes via Server-Sent Events (SSE).
//!
//! ## Security controls
//!
//! - **Authentication**: requires valid Bearer token or API key (via `AuthLayer`)
//! - **Tenant isolation**: events are filtered server-side based on caller grants
//! - **Connection limits**: per-tenant concurrent SSE connection cap (default: 10)
//! - **Backpressure**: slow clients that fall behind receive a lagged warning
//!   and the stream continues from the latest event

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::Stream;
use serde::Deserialize;
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use tracing::{debug, warn};

use acteon_core::stream::outcome_category;
use acteon_core::{StreamEvent, StreamEventType};

use crate::auth::identity::CallerIdentity;
use crate::auth::role::Permission;

use super::AppState;

/// Global registry tracking active SSE connections per tenant.
///
/// Uses a simple `HashMap<String, AtomicUsize>` behind a `tokio::sync::RwLock`.
/// The `AtomicUsize` approach avoids holding the write lock during the
/// entire SSE session -- we only take a write lock to insert a new tenant entry.
pub struct ConnectionRegistry {
    connections: tokio::sync::RwLock<HashMap<String, Arc<AtomicUsize>>>,
    max_per_tenant: usize,
}

impl ConnectionRegistry {
    /// Create a new connection registry with the given per-tenant limit.
    pub fn new(max_per_tenant: usize) -> Self {
        Self {
            connections: tokio::sync::RwLock::new(HashMap::new()),
            max_per_tenant,
        }
    }

    /// Try to acquire a connection slot for the given tenant.
    /// Returns `Some(guard)` on success, `None` if the limit is reached.
    pub async fn try_acquire(&self, tenant: &str) -> Option<ConnectionGuard> {
        // Fast path: read lock to check existing counter.
        {
            let conns = self.connections.read().await;
            if let Some(counter) = conns.get(tenant) {
                let current = counter.load(Ordering::Relaxed);
                if current >= self.max_per_tenant {
                    return None;
                }
                counter.fetch_add(1, Ordering::Relaxed);
                return Some(ConnectionGuard {
                    counter: Arc::clone(counter),
                });
            }
        }
        // Slow path: write lock to insert new tenant entry.
        let mut conns = self.connections.write().await;
        let counter = conns
            .entry(tenant.to_owned())
            .or_insert_with(|| Arc::new(AtomicUsize::new(0)));
        let current = counter.load(Ordering::Relaxed);
        if current >= self.max_per_tenant {
            return None;
        }
        counter.fetch_add(1, Ordering::Relaxed);
        Some(ConnectionGuard {
            counter: Arc::clone(counter),
        })
    }
}

/// RAII guard that decrements the connection counter on drop.
pub struct ConnectionGuard {
    counter: Arc<AtomicUsize>,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Query parameters for the SSE stream endpoint.
#[derive(Debug, Deserialize, Default)]
pub struct StreamQuery {
    /// Filter events by namespace.
    pub namespace: Option<String>,
    /// Filter events by action type.
    pub action_type: Option<String>,
    /// Filter events by outcome category (e.g., `executed`, `suppressed`, `failed`).
    pub outcome: Option<String>,
    /// Filter events by stream event type (e.g., `action_dispatched`, `group_flushed`).
    pub event_type: Option<String>,
}

/// `GET /v1/stream` -- subscribe to real-time action outcomes via SSE.
///
/// The stream emits events as they flow through the gateway pipeline.
/// Events are filtered server-side based on the caller's tenant grants
/// and optional query-parameter filters.
///
/// Supports `Last-Event-ID` header for reconnection (note: events between
/// disconnect and reconnect are lost since broadcast channels do not persist).
#[allow(clippy::unused_async)]
pub async fn stream(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Query(query): Query<StreamQuery>,
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

    // 2. Determine the caller's allowed tenants for filtering.
    let allowed_tenants: Option<Vec<String>> = identity
        .allowed_tenants()
        .map(|tenants| tenants.into_iter().map(String::from).collect());

    // 3. Enforce per-tenant connection limit.
    //    For wildcard callers, use the caller ID as the bucket.
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

    // 4. Subscribe to the broadcast channel.
    let gateway = state.gateway.read().await;
    let rx = gateway.stream_tx().subscribe();
    drop(gateway); // Release the read lock immediately.

    // 5. Build the filtered SSE stream.
    let event_stream = make_event_stream(rx, allowed_tenants, query, guard);

    Ok(Sse::new(event_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    ))
}

/// Build a filtered SSE event stream from the broadcast receiver.
///
/// The `conn_guard` is moved into the stream future so it is dropped when
/// the client disconnects, releasing the connection slot.
fn make_event_stream(
    rx: broadcast::Receiver<StreamEvent>,
    allowed_tenants: Option<Vec<String>>,
    query: StreamQuery,
    conn_guard: ConnectionGuard,
) -> impl Stream<Item = Result<Event, Infallible>> {
    let broadcast_stream = BroadcastStream::new(rx);

    // Move conn_guard into the closure so it is dropped when the stream ends
    // (i.e., when the client disconnects). This releases the connection slot.
    broadcast_stream.filter_map(move |result| {
        let _ = &conn_guard;
        match result {
            Ok(event) => {
                // Tenant isolation: skip events the caller is not authorized to see.
                if let Some(ref tenants) = allowed_tenants
                    && !tenants.iter().any(|t| t == &event.tenant)
                {
                    return None;
                }

                // Apply optional query filters.
                if let Some(ref ns) = query.namespace
                    && &event.namespace != ns
                {
                    return None;
                }
                if let Some(ref at) = query.action_type
                    && event.action_type.as_deref() != Some(at.as_str())
                {
                    return None;
                }
                if let Some(ref et) = query.event_type {
                    let type_tag = stream_event_type_tag(&event.event_type);
                    if type_tag != et {
                        return None;
                    }
                }
                if let Some(ref oc) = query.outcome {
                    if let StreamEventType::ActionDispatched { ref outcome, .. } = event.event_type
                    {
                        if outcome_category(outcome) != oc.as_str() {
                            return None;
                        }
                    } else {
                        // Non-dispatch events don't have an outcome category.
                        return None;
                    }
                }

                // Serialize and emit.
                let event_id = event.id.clone();
                let type_tag = stream_event_type_tag(&event.event_type);
                match serde_json::to_string(&event) {
                    Ok(json) => Some(Ok(Event::default().id(event_id).event(type_tag).data(json))),
                    Err(e) => {
                        warn!(error = %e, "failed to serialize stream event");
                        None
                    }
                }
            }
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                debug!(skipped = n, "SSE client lagged, skipping events");
                // Emit a warning event so the client knows events were dropped.
                Some(Ok(Event::default()
                    .event("lagged")
                    .data(format!("{{\"skipped\":{n}}}"))))
            }
        }
    })
}

/// Return the SSE event type tag for a [`StreamEventType`].
fn stream_event_type_tag(event_type: &StreamEventType) -> &'static str {
    match event_type {
        StreamEventType::ActionDispatched { .. } => "action_dispatched",
        StreamEventType::GroupFlushed { .. } => "group_flushed",
        StreamEventType::Timeout { .. } => "timeout",
        StreamEventType::ChainAdvanced { .. } => "chain_advanced",
        StreamEventType::ApprovalRequired { .. } => "approval_required",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_core::{ActionOutcome, ProviderResponse};
    use chrono::Utc;

    #[tokio::test]
    async fn connection_registry_acquire_and_release() {
        let registry = ConnectionRegistry::new(2);
        let guard1 = registry.try_acquire("tenant-1").await;
        assert!(guard1.is_some(), "first acquire should succeed");
        let guard2 = registry.try_acquire("tenant-1").await;
        assert!(guard2.is_some(), "second acquire should succeed (limit=2)");
        let guard3 = registry.try_acquire("tenant-1").await;
        assert!(guard3.is_none(), "third acquire should fail (limit=2)");
        drop(guard1);
        let guard4 = registry.try_acquire("tenant-1").await;
        assert!(guard4.is_some(), "acquire after release should succeed");
    }

    #[tokio::test]
    async fn connection_registry_separate_tenants() {
        let registry = ConnectionRegistry::new(1);
        let _g1 = registry.try_acquire("tenant-a").await;
        assert!(_g1.is_some());
        let _g2 = registry.try_acquire("tenant-b").await;
        assert!(_g2.is_some());
        let g3 = registry.try_acquire("tenant-a").await;
        assert!(g3.is_none());
    }

    #[tokio::test]
    async fn connection_guard_decrements_on_drop() {
        let registry = ConnectionRegistry::new(1);
        {
            let _guard = registry.try_acquire("t").await.unwrap();
            assert!(registry.try_acquire("t").await.is_none(), "at limit");
        }
        assert!(
            registry.try_acquire("t").await.is_some(),
            "after guard drop"
        );
    }

    #[test]
    fn type_tag_for_all_variants() {
        assert_eq!(
            stream_event_type_tag(&StreamEventType::ActionDispatched {
                outcome: ActionOutcome::Deduplicated,
                provider: "p".into(),
            }),
            "action_dispatched"
        );
        assert_eq!(
            stream_event_type_tag(&StreamEventType::GroupFlushed {
                group_id: "g".into(),
                event_count: 0,
            }),
            "group_flushed"
        );
        assert_eq!(
            stream_event_type_tag(&StreamEventType::Timeout {
                fingerprint: "f".into(),
                state_machine: "s".into(),
                previous_state: "a".into(),
                new_state: "b".into(),
            }),
            "timeout"
        );
        assert_eq!(
            stream_event_type_tag(&StreamEventType::ChainAdvanced {
                chain_id: "c".into(),
            }),
            "chain_advanced"
        );
        assert_eq!(
            stream_event_type_tag(&StreamEventType::ApprovalRequired {
                approval_id: "a".into(),
            }),
            "approval_required"
        );
    }

    fn mk_dispatched(ns: &str, tenant: &str, at: &str, outcome: ActionOutcome) -> StreamEvent {
        StreamEvent {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            event_type: StreamEventType::ActionDispatched {
                outcome,
                provider: "email".into(),
            },
            namespace: ns.into(),
            tenant: tenant.into(),
            action_type: Some(at.into()),
            action_id: Some("a".into()),
        }
    }

    fn mk_bg(ns: &str, tenant: &str, et: StreamEventType) -> StreamEvent {
        StreamEvent {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            event_type: et,
            namespace: ns.into(),
            tenant: tenant.into(),
            action_type: None,
            action_id: None,
        }
    }

    async fn collect(
        events: Vec<StreamEvent>,
        tenants: Option<Vec<String>>,
        q: StreamQuery,
    ) -> usize {
        let (tx, rx) = broadcast::channel(128);
        let guard = ConnectionGuard {
            counter: Arc::new(AtomicUsize::new(1)),
        };
        let s = make_event_stream(rx, tenants, q, guard);
        let mut s = Box::pin(s);
        for e in events {
            let _ = tx.send(e);
        }
        drop(tx);
        let mut n = 0;
        while let Some(Ok(_)) = s.next().await {
            n += 1;
        }
        n
    }

    #[tokio::test]
    async fn filter_by_namespace() {
        let evts = vec![
            mk_dispatched("ns-a", "t1", "s", ActionOutcome::Deduplicated),
            mk_dispatched("ns-b", "t1", "s", ActionOutcome::Deduplicated),
            mk_dispatched("ns-a", "t1", "s", ActionOutcome::Deduplicated),
        ];
        assert_eq!(
            collect(
                evts,
                None,
                StreamQuery {
                    namespace: Some("ns-a".into()),
                    ..Default::default()
                }
            )
            .await,
            2
        );
    }

    #[tokio::test]
    async fn filter_by_action_type() {
        let evts = vec![
            mk_dispatched("ns", "t1", "send_email", ActionOutcome::Deduplicated),
            mk_dispatched("ns", "t1", "send_sms", ActionOutcome::Deduplicated),
            mk_dispatched("ns", "t1", "send_email", ActionOutcome::Deduplicated),
        ];
        assert_eq!(
            collect(
                evts,
                None,
                StreamQuery {
                    action_type: Some("send_email".into()),
                    ..Default::default()
                }
            )
            .await,
            2
        );
    }

    #[tokio::test]
    async fn filter_by_outcome() {
        let evts = vec![
            mk_dispatched(
                "ns",
                "t1",
                "s",
                ActionOutcome::Executed(ProviderResponse::success(serde_json::Value::Null)),
            ),
            mk_dispatched(
                "ns",
                "t1",
                "s",
                ActionOutcome::Suppressed { rule: "r".into() },
            ),
            mk_dispatched(
                "ns",
                "t1",
                "s",
                ActionOutcome::Executed(ProviderResponse::success(serde_json::Value::Null)),
            ),
        ];
        assert_eq!(
            collect(
                evts,
                None,
                StreamQuery {
                    outcome: Some("executed".into()),
                    ..Default::default()
                }
            )
            .await,
            2
        );
    }

    #[tokio::test]
    async fn filter_by_event_type() {
        let evts = vec![
            mk_dispatched("ns", "t1", "s", ActionOutcome::Deduplicated),
            mk_bg(
                "ns",
                "t1",
                StreamEventType::GroupFlushed {
                    group_id: "g".into(),
                    event_count: 3,
                },
            ),
            mk_dispatched("ns", "t1", "s", ActionOutcome::Deduplicated),
        ];
        assert_eq!(
            collect(
                evts,
                None,
                StreamQuery {
                    event_type: Some("group_flushed".into()),
                    ..Default::default()
                }
            )
            .await,
            1
        );
    }

    #[tokio::test]
    async fn tenant_isolation() {
        let evts = vec![
            mk_dispatched("ns", "tenant-a", "s", ActionOutcome::Deduplicated),
            mk_dispatched("ns", "tenant-b", "s", ActionOutcome::Deduplicated),
            mk_dispatched("ns", "tenant-a", "s", ActionOutcome::Deduplicated),
        ];
        assert_eq!(
            collect(evts, Some(vec!["tenant-a".into()]), StreamQuery::default()).await,
            2
        );
    }

    #[tokio::test]
    async fn wildcard_tenant_sees_all() {
        let evts = vec![
            mk_dispatched("ns", "a", "s", ActionOutcome::Deduplicated),
            mk_dispatched("ns", "b", "s", ActionOutcome::Deduplicated),
            mk_dispatched("ns", "c", "s", ActionOutcome::Deduplicated),
        ];
        assert_eq!(collect(evts, None, StreamQuery::default()).await, 3);
    }

    #[tokio::test]
    async fn no_filters_passes_all() {
        let evts = vec![
            mk_dispatched("n1", "t1", "a", ActionOutcome::Deduplicated),
            mk_dispatched("n2", "t2", "b", ActionOutcome::Deduplicated),
            mk_bg(
                "n3",
                "t3",
                StreamEventType::ChainAdvanced {
                    chain_id: "c".into(),
                },
            ),
        ];
        assert_eq!(collect(evts, None, StreamQuery::default()).await, 3);
    }

    #[tokio::test]
    async fn combined_filters() {
        let evts = vec![
            mk_dispatched(
                "alerts",
                "t1",
                "send_email",
                ActionOutcome::Executed(ProviderResponse::success(serde_json::Value::Null)),
            ),
            mk_dispatched(
                "alerts",
                "t1",
                "send_sms",
                ActionOutcome::Executed(ProviderResponse::success(serde_json::Value::Null)),
            ),
            mk_dispatched(
                "notif",
                "t1",
                "send_email",
                ActionOutcome::Executed(ProviderResponse::success(serde_json::Value::Null)),
            ),
            mk_dispatched(
                "alerts",
                "t1",
                "send_email",
                ActionOutcome::Suppressed { rule: "r".into() },
            ),
        ];
        assert_eq!(
            collect(
                evts,
                None,
                StreamQuery {
                    namespace: Some("alerts".into()),
                    action_type: Some("send_email".into()),
                    outcome: Some("executed".into()),
                    event_type: None,
                }
            )
            .await,
            1
        );
    }

    #[tokio::test]
    async fn outcome_filter_excludes_non_dispatch() {
        let evts = vec![
            mk_bg(
                "ns",
                "t1",
                StreamEventType::GroupFlushed {
                    group_id: "g".into(),
                    event_count: 1,
                },
            ),
            mk_dispatched(
                "ns",
                "t1",
                "s",
                ActionOutcome::Executed(ProviderResponse::success(serde_json::Value::Null)),
            ),
        ];
        assert_eq!(
            collect(
                evts,
                None,
                StreamQuery {
                    outcome: Some("executed".into()),
                    ..Default::default()
                }
            )
            .await,
            1
        );
    }
}
