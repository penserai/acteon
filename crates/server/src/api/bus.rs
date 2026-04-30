//! Phase 1 bus API: topics CRUD + publish + subscribe.
//!
//! All endpoints return `503 Service Unavailable` when the server was
//! compiled without the `bus` feature or when `[bus].enabled = false`
//! in the TOML config. On the feature-enabled path they interact with
//! an `acteon_bus::SharedBackend` held in [`super::AppState`]. The
//! reference is a plain code span rather than a rustdoc link because
//! `acteon_bus` is only in scope when built with `--features bus`, and
//! CI's `cargo doc` runs with default features.
//
// The handlers all use `Result<_, axum::response::Response>` for early-
// return error paths so each error path can shape its own status +
// body without a custom error enum and `IntoResponse` impl. The Err
// variant is large because `Response` carries a body buffer, but it's
// constructed only on errors and consumed immediately, so the size is
// not a real cost — silence the lint module-wide.
#![allow(clippy::result_large_err)]

use std::collections::{BTreeMap, HashMap};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[cfg(feature = "bus")]
use axum::response::sse::{Event, KeepAlive, Sse};
#[cfg(feature = "bus")]
use futures::StreamExt;
#[cfg(feature = "bus")]
use futures::stream::Stream;
#[cfg(feature = "bus")]
use std::convert::Infallible;
#[cfg(feature = "bus")]
use std::time::Duration;

use super::AppState;
use super::schemas::ErrorResponse;

#[cfg(feature = "bus")]
use crate::auth::identity::CallerIdentity;
#[cfg(feature = "bus")]
use crate::auth::role::Permission;
#[cfg(feature = "bus")]
use acteon_core::Topic;
#[cfg(feature = "bus")]
use acteon_state::{KeyKind, StateKey};

/// Authorization helper shared by the bus handlers.
///
/// Every bus endpoint that touches a specific `(namespace, tenant)` —
/// create/delete/publish/subscribe — flows through this. It checks the
/// caller has the base permission for the action class (dispatch-style
/// for producers, stream-subscribe for consumers) and that their grant
/// covers the tenant + namespace of the target topic, treating
/// `provider=bus` and `action_type=publish|subscribe|manage` as the
/// fourth/fifth dimensions of the grant match so operators can lock
/// down bus access independently from regular action dispatch.
#[cfg(feature = "bus")]
#[allow(clippy::result_large_err)]
// The `Err` variant is an `axum::response::Response` (the rejection
// we want the handler to return verbatim). Boxing it would force
// an extra alloc in the hot happy-path; the Result is only ever
// constructed in these handlers and never stored, so the large-err
// lint isn't a concern here.
fn authorize_bus_op(
    identity: &CallerIdentity,
    tenant: &str,
    namespace: &str,
    action: BusOp,
) -> Result<(), axum::response::Response> {
    let (permission, action_verb) = match action {
        BusOp::Manage => (Permission::Dispatch, "manage"),
        BusOp::Publish => (Permission::Dispatch, "publish"),
        BusOp::Subscribe => (Permission::StreamSubscribe, "subscribe"),
        BusOp::ManageSchema => (Permission::Dispatch, "schema"),
        BusOp::ManageAgent => (Permission::Dispatch, "agent"),
        BusOp::ManageConversation => (Permission::Dispatch, "conversation"),
    };
    if !identity.role.has_permission(permission) {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: format!(
                    "insufficient role for bus.{action_verb}: requires admin or operator"
                ),
            }),
        )
            .into_response());
    }
    if !identity.is_authorized(tenant, namespace, "bus", action_verb) {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: format!(
                    "forbidden: no grant covers tenant={tenant}, namespace={namespace}, provider=bus, action={action_verb}"
                ),
            }),
        )
            .into_response());
    }
    Ok(())
}

#[cfg(feature = "bus")]
#[derive(Clone, Copy)]
enum BusOp {
    /// Topic CRUD (create / delete).
    Manage,
    /// Produce to a topic.
    Publish,
    /// Subscribe (SSE) to a topic.
    Subscribe,
    /// Schema CRUD + topic-binding CRUD (Phase 3).
    ManageSchema,
    /// Agent CRUD + heartbeat + send-to-agent (Phase 4). Send also
    /// flows through [`BusOp::Publish`] on the underlying topic so
    /// operators can restrict *inbox writes* independently of *agent
    /// registry ops*.
    ManageAgent,
    /// Conversation CRUD + transitions + thread reads (Phase 5).
    /// Appending a message also flows through [`BusOp::Publish`] so
    /// operators can split read/write ACLs.
    ManageConversation,
}

/// Parse a `namespace.tenant.name` Kafka topic string.
///
/// Returns the `(namespace, tenant)` pair used for tenant scoping plus
/// the leaf name so callers can log a consistent identifier.
#[cfg(feature = "bus")]
fn parse_kafka_name(topic: &str) -> Result<(&str, &str, &str), String> {
    let parts: Vec<&str> = topic.splitn(3, '.').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.is_empty()) {
        return Err(format!(
            "invalid topic '{topic}' (expected namespace.tenant.name)"
        ));
    }
    Ok((parts[0], parts[1], parts[2]))
}

/// Caps on user-supplied headers across every publish-style handler.
/// Sized to fit comfortably within typical Kafka `message.max.bytes`
/// budgets while leaving room for the payload, so a misbehaving
/// caller can't blow up the producer's memory or wedge the broker
/// connection with oversized records.
#[cfg(feature = "bus")]
const MAX_USER_HEADER_COUNT: usize = 20;
#[cfg(feature = "bus")]
const MAX_USER_HEADER_KEY_BYTES: usize = 256;
#[cfg(feature = "bus")]
const MAX_USER_HEADER_VALUE_BYTES: usize = 4096;

/// Bound on how many times a CAS-update loop retries on conflict.
/// The expected per-record contention is low (operator + agent
/// concurrent edit on the same conversation), so 8 attempts is well
/// past the realistic worst case before declaring a 409.
#[cfg(feature = "bus")]
const MAX_CAS_RETRY_ATTEMPTS: u32 = 8;

/// How often the busy scan loops in `lookup_tool_result` and
/// `replay_conversation_messages` yield back to the tokio executor
/// while skipping non-matching records. `stream.next()` may return
/// `Poll::Ready` repeatedly when the consumer has buffered data, so
/// without an explicit yield a long lookup on a busy topic could
/// monopolize a single-threaded executor and starve other tasks.
#[cfg(feature = "bus")]
const SCAN_YIELD_EVERY: u32 = 256;

/// Read-modify-write a JSON-serialized state row atomically via
/// `compare_and_swap`. The mutator closure runs on a freshly-loaded
/// copy each iteration; if the underlying row changed mid-loop, the
/// CAS fails and we re-read and re-apply. Returns the mutated value
/// after a successful commit, or a typed `Response` for missing key,
/// mutator rejection, contention exhaustion, or a backend error.
///
/// Used by `update_*` and `transition_*` handlers to close the
/// load-then-set TOCTOU window the second adversarial review
/// flagged.
#[cfg(feature = "bus")]
#[allow(clippy::result_large_err)]
async fn cas_update<T, F>(
    state: &AppState,
    key: &StateKey,
    missing_msg: &str,
    mut mutate: F,
) -> Result<T, axum::response::Response>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
    F: FnMut(&mut T) -> Result<(), axum::response::Response>,
{
    // Clone the `Arc<dyn StateStore>` once outside the loop and drop
    // the gateway read guard before any `.await` on the state store.
    // Holding `gw` across DB roundtrips — and worse, across all 8
    // retry iterations — would block any pending gateway writer
    // (config reloader, etc.) and cascade into queue-head-blocking
    // for every other incoming request, since tokio's `RwLock` is
    // fair. This was the third-pass review's HIGH finding.
    let store: std::sync::Arc<dyn acteon_state::StateStore> = {
        let gw = state.gateway.read().await;
        gw.state_store().clone()
    };
    for _ in 0..MAX_CAS_RETRY_ATTEMPTS {
        let (raw, version) = match store.get_versioned(key).await {
            Ok(Some(p)) => p,
            Ok(None) => {
                return Err((
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse {
                        error: missing_msg.to_string(),
                    }),
                )
                    .into_response());
            }
            Err(e) => {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response());
            }
        };
        let mut current: T = serde_json::from_str(&raw).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("corrupt state record at {}: {e}", key.canonical()),
                }),
            )
                .into_response()
        })?;
        mutate(&mut current)?;
        let payload = serde_json::to_string(&current).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response()
        })?;
        match store.compare_and_swap(key, version, &payload, None).await {
            Ok(acteon_state::CasResult::Ok) => return Ok(current),
            // Lost the race; reload and reapply on the next loop iteration.
            Ok(acteon_state::CasResult::Conflict { .. }) => {}
            Err(e) => {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response());
            }
        }
    }
    Err((
        StatusCode::CONFLICT,
        Json(ErrorResponse {
            error: format!("state at {} is changing too fast; retry", key.canonical()),
        }),
    )
        .into_response())
}

/// Caps on operator-supplied `labels` maps across all create/update
/// DTOs. Labels are persisted in the state store and replayed via
/// `list_*` handlers — without bounds, a single registration could
/// bloat the row and OOM the gateway during a list scan.
#[cfg(feature = "bus")]
const MAX_LABELS_COUNT: usize = 32;
#[cfg(feature = "bus")]
const MAX_LABEL_KEY_BYTES: usize = 256;
#[cfg(feature = "bus")]
const MAX_LABEL_VALUE_BYTES: usize = 4096;

/// Validate a caller-supplied labels map. Same rationale as
/// [`validate_user_headers`] but applied to the persisted-state path
/// rather than the publish path.
#[cfg(feature = "bus")]
fn validate_user_labels(
    labels: &std::collections::HashMap<String, String>,
) -> Result<(), axum::response::Response> {
    if labels.len() > MAX_LABELS_COUNT {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!(
                    "too many labels: {}; max is {MAX_LABELS_COUNT}",
                    labels.len()
                ),
            }),
        )
            .into_response());
    }
    for (k, v) in labels {
        if k.len() > MAX_LABEL_KEY_BYTES {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!(
                        "label key length {} exceeds {MAX_LABEL_KEY_BYTES} bytes",
                        k.len()
                    ),
                }),
            )
                .into_response());
        }
        if v.len() > MAX_LABEL_VALUE_BYTES {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!(
                        "label '{k}' value length {} exceeds {MAX_LABEL_VALUE_BYTES} bytes",
                        v.len()
                    ),
                }),
            )
                .into_response());
        }
    }
    Ok(())
}

/// Encode a per-partition offset map as an opaque resume token.
/// URL-safe base64 of a JSON object so clients can pass it back
/// through query strings without manual escaping. Cheap to decode and
/// debug-readable when base64-decoded.
#[cfg(feature = "bus")]
fn encode_replay_cursor(offsets: &std::collections::BTreeMap<i32, i64>) -> String {
    use base64::Engine;
    // Render as `{"P": O, "P": O}`; serde_json keeps key order via
    // the BTreeMap, so the cursor is stable for a given offset map.
    let json = serde_json::to_string(offsets).unwrap_or_else(|_| "{}".to_string());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json.as_bytes())
}

/// Hard cap on a decoded replay cursor's length, in bytes. A cursor
/// is a JSON object of `{partition: offset}` — at JSON cost of ~20
/// bytes per entry, 8 KB comfortably fits ~400 partitions, well past
/// any realistic Kafka topic. Capping here keeps a malicious client
/// from forcing the server to allocate megabytes for `serde_json` to
/// chew through before any business logic runs.
#[cfg(feature = "bus")]
const MAX_REPLAY_CURSOR_BYTES: usize = 8 * 1024;

/// Decode a cursor produced by [`encode_replay_cursor`]. Tolerates a
/// missing or malformed token with a structured error so the handler
/// can surface a 400. Caps the decoded payload at
/// [`MAX_REPLAY_CURSOR_BYTES`] *before* the JSON parse so a hostile
/// client can't trigger a CPU/memory denial-of-service by sending a megabyte of
/// nested JSON in the query string.
#[cfg(feature = "bus")]
fn decode_replay_cursor(s: &str) -> Result<std::collections::BTreeMap<i32, i64>, String> {
    use base64::Engine;
    // Reject the encoded form first to skip the base64 work entirely
    // when the input is already too big. Base64 expands ~33%, so
    // anything past `MAX * 4 / 3 + a few` bytes can't possibly decode
    // to ≤ MAX bytes.
    if s.len() > MAX_REPLAY_CURSOR_BYTES * 4 / 3 + 4 {
        return Err(format!(
            "cursor too large: {} encoded bytes (max ~{} encoded)",
            s.len(),
            MAX_REPLAY_CURSOR_BYTES * 4 / 3
        ));
    }
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.as_bytes())
        .map_err(|e| format!("base64: {e}"))?;
    if raw.len() > MAX_REPLAY_CURSOR_BYTES {
        return Err(format!(
            "cursor too large: {} decoded bytes (max {MAX_REPLAY_CURSOR_BYTES})",
            raw.len()
        ));
    }
    let json = std::str::from_utf8(&raw).map_err(|e| format!("utf8: {e}"))?;
    serde_json::from_str::<std::collections::BTreeMap<i32, i64>>(json)
        .map_err(|e| format!("json: {e}"))
}

/// Validate a caller-supplied header map. Rejects anything that looks
/// like an attempt to oversize the publish path. The reserved
/// `acteon.*` prefix is checked separately by each handler so the
/// error message can describe which path the prefix conflict came
/// from.
#[cfg(feature = "bus")]
#[allow(clippy::result_large_err)]
fn validate_user_headers(
    headers: &std::collections::BTreeMap<String, String>,
) -> Result<(), axum::response::Response> {
    if headers.len() > MAX_USER_HEADER_COUNT {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!(
                    "too many headers: {}; max is {MAX_USER_HEADER_COUNT}",
                    headers.len()
                ),
            }),
        )
            .into_response());
    }
    for (k, v) in headers {
        if k.len() > MAX_USER_HEADER_KEY_BYTES {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!(
                        "header key length {} exceeds {MAX_USER_HEADER_KEY_BYTES} bytes",
                        k.len()
                    ),
                }),
            )
                .into_response());
        }
        if v.len() > MAX_USER_HEADER_VALUE_BYTES {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!(
                        "header '{k}' value length {} exceeds {MAX_USER_HEADER_VALUE_BYTES} bytes",
                        v.len()
                    ),
                }),
            )
                .into_response());
        }
    }
    Ok(())
}

// =============================================================================
// Request / response DTOs
// =============================================================================

/// Request body for `POST /v1/topics`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateTopicRequest {
    pub name: String,
    pub namespace: String,
    pub tenant: String,
    #[serde(default)]
    pub partitions: Option<i32>,
    #[serde(default)]
    pub replication_factor: Option<i16>,
    #[serde(default)]
    pub retention_ms: Option<i64>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TopicResponse {
    pub name: String,
    pub namespace: String,
    pub tenant: String,
    pub kafka_name: String,
    pub partitions: i32,
    pub replication_factor: i16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retention_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ListTopicsResponse {
    pub topics: Vec<TopicResponse>,
    pub count: usize,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ListTopicsParams {
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub tenant: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PublishRequest {
    /// Target topic. Either the short `namespace.tenant.name` Kafka
    /// form or the three parts separately.
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub tenant: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// Partition key; messages with the same key land on the same
    /// Kafka partition, preserving ordering per key.
    #[serde(default)]
    pub key: Option<String>,
    /// Free-form JSON payload. Phase 3 will add schema validation.
    #[schema(value_type = Object)]
    pub payload: serde_json::Value,
    /// User-supplied headers. The `acteon.*` prefix is reserved for
    /// server-set metadata; any header with that prefix causes a
    /// `400 Bad Request`.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PublishResponse {
    pub topic: String,
    pub partition: i32,
    pub offset: i64,
    pub produced_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct SubscribeParams {
    /// Topic to subscribe to (Kafka name: `namespace.tenant.name`).
    pub topic: String,
    /// Starting position: `earliest` or `latest` (default).
    #[serde(default)]
    pub from: Option<String>,
}

// =============================================================================
// Handlers (feature-gated)
// =============================================================================

fn service_unavailable(msg: &str) -> axum::response::Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ErrorResponse {
            error: msg.to_string(),
        }),
    )
        .into_response()
}

#[utoipa::path(
    post,
    path = "/v1/bus/topics",
    tag = "bus",
    request_body = CreateTopicRequest,
    responses(
        (status = 201, description = "Topic created", body = TopicResponse),
        (status = 400, description = "Invalid topic name", body = ErrorResponse),
        (status = 409, description = "Topic already exists", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
#[allow(clippy::too_many_lines)]
pub async fn create_topic(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Json(req): Json<CreateTopicRequest>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        let Some(backend) = state.bus_backend.as_ref() else {
            return service_unavailable("bus feature not enabled");
        };
        // Enforce tenant scoping: the caller must have a grant covering
        // the `(tenant, namespace, bus, manage)` tuple of the topic they
        // are creating. Without this, a valid caller could create
        // topics under any tenant simply by naming them.
        if let Err(resp) = authorize_bus_op(&identity, &req.tenant, &req.namespace, BusOp::Manage) {
            return resp;
        }
        let mut topic = Topic::new(&req.name, &req.namespace, &req.tenant);
        if let Some(p) = req.partitions {
            topic.partitions = p;
        }
        if let Some(r) = req.replication_factor {
            topic.replication_factor = r;
        }
        topic.retention_ms = req.retention_ms;
        topic.description = req.description.clone();
        if let Err(resp) = validate_user_labels(&req.labels) {
            return resp;
        }
        topic.labels = req.labels.clone();
        if let Err(e) = topic.validate() {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response();
        }

        // Persist to state store first so we have a record even if Kafka
        // admin fails — idempotent retries will reconcile.
        let gw = state.gateway.read().await;
        let store = gw.state_store();
        let key = StateKey::new(
            topic.namespace.clone(),
            topic.tenant.clone(),
            KeyKind::BusTopic,
            topic.id(),
        );
        let Ok(body) = serde_json::to_string(&topic) else {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "failed to serialize topic".into(),
                }),
            )
                .into_response();
        };
        // Atomic conflict check + insert. Two concurrent creates with
        // the same key are guaranteed to see exactly one `true` here;
        // the loser gets `Ok(false)` and a clean 409.
        match store.check_and_set(&key, &body, None).await {
            Ok(true) => {}
            Ok(false) => {
                return (
                    StatusCode::CONFLICT,
                    Json(ErrorResponse {
                        error: format!("topic {} already exists", topic.kafka_topic_name()),
                    }),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        }
        drop(gw);

        // Now create in Kafka. The backend never auto-adopts a
        // pre-existing topic — silently absorbing an out-of-band topic
        // is both a privilege-escalation vector (any caller who can
        // create topics in this tenant could otherwise "claim" a
        // topic an admin pre-created) and a configuration-drift
        // hazard. Any duplicate surfaces as `TopicAlreadyExists` and
        // becomes a 409 below; operators reconcile explicitly.
        if let Err(e) = backend.create_topic(&topic).await {
            // Best-effort rollback of the state row on Kafka failure.
            // If the rollback itself fails — e.g. state store
            // temporary outage — Acteon carries a dangling record that
            // doesn't exist (or differs) in Kafka. Log loudly so
            // operators can reconcile.
            let gw = state.gateway.read().await;
            if let Err(rollback_err) = gw.state_store().delete(&key).await {
                tracing::error!(
                    key = %key.canonical(),
                    kafka_name = %topic.kafka_topic_name(),
                    kafka_error = %e,
                    rollback_error = %rollback_err,
                    "bus: Kafka create_topic failed and state-store rollback also failed — dangling Topic row needs manual cleanup"
                );
            } else {
                tracing::warn!(
                    kafka_name = %topic.kafka_topic_name(),
                    kafka_error = %e,
                    "bus: Kafka create_topic failed; state row rolled back"
                );
            }
            // Duplicate-topic errors get a 409 to match the
            // in-Acteon-state pre-check above; other failures stay
            // as 500.
            let status = if matches!(&e, acteon_bus::BusError::TopicAlreadyExists(_)) {
                StatusCode::CONFLICT
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            return (
                status,
                Json(ErrorResponse {
                    error: format!("kafka create_topic failed: {e}"),
                }),
            )
                .into_response();
        }

        (StatusCode::CREATED, Json(topic_to_response(&topic))).into_response()
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, req);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    get,
    path = "/v1/bus/topics",
    tag = "bus",
    params(ListTopicsParams),
    responses(
        (status = 200, description = "Topic list", body = ListTopicsResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
pub async fn list_topics(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Query(params): Query<ListTopicsParams>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        if state.bus_backend.is_none() {
            return service_unavailable("bus feature not enabled");
        }
        let gw = state.gateway.read().await;
        let store = gw.state_store();
        let entries = match store.scan_keys_by_kind(KeyKind::BusTopic).await {
            Ok(e) => e,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        };
        // Filter by authorization *after* query filters so a caller
        // scoped to tenant A never sees tenant B's topics even if they
        // pass `?tenant=B`. Wildcard-grant callers see everything.
        let topics: Vec<TopicResponse> = entries
            .into_iter()
            .filter_map(|(_, v)| serde_json::from_str::<Topic>(&v).ok())
            .filter(|t| {
                params.namespace.as_deref().is_none_or(|n| n == t.namespace)
                    && params.tenant.as_deref().is_none_or(|tn| tn == t.tenant)
                    && identity.is_authorized(&t.tenant, &t.namespace, "bus", "manage")
            })
            .map(|t| topic_to_response(&t))
            .collect();
        let body = ListTopicsResponse {
            count: topics.len(),
            topics,
        };
        (StatusCode::OK, Json(body)).into_response()
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, params);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    delete,
    path = "/v1/bus/topics/{kafka_name}",
    tag = "bus",
    responses(
        (status = 204, description = "Topic deleted"),
        (status = 404, description = "Topic not found", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
pub async fn delete_topic(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(kafka_name): Path<String>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        let Some(backend) = state.bus_backend.as_ref() else {
            return service_unavailable("bus feature not enabled");
        };
        let (ns, tenant, _) = match parse_kafka_name(&kafka_name) {
            Ok(p) => p,
            Err(msg) => {
                return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: msg }))
                    .into_response();
            }
        };
        if let Err(resp) = authorize_bus_op(&identity, tenant, ns, BusOp::Manage) {
            return resp;
        }
        let key = StateKey::new(ns, tenant, KeyKind::BusTopic, &kafka_name);

        let gw = state.gateway.read().await;
        let store = gw.state_store();
        match store.get(&key).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse {
                        error: format!("topic {kafka_name} not found"),
                    }),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        }
        if let Err(e) = store.delete(&key).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response();
        }
        drop(gw);

        if let Err(e) = backend.delete_topic(&kafka_name).await {
            tracing::error!(
                kafka_error = %e,
                %kafka_name,
                "bus: Kafka delete_topic failed after state row was removed — orphan topic in Kafka needs manual cleanup"
            );
        }
        StatusCode::NO_CONTENT.into_response()
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, kafka_name);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    post,
    path = "/v1/bus/publish",
    tag = "bus",
    request_body = PublishRequest,
    responses(
        (status = 200, description = "Published", body = PublishResponse),
        (status = 400, description = "Invalid topic, reserved header, or schema-validation failure", body = SchemaValidationErrorResponse),
        (status = 404, description = "Topic not found", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
#[allow(clippy::too_many_lines)]
pub async fn publish(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Json(req): Json<PublishRequest>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        let Some(backend) = state.bus_backend.as_ref() else {
            return service_unavailable("bus feature not enabled");
        };
        let topic_name = match resolve_topic_name(&req) {
            Ok(n) => n,
            Err(msg) => {
                return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: msg }))
                    .into_response();
            }
        };
        // Reject oversized header sets before doing anything else; a
        // misbehaving caller could otherwise saturate the producer's
        // memory or trip Kafka's `message.max.bytes` ceiling.
        if let Err(resp) = validate_user_headers(&req.headers) {
            return resp;
        }
        // Reject reserved `acteon.*` headers explicitly so callers see a
        // 400 instead of having the header silently dropped by
        // `BusMessage::with_header`. Silent stripping caused a
        // frustrating debugging experience in the review.
        if let Some(reserved) = req.headers.keys().find(|k| k.starts_with("acteon.")) {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!(
                        "header '{reserved}' uses the reserved 'acteon.' prefix; those are set by the server"
                    ),
                }),
            )
                .into_response();
        }
        // Parse the Kafka name once — we need the tenant + namespace for
        // authorization and for the state-store governance lookup.
        let (ns, tenant, _leaf) = match parse_kafka_name(&topic_name) {
            Ok(p) => p,
            Err(msg) => {
                return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: msg }))
                    .into_response();
            }
        };
        // If the caller supplied an explicit (namespace, tenant, name)
        // triple, cross-check against the resolved topic so a caller
        // can't set one in `topic` and another in the triple to sneak
        // past tenant scoping.
        if let (Some(rn), Some(rt)) = (&req.namespace, &req.tenant)
            && (rn != ns || rt != tenant)
        {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "namespace/tenant mismatch between 'topic' and explicit fields"
                        .to_string(),
                }),
            )
                .into_response();
        }
        // Tenant scoping: require the caller's grant to cover this
        // topic's (tenant, namespace). Without this, a valid caller
        // could publish to any tenant's topic by naming it explicitly.
        if let Err(resp) = authorize_bus_op(&identity, tenant, ns, BusOp::Publish) {
            return resp;
        }
        // Governance: the publish edge is Acteon's control plane. Require
        // that the topic is registered in state *before* we hand the
        // message to Kafka. Without this check a client could bypass
        // Acteon entirely when the broker has `auto.create.topics.enable`
        // — Phase 3 schema validation would have no hook either.
        let topic_key = StateKey::new(ns, tenant, KeyKind::BusTopic, &topic_name);
        let topic: Topic = {
            let gw = state.gateway.read().await;
            match gw.state_store().get(&topic_key).await {
                Ok(Some(raw)) => match serde_json::from_str::<Topic>(&raw) {
                    Ok(t) => t,
                    Err(e) => {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(ErrorResponse {
                                error: format!("corrupt topic record for {topic_name}: {e}"),
                            }),
                        )
                            .into_response();
                    }
                },
                Ok(None) => {
                    return (
                        StatusCode::NOT_FOUND,
                        Json(ErrorResponse {
                            error: format!(
                                "topic {topic_name} is not registered in Acteon; create it with POST /v1/bus/topics first"
                            ),
                        }),
                    )
                        .into_response();
                }
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: e.to_string(),
                        }),
                    )
                        .into_response();
                }
            }
        };
        // Phase 3: if the topic is bound to a schema, validate the
        // payload before we hand it to Kafka. A bound topic with a
        // missing compiled validator is a server bug — log it and fall
        // through rather than blocking publish on a cold cache, since
        // the state row is the source of truth and we re-hydrate on
        // startup via `ensure_schema_in_validator` below.
        if let (Some(subject), Some(version)) = (&topic.schema_subject, topic.schema_version) {
            if let Err(resp) = ensure_schema_in_validator(
                &state,
                &topic.namespace,
                &topic.tenant,
                subject,
                version,
            )
            .await
            {
                return resp;
            }
            match state.bus_schema_validator.validate(
                &topic.namespace,
                &topic.tenant,
                subject,
                version,
                &req.payload,
            ) {
                Ok(()) => {}
                Err(acteon_bus::SchemaValidatorError::InvalidPayload(issues)) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(SchemaValidationErrorResponse {
                            error: format!("payload does not match schema '{subject}' v{version}"),
                            subject: subject.clone(),
                            version,
                            issues: issues
                                .into_iter()
                                .map(|i| SchemaValidationIssueDto {
                                    path: i.path,
                                    message: i.message,
                                })
                                .collect(),
                        }),
                    )
                        .into_response();
                }
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: e.to_string(),
                        }),
                    )
                        .into_response();
                }
            }
        }
        let mut msg = acteon_bus::BusMessage::new(topic_name.clone(), req.payload.clone());
        if let Some(k) = req.key.clone() {
            msg = msg.with_key(k);
        }
        for (k, v) in &req.headers {
            msg = msg.with_header(k.clone(), v.clone());
        }
        match backend.produce(msg).await {
            Ok(receipt) => (
                StatusCode::OK,
                Json(PublishResponse {
                    topic: receipt.topic,
                    partition: receipt.partition,
                    offset: receipt.offset,
                    produced_at: receipt.timestamp,
                }),
            )
                .into_response(),
            Err(acteon_bus::BusError::TopicNotFound(_)) => (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("topic {topic_name} not found"),
                }),
            )
                .into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response(),
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, req);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    get,
    path = "/v1/bus/subscribe/{subscription_id}",
    tag = "bus",
    params(SubscribeParams),
    responses(
        (status = 200, description = "SSE stream of bus messages"),
        (status = 404, description = "Topic not found", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
pub async fn subscribe(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(subscription_id): Path<String>,
    Query(params): Query<SubscribeParams>,
) -> axum::response::Response {
    #[cfg(feature = "bus")]
    {
        let Some(backend) = state.bus_backend.clone() else {
            return service_unavailable("bus feature not enabled");
        };
        let (ns, tenant, _leaf) = match parse_kafka_name(&params.topic) {
            Ok(p) => p,
            Err(msg) => {
                return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: msg }))
                    .into_response();
            }
        };
        if let Err(resp) = authorize_bus_op(&identity, tenant, ns, BusOp::Subscribe) {
            return resp;
        }
        let from = match params.from.as_deref() {
            Some("earliest") => acteon_bus::StartOffset::Earliest,
            Some("latest") | None => acteon_bus::StartOffset::Latest,
            Some(other) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: format!("unknown 'from' value '{other}' (expected earliest|latest)"),
                    }),
                )
                    .into_response();
            }
        };
        let topic = params.topic.clone();
        let inner = match backend.subscribe(&topic, &subscription_id, from).await {
            Ok(s) => s,
            Err(acteon_bus::BusError::TopicNotFound(_)) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse {
                        error: format!("topic {topic} not found"),
                    }),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        };
        let stream = sse_stream(inner);
        Sse::new(stream)
            .keep_alive(
                KeepAlive::new()
                    .interval(Duration::from_secs(15))
                    .text("keep-alive"),
            )
            .into_response()
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, subscription_id, params);
        service_unavailable("bus feature not compiled")
    }
}

// =============================================================================
// Helpers
// =============================================================================

#[cfg(feature = "bus")]
fn topic_to_response(t: &Topic) -> TopicResponse {
    TopicResponse {
        name: t.name.clone(),
        namespace: t.namespace.clone(),
        tenant: t.tenant.clone(),
        kafka_name: t.kafka_topic_name(),
        partitions: t.partitions,
        replication_factor: t.replication_factor,
        retention_ms: t.retention_ms,
        description: t.description.clone(),
        labels: t.labels.clone(),
        schema_subject: t.schema_subject.clone(),
        schema_version: t.schema_version,
        created_at: t.created_at,
        updated_at: t.updated_at,
    }
}

#[cfg(feature = "bus")]
fn resolve_topic_name(req: &PublishRequest) -> Result<String, String> {
    if let Some(t) = &req.topic {
        return Ok(t.clone());
    }
    match (&req.namespace, &req.tenant, &req.name) {
        (Some(ns), Some(t), Some(n)) => Ok(format!("{ns}.{t}.{n}")),
        _ => Err("must supply either 'topic' or all of 'namespace', 'tenant', 'name'".to_string()),
    }
}

#[cfg(feature = "bus")]
fn sse_stream(
    inner: acteon_bus::SubscribeStream,
) -> impl Stream<Item = Result<Event, Infallible>> + Send + 'static {
    inner.map(|result| {
        let ev = match result {
            Ok(msg) => {
                let id = msg.offset.unwrap_or_default().to_string();
                let data = serde_json::to_string(&msg).unwrap_or_else(|_| "{}".into());
                Event::default().event("bus.message").id(id).data(data)
            }
            Err(e) => Event::default()
                .event("bus.error")
                .data(format!("{{\"error\":\"{e}\"}}")),
        };
        Ok::<_, Infallible>(ev)
    })
}

// =============================================================================
// Phase 2 — Subscriptions + ack + lag + DLQ
// =============================================================================

/// Request body for `POST /v1/bus/subscriptions`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateSubscriptionRequest {
    /// Stable identifier. Doubles as the Kafka `group.id`.
    /// Must be `[a-zA-Z0-9_-]{1..=120}`.
    pub id: String,
    /// Target Kafka topic (full `namespace.tenant.name` form).
    /// Must belong to the same `(namespace, tenant)` as the
    /// subscription — cross-tenant subscriptions are rejected.
    pub topic: String,
    pub namespace: String,
    pub tenant: String,
    /// `earliest` or `latest`. Defaults to `latest`.
    #[serde(default)]
    pub starting_offset: Option<String>,
    /// `manual` (default) or `auto_on_delivery`.
    #[serde(default)]
    pub ack_mode: Option<String>,
    /// Optional DLQ topic (`namespace.tenant.name`). Must also belong
    /// to the subscription's tenant and be registered in Acteon state.
    #[serde(default)]
    pub dead_letter_topic: Option<String>,
    /// Ack timeout in milliseconds.
    #[serde(default)]
    pub ack_timeout_ms: Option<u64>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SubscriptionResponse {
    pub id: String,
    pub topic: String,
    pub namespace: String,
    pub tenant: String,
    pub starting_offset: String,
    pub ack_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dead_letter_topic: Option<String>,
    pub ack_timeout_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ListSubscriptionsResponse {
    pub subscriptions: Vec<SubscriptionResponse>,
    pub count: usize,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ListSubscriptionsParams {
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub tenant: Option<String>,
    #[serde(default)]
    pub topic: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AckRequest {
    pub partition: i32,
    /// Last consumed offset. The bus commits `offset + 1` to Kafka so
    /// a reconnecting consumer resumes after this record.
    pub offset: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AckResponse {
    pub committed: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LagEntry {
    pub partition: i32,
    pub committed: i64,
    pub high_water_mark: i64,
    pub lag: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LagResponse {
    pub subscription_id: String,
    pub topic: String,
    pub partitions: Vec<LagEntry>,
    pub total_lag: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DeadLetterRequest {
    pub partition: i32,
    pub offset: i64,
    pub reason: String,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    #[schema(value_type = Option<Object>)]
    pub payload: Option<serde_json::Value>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DeadLetterResponse {
    pub dlq_topic: String,
    pub partition: i32,
    pub offset: i64,
}

// =============================================================================
// Handlers
// =============================================================================

#[utoipa::path(
    post,
    path = "/v1/bus/subscriptions",
    tag = "bus",
    request_body = CreateSubscriptionRequest,
    responses(
        (status = 201, description = "Subscription created", body = SubscriptionResponse),
        (status = 400, description = "Invalid request or cross-tenant topic", body = ErrorResponse),
        (status = 404, description = "Topic or DLQ topic not registered in Acteon state", body = ErrorResponse),
        (status = 409, description = "Subscription id already exists", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
#[allow(clippy::too_many_lines)]
pub async fn create_subscription(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Json(req): Json<CreateSubscriptionRequest>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        if state.bus_backend.is_none() {
            return service_unavailable("bus feature not enabled");
        }
        // Tenant-isolation invariant #1: the subscription tenant must
        // match the topic tenant. Otherwise Tenant-A could durably
        // read Tenant-B's records simply by crafting a topic string.
        let (topic_ns, topic_tenant, _) = match parse_kafka_name(&req.topic) {
            Ok(p) => p,
            Err(msg) => {
                return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: msg }))
                    .into_response();
            }
        };
        if topic_ns != req.namespace || topic_tenant != req.tenant {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!(
                        "cross-tenant subscription rejected: topic '{}' belongs to \
                         {topic_ns}.{topic_tenant}, subscription is for {}.{}",
                        req.topic, req.namespace, req.tenant
                    ),
                }),
            )
                .into_response();
        }
        // Tenant-isolation invariant #2: if a DLQ is supplied, it must
        // also belong to the subscription's tenant.
        if let Some(dlq) = &req.dead_letter_topic {
            let (dlq_ns, dlq_tenant, _) = match parse_kafka_name(dlq) {
                Ok(p) => p,
                Err(msg) => {
                    return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: msg }))
                        .into_response();
                }
            };
            if dlq_ns != req.namespace || dlq_tenant != req.tenant {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: format!(
                            "cross-tenant DLQ rejected: dead_letter_topic '{dlq}' belongs to \
                             {dlq_ns}.{dlq_tenant}, subscription is for {}.{}",
                            req.namespace, req.tenant
                        ),
                    }),
                )
                    .into_response();
            }
        }
        if let Err(resp) = authorize_bus_op(&identity, &req.tenant, &req.namespace, BusOp::Manage) {
            return resp;
        }
        let mut sub =
            acteon_core::Subscription::new(&req.id, &req.topic, &req.namespace, &req.tenant);
        if let Some(so) = req.starting_offset.as_deref() {
            sub.starting_offset = match so {
                "earliest" => acteon_core::SubscriptionStartOffset::Earliest,
                "latest" => acteon_core::SubscriptionStartOffset::Latest,
                other => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse {
                            error: format!("unknown starting_offset '{other}'"),
                        }),
                    )
                        .into_response();
                }
            };
        }
        if let Some(am) = req.ack_mode.as_deref() {
            sub.ack_mode = match am {
                "manual" => acteon_core::AckMode::Manual,
                "auto_on_delivery" => acteon_core::AckMode::AutoOnDelivery,
                other => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse {
                            error: format!("unknown ack_mode '{other}'"),
                        }),
                    )
                        .into_response();
                }
            };
        }
        sub.dead_letter_topic = req.dead_letter_topic.clone();
        if let Some(t) = req.ack_timeout_ms {
            sub.ack_timeout_ms = t;
        }
        sub.description = req.description.clone();
        if let Err(resp) = validate_user_labels(&req.labels) {
            return resp;
        }
        sub.labels = req.labels.clone();
        if let Err(e) = sub.validate() {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response();
        }

        // Governance: both the target topic and the DLQ must be
        // registered in Acteon state. Closes the same shadow-topic
        // bypass that /publish guards against.
        let gw = state.gateway.read().await;
        let store = gw.state_store();
        let topic_key = StateKey::new(topic_ns, topic_tenant, KeyKind::BusTopic, &req.topic);
        match store.get(&topic_key).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse {
                        error: format!("topic {} is not registered in Acteon", req.topic),
                    }),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        }
        if let Some(dlq) = &sub.dead_letter_topic {
            let dlq_key = StateKey::new(
                sub.namespace.clone(),
                sub.tenant.clone(),
                KeyKind::BusTopic,
                dlq.clone(),
            );
            match store.get(&dlq_key).await {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return (
                        StatusCode::NOT_FOUND,
                        Json(ErrorResponse {
                            error: format!("dead_letter_topic {dlq} is not registered in Acteon"),
                        }),
                    )
                        .into_response();
                }
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: e.to_string(),
                        }),
                    )
                        .into_response();
                }
            }
        }

        let sub_key = StateKey::new(
            sub.namespace.clone(),
            sub.tenant.clone(),
            KeyKind::BusSubscription,
            sub.id.clone(),
        );
        let Ok(body) = serde_json::to_string(&sub) else {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "failed to serialize subscription".into(),
                }),
            )
                .into_response();
        };
        // Atomic conflict check + insert; see `create_topic` for the
        // rationale.
        match store.check_and_set(&sub_key, &body, None).await {
            Ok(true) => {}
            Ok(false) => {
                return (
                    StatusCode::CONFLICT,
                    Json(ErrorResponse {
                        error: format!("subscription {} already exists", sub.id),
                    }),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        }

        (StatusCode::CREATED, Json(subscription_to_response(&sub))).into_response()
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, req);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    get,
    path = "/v1/bus/subscriptions",
    tag = "bus",
    params(ListSubscriptionsParams),
    responses(
        (status = 200, description = "Subscription list", body = ListSubscriptionsResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
pub async fn list_subscriptions(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Query(params): Query<ListSubscriptionsParams>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        if state.bus_backend.is_none() {
            return service_unavailable("bus feature not enabled");
        }
        let gw = state.gateway.read().await;
        let store = gw.state_store();
        let entries = match store.scan_keys_by_kind(KeyKind::BusSubscription).await {
            Ok(e) => e,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        };
        let subs: Vec<SubscriptionResponse> = entries
            .into_iter()
            .filter_map(|(_, v)| serde_json::from_str::<acteon_core::Subscription>(&v).ok())
            .filter(|s| {
                params.namespace.as_deref().is_none_or(|n| n == s.namespace)
                    && params.tenant.as_deref().is_none_or(|t| t == s.tenant)
                    && params.topic.as_deref().is_none_or(|t| t == s.topic)
                    && identity.is_authorized(&s.tenant, &s.namespace, "bus", "manage")
            })
            .map(|s| subscription_to_response(&s))
            .collect();
        let body = ListSubscriptionsResponse {
            count: subs.len(),
            subscriptions: subs,
        };
        (StatusCode::OK, Json(body)).into_response()
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, params);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    delete,
    path = "/v1/bus/subscriptions/{namespace}/{tenant}/{id}",
    tag = "bus",
    responses(
        (status = 204, description = "Subscription deleted"),
        (status = 404, description = "Subscription not found", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
pub async fn delete_subscription(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, id)): Path<(String, String, String)>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        if state.bus_backend.is_none() {
            return service_unavailable("bus feature not enabled");
        }
        if let Err(resp) = authorize_bus_op(&identity, &tenant, &namespace, BusOp::Manage) {
            return resp;
        }
        // Direct O(1) StateKey lookup — no scan, no O(N) filter in
        // memory. This is the payoff of putting (namespace, tenant) in
        // the URL path.
        let key = StateKey::new(
            namespace.clone(),
            tenant.clone(),
            KeyKind::BusSubscription,
            id.clone(),
        );
        let gw = state.gateway.read().await;
        let store = gw.state_store();
        match store.get(&key).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse {
                        error: format!("subscription {namespace}.{tenant}.{id} not found"),
                    }),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        }
        if let Err(e) = store.delete(&key).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response();
        }
        StatusCode::NO_CONTENT.into_response()
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, namespace, tenant, id);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    post,
    path = "/v1/bus/subscriptions/{namespace}/{tenant}/{id}/ack",
    tag = "bus",
    request_body = AckRequest,
    responses(
        (status = 200, description = "Offset committed", body = AckResponse),
        (status = 404, description = "Subscription not found", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
pub async fn ack_subscription(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, id)): Path<(String, String, String)>,
    Json(req): Json<AckRequest>,
) -> impl IntoResponse {
    // **Performance warning**: this endpoint spins up a fresh Kafka
    // consumer, performs a full JoinGroup/SyncGroup round-trip, then
    // commits. The Kafka round-trip is hundreds of milliseconds on a
    // warm broker and is **not** suitable for per-record acks in a
    // high-throughput workload. Use it for end-of-batch checkpoints
    // only. A future phase introduces a stateful subscription
    // registry that holds one long-lived consumer so commits stream
    // through it with microsecond overhead.
    #[cfg(feature = "bus")]
    {
        let Some(backend) = state.bus_backend.clone() else {
            return service_unavailable("bus feature not enabled");
        };
        if let Err(resp) = authorize_bus_op(&identity, &tenant, &namespace, BusOp::Subscribe) {
            return resp;
        }
        let sub = match load_subscription(&state, &namespace, &tenant, &id).await {
            Ok(s) => s,
            Err(resp) => return resp,
        };
        match backend
            .commit_offset(
                &sub.topic,
                &sub.id,
                acteon_bus::OffsetPosition {
                    partition: req.partition,
                    offset: req.offset,
                },
            )
            .await
        {
            Ok(()) => (StatusCode::OK, Json(AckResponse { committed: true })).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response(),
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, namespace, tenant, id, req);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    get,
    path = "/v1/bus/subscriptions/{namespace}/{tenant}/{id}/lag",
    tag = "bus",
    responses(
        (status = 200, description = "Lag snapshot", body = LagResponse),
        (status = 404, description = "Subscription not found", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
pub async fn subscription_lag(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, id)): Path<(String, String, String)>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        let Some(backend) = state.bus_backend.clone() else {
            return service_unavailable("bus feature not enabled");
        };
        if let Err(resp) = authorize_bus_op(&identity, &tenant, &namespace, BusOp::Subscribe) {
            return resp;
        }
        let sub = match load_subscription(&state, &namespace, &tenant, &id).await {
            Ok(s) => s,
            Err(resp) => return resp,
        };
        match backend.consumer_lag(&sub.topic, &sub.id).await {
            Ok(entries) => {
                let total_lag: i64 = entries.iter().map(|e| e.lag).sum();
                let body = LagResponse {
                    subscription_id: sub.id.clone(),
                    topic: sub.topic.clone(),
                    partitions: entries
                        .into_iter()
                        .map(|e| LagEntry {
                            partition: e.partition,
                            committed: e.committed,
                            high_water_mark: e.high_water_mark,
                            lag: e.lag,
                        })
                        .collect(),
                    total_lag,
                };
                (StatusCode::OK, Json(body)).into_response()
            }
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response(),
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, namespace, tenant, id);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    post,
    path = "/v1/bus/subscriptions/{namespace}/{tenant}/{id}/deadletter",
    tag = "bus",
    request_body = DeadLetterRequest,
    responses(
        (status = 200, description = "Message routed to DLQ", body = DeadLetterResponse),
        (status = 400, description = "Subscription has no DLQ configured", body = ErrorResponse),
        (status = 404, description = "Subscription or DLQ topic not found", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
#[allow(clippy::too_many_lines)]
pub async fn deadletter_subscription(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, id)): Path<(String, String, String)>,
    Json(req): Json<DeadLetterRequest>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        let Some(backend) = state.bus_backend.clone() else {
            return service_unavailable("bus feature not enabled");
        };
        if let Err(resp) = authorize_bus_op(&identity, &tenant, &namespace, BusOp::Subscribe) {
            return resp;
        }
        let sub = match load_subscription(&state, &namespace, &tenant, &id).await {
            Ok(s) => s,
            Err(resp) => return resp,
        };
        let Some(dlq) = sub.dead_letter_topic.clone() else {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("subscription {id} has no dead_letter_topic configured"),
                }),
            )
                .into_response();
        };
        if let Err(resp) = validate_user_headers(&req.headers) {
            return resp;
        }
        // Governance: confirm the DLQ topic is still registered in
        // state. Normally guaranteed by create_subscription but the
        // topic could have been deleted since then — we don't want to
        // silently produce to a shadow topic if Acteon state no longer
        // knows about it.
        let dlq_key = StateKey::new(
            sub.namespace.clone(),
            sub.tenant.clone(),
            KeyKind::BusTopic,
            dlq.clone(),
        );
        {
            let gw = state.gateway.read().await;
            match gw.state_store().get(&dlq_key).await {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return (
                        StatusCode::NOT_FOUND,
                        Json(ErrorResponse {
                            error: format!(
                                "dead_letter_topic {dlq} is no longer registered in Acteon"
                            ),
                        }),
                    )
                        .into_response();
                }
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: e.to_string(),
                        }),
                    )
                        .into_response();
                }
            }
        }
        let mut msg = acteon_bus::BusMessage::new(
            dlq.clone(),
            req.payload.clone().unwrap_or(serde_json::Value::Null),
        );
        if let Some(k) = req.key.clone() {
            msg = msg.with_key(k);
        }
        for (k, v) in &req.headers {
            msg = msg.with_header(k.clone(), v.clone());
        }
        msg.headers
            .insert("acteon.dlq.origin_topic".into(), sub.topic.clone());
        msg.headers.insert(
            "acteon.dlq.origin_partition".into(),
            req.partition.to_string(),
        );
        msg.headers
            .insert("acteon.dlq.origin_offset".into(), req.offset.to_string());
        msg.headers
            .insert("acteon.dlq.subscription".into(), sub.id.clone());
        msg.headers
            .insert("acteon.dlq.reason".into(), req.reason.clone());

        match backend.produce(msg).await {
            Ok(receipt) => (
                StatusCode::OK,
                Json(DeadLetterResponse {
                    dlq_topic: receipt.topic,
                    partition: receipt.partition,
                    offset: receipt.offset,
                }),
            )
                .into_response(),
            Err(acteon_bus::BusError::TopicNotFound(_)) => (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("dead-letter topic {dlq} not found in Kafka"),
                }),
            )
                .into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response(),
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, namespace, tenant, id, req);
        service_unavailable("bus feature not compiled")
    }
}

// =============================================================================
// Helpers
// =============================================================================

#[cfg(feature = "bus")]
fn subscription_to_response(s: &acteon_core::Subscription) -> SubscriptionResponse {
    SubscriptionResponse {
        id: s.id.clone(),
        topic: s.topic.clone(),
        namespace: s.namespace.clone(),
        tenant: s.tenant.clone(),
        starting_offset: match s.starting_offset {
            acteon_core::SubscriptionStartOffset::Earliest => "earliest".into(),
            acteon_core::SubscriptionStartOffset::Latest => "latest".into(),
        },
        ack_mode: match s.ack_mode {
            acteon_core::AckMode::Manual => "manual".into(),
            acteon_core::AckMode::AutoOnDelivery => "auto_on_delivery".into(),
        },
        dead_letter_topic: s.dead_letter_topic.clone(),
        ack_timeout_ms: s.ack_timeout_ms,
        description: s.description.clone(),
        labels: s.labels.clone(),
        created_at: s.created_at,
        updated_at: s.updated_at,
    }
}

/// O(1) direct lookup of a subscription by its full `(namespace, tenant, id)`
/// triple. Replaces the O(N) scan from an earlier draft.
#[cfg(feature = "bus")]
async fn load_subscription(
    state: &AppState,
    namespace: &str,
    tenant: &str,
    id: &str,
) -> Result<acteon_core::Subscription, axum::response::Response> {
    let key = StateKey::new(
        namespace.to_string(),
        tenant.to_string(),
        KeyKind::BusSubscription,
        id.to_string(),
    );
    let gw = state.gateway.read().await;
    match gw.state_store().get(&key).await {
        Ok(Some(raw)) => serde_json::from_str::<acteon_core::Subscription>(&raw).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!(
                        "corrupt subscription record for {namespace}.{tenant}.{id}: {e}"
                    ),
                }),
            )
                .into_response()
        }),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("subscription {namespace}.{tenant}.{id} not found"),
            }),
        )
            .into_response()),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response()),
    }
}

// =============================================================================
// Phase 3: JSON-Schema registry + topic binding
// =============================================================================

/// Bound on how many times `create_schema` retries scan-then-claim for
/// the next monotonic version. With N concurrent registrations on the
/// same subject, the worst case is N attempts; 8 is comfortably above
/// any realistic operator-driven concurrency on schema CRUD.
#[cfg(feature = "bus")]
const MAX_VERSION_ALLOC_ATTEMPTS: u32 = 8;

/// Body of a request to register a new schema version.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateSchemaRequest {
    pub subject: String,
    pub namespace: String,
    pub tenant: String,
    /// JSON Schema document (draft 2020-12). Validated by the
    /// `jsonschema` crate when compiled.
    #[schema(value_type = Object)]
    pub body: serde_json::Value,
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

/// Wire representation of a registered schema version.
#[derive(Debug, Serialize, ToSchema)]
pub struct SchemaResponse {
    pub subject: String,
    pub version: i32,
    pub namespace: String,
    pub tenant: String,
    #[schema(value_type = Object)]
    pub body: serde_json::Value,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ListSchemasResponse {
    pub schemas: Vec<SchemaResponse>,
    pub count: usize,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ListSchemasParams {
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub tenant: Option<String>,
    #[serde(default)]
    pub subject: Option<String>,
    /// When true, return only the latest version per subject (default
    /// false — returns all versions).
    #[serde(default)]
    pub latest_only: bool,
}

/// Body of a `PUT /v1/bus/topics/{ns}/{t}/{name}/schema` request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct BindTopicSchemaRequest {
    pub subject: String,
    /// Specific version to pin. Must be >= 1.
    pub version: i32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BindTopicSchemaResponse {
    pub topic: String,
    pub subject: String,
    pub version: i32,
}

/// Response body for payload-validation failures. Uses a distinct type
/// so `OpenAPI` consumers can see the per-issue detail without matching
/// on `ErrorResponse`.
#[derive(Debug, Serialize, ToSchema)]
pub struct SchemaValidationErrorResponse {
    pub error: String,
    pub subject: String,
    pub version: i32,
    pub issues: Vec<SchemaValidationIssueDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SchemaValidationIssueDto {
    pub path: String,
    pub message: String,
}

#[cfg(feature = "bus")]
fn schema_to_response(s: &acteon_core::Schema) -> SchemaResponse {
    SchemaResponse {
        subject: s.subject.clone(),
        version: s.version,
        namespace: s.namespace.clone(),
        tenant: s.tenant.clone(),
        body: s.body.clone(),
        labels: s.labels.clone(),
        created_at: s.created_at,
    }
}

/// Rehydrate a schema body into the in-memory validator cache if it
/// isn't there already. The validator cache is process-local and
/// survives only while the server is up, so on cold-start or after the
/// cache was evicted we need to read from state and recompile.
///
/// Returns `Err(Response)` for surfacing to callers when the schema row
/// is missing (bound topic with deleted schema — a governance hole)
/// or when recompilation fails.
#[cfg(feature = "bus")]
async fn ensure_schema_in_validator(
    state: &AppState,
    namespace: &str,
    tenant: &str,
    subject: &str,
    version: i32,
) -> Result<(), axum::response::Response> {
    // Optimistic path: validate() below will succeed if the entry is
    // present. We re-register unconditionally here which is a cheap
    // hash-map write; the extra cost is negligible vs. the cross-
    // process cache-miss path.
    let key = StateKey::new(
        namespace.to_string(),
        tenant.to_string(),
        KeyKind::BusSchema,
        format!("{subject}:{version}"),
    );
    let gw = state.gateway.read().await;
    match gw.state_store().get(&key).await {
        Ok(Some(raw)) => match serde_json::from_str::<acteon_core::Schema>(&raw) {
            Ok(s) => state
                .bus_schema_validator
                .register(&s.namespace, &s.tenant, &s.subject, s.version, &s.body)
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: format!("failed to compile schema {subject}:{version}: {e}"),
                        }),
                    )
                        .into_response()
                }),
            Err(e) => Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("corrupt schema record for {subject}:{version}: {e}"),
                }),
            )
                .into_response()),
        },
        Ok(None) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!(
                    "topic binding points at missing schema '{subject}' v{version} — unbind or re-register the schema"
                ),
            }),
        )
            .into_response()),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response()),
    }
}

/// Enforce read-side participant ACL. The write-side handlers
/// (`append_conversation_message`, `post_tool_call`,
/// `post_tool_result`) already gate on the envelope's `sender`; the
/// read paths used to skip the equivalent check, so any caller with
/// the tenant-level `ManageConversation` grant could read a
/// "private" conversation just by knowing its id. This helper
/// closes that gap.
///
/// Contract:
/// - Empty participants list → ACL is open; no `as_agent` required.
/// - Non-empty list → caller must supply `as_agent`, and it must be
///   on the list. Otherwise 403.
///
/// V1 read-isolation: the asserted `as_agent` is taken at face value
/// once the caller has the tenant grant. A future iteration can
/// derive the agent identity from the API-key grant directly so
/// callers can't claim arbitrary agent identities.
#[cfg(feature = "bus")]
fn enforce_conversation_read_acl(
    conv: &acteon_core::Conversation,
    as_agent: Option<&str>,
) -> Result<(), axum::response::Response> {
    if conv.participants.is_empty() {
        return Ok(());
    }
    match as_agent {
        Some(a) if conv.participants.iter().any(|p| p == a) => Ok(()),
        Some(a) => Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: format!(
                    "as_agent '{a}' is not a participant of conversation {}; cannot read",
                    conv.conversation_id
                ),
            }),
        )
            .into_response()),
        None => Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: format!(
                    "conversation {} has a participant ACL; pass `as_agent` matching one of the participants",
                    conv.conversation_id
                ),
            }),
        )
            .into_response()),
    }
}

/// Apply Phase 3 schema validation to a payload destined for
/// `kafka_topic`. Used by every publish-style handler that doesn't
/// already inline the validation block (the original `/v1/bus/publish`
/// handler still does it inline; the conversation-layer handlers —
/// tool calls, tool results — call this so a schema-bound events
/// topic enforces uniformly).
///
/// No-ops cleanly when the topic record is missing or has no
/// `(schema_subject, schema_version)` binding. On a binding mismatch,
/// returns a 400 with the same `SchemaValidationErrorResponse` shape
/// the publish handler uses, so SDK consumers see consistent errors.
#[cfg(feature = "bus")]
async fn validate_payload_against_topic_schema(
    state: &AppState,
    namespace: &str,
    tenant: &str,
    kafka_topic: &str,
    payload: &serde_json::Value,
) -> Result<(), axum::response::Response> {
    // Clone the `Arc<dyn StateStore>` and drop the gateway guard
    // before any `.await` — same lock-amplification rule the rest of
    // the bus handlers follow.
    let store: std::sync::Arc<dyn acteon_state::StateStore> = {
        let gw = state.gateway.read().await;
        gw.state_store().clone()
    };
    let key = StateKey::new(
        namespace.to_string(),
        tenant.to_string(),
        KeyKind::BusTopic,
        kafka_topic,
    );
    let topic: Topic = match store.get(&key).await {
        Ok(Some(raw)) => match serde_json::from_str(&raw) {
            Ok(t) => t,
            // Corrupt row — skip validation, let the underlying
            // produce surface the issue. This mirrors the publish
            // handler's resilience.
            Err(_) => return Ok(()),
        },
        // Missing topic row (someone deleted it out of band): no
        // binding to enforce. The underlying produce will fail
        // separately if Kafka has lost the topic too.
        Ok(None) | Err(_) => return Ok(()),
    };
    let (Some(subject), Some(version)) = (&topic.schema_subject, topic.schema_version) else {
        return Ok(());
    };
    ensure_schema_in_validator(state, &topic.namespace, &topic.tenant, subject, version).await?;
    match state.bus_schema_validator.validate(
        &topic.namespace,
        &topic.tenant,
        subject,
        version,
        payload,
    ) {
        Ok(()) => Ok(()),
        Err(acteon_bus::SchemaValidatorError::InvalidPayload(issues)) => Err((
            StatusCode::BAD_REQUEST,
            Json(SchemaValidationErrorResponse {
                error: format!("payload does not match schema '{subject}' v{version}"),
                subject: subject.clone(),
                version,
                issues: issues
                    .into_iter()
                    .map(|i| SchemaValidationIssueDto {
                        path: i.path,
                        message: i.message,
                    })
                    .collect(),
            }),
        )
            .into_response()),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response()),
    }
}

#[utoipa::path(
    post,
    path = "/v1/bus/schemas",
    tag = "bus",
    request_body = CreateSchemaRequest,
    responses(
        (status = 201, description = "Schema version registered", body = SchemaResponse),
        (status = 400, description = "Invalid subject or schema body", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
#[allow(clippy::too_many_lines)]
pub async fn create_schema(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Json(req): Json<CreateSchemaRequest>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        if state.bus_backend.is_none() {
            return service_unavailable("bus feature not enabled");
        }
        if let Err(resp) =
            authorize_bus_op(&identity, &req.tenant, &req.namespace, BusOp::ManageSchema)
        {
            return resp;
        }
        if let Err(resp) = validate_user_labels(&req.labels) {
            return resp;
        }
        if let Err(e) = acteon_core::Schema::validate_subject(&req.subject) {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response();
        }
        if let Err(e) = acteon_core::Schema::validate_fragment(&req.namespace) {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("invalid namespace: {e}"),
                }),
            )
                .into_response();
        }
        if let Err(e) = acteon_core::Schema::validate_fragment(&req.tenant) {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("invalid tenant: {e}"),
                }),
            )
                .into_response();
        }
        // Compile the body first — a bad body should fail fast with a
        // 400 rather than land in state and cause 500s on publish later.
        if let Err(e) = acteon_bus::SchemaValidator::new().register(
            &req.namespace,
            &req.tenant,
            &req.subject,
            1,
            &req.body,
        ) {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("schema body invalid: {e}"),
                }),
            )
                .into_response();
        }
        // Reserving the next monotonic version requires a scan-then-
        // claim pattern. Two concurrent posts can compute the same
        // `next_version`; the atomic `check_and_set` below detects
        // this and we retry up to MAX_VERSION_ALLOC_ATTEMPTS. With N
        // concurrent posts the loop is N attempts at worst, which is
        // fine — V1 throughput on a single subject is operator-driven.
        let gw = state.gateway.read().await;
        let prefix = format!("{}:", req.subject);
        let mut allocated: Option<acteon_core::Schema> = None;
        for _ in 0..MAX_VERSION_ALLOC_ATTEMPTS {
            let next_version: i32 = match gw
                .state_store()
                .scan_keys(
                    &req.namespace,
                    &req.tenant,
                    KeyKind::BusSchema,
                    Some(&prefix),
                )
                .await
            {
                Ok(rows) => rows
                    .iter()
                    .filter_map(|(_, v)| serde_json::from_str::<acteon_core::Schema>(v).ok())
                    .filter(|s| s.subject == req.subject)
                    .map(|s| s.version)
                    .max()
                    .map_or(1, |m| m + 1),
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: e.to_string(),
                        }),
                    )
                        .into_response();
                }
            };
            // Stamp `created_at` per attempt so the persisted timestamp
            // reflects when the version was actually claimed, not when
            // the handler started. Version is the canonical ordering
            // signal; this just keeps timestamps honest under retry.
            let candidate = acteon_core::Schema {
                subject: req.subject.clone(),
                version: next_version,
                namespace: req.namespace.clone(),
                tenant: req.tenant.clone(),
                format: acteon_core::SchemaFormat::default(),
                body: req.body.clone(),
                labels: req.labels.clone(),
                created_at: Utc::now(),
            };
            if let Err(e) = candidate.validate() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
            let key = StateKey::new(
                candidate.namespace.clone(),
                candidate.tenant.clone(),
                KeyKind::BusSchema,
                candidate.id(),
            );
            let payload = match serde_json::to_string(&candidate) {
                Ok(s) => s,
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: e.to_string(),
                        }),
                    )
                        .into_response();
                }
            };
            match gw.state_store().check_and_set(&key, &payload, None).await {
                Ok(true) => {
                    allocated = Some(candidate);
                    break;
                }
                // Lost the race against another concurrent registration
                // for this subject — rescan and retry on next loop
                // iteration.
                Ok(false) => {}
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: e.to_string(),
                        }),
                    )
                        .into_response();
                }
            }
        }
        let Some(schema) = allocated else {
            return (
                StatusCode::CONFLICT,
                Json(ErrorResponse {
                    error: format!(
                        "schema '{}' could not allocate a unique version after {MAX_VERSION_ALLOC_ATTEMPTS} attempts; retry or coordinate writers",
                        req.subject
                    ),
                }),
            )
                .into_response();
        };
        drop(gw);
        // Eagerly register with the compiled-validator cache so the
        // next publish is a warm hit.
        let _ = state.bus_schema_validator.register(
            &schema.namespace,
            &schema.tenant,
            &schema.subject,
            schema.version,
            &schema.body,
        );
        (StatusCode::CREATED, Json(schema_to_response(&schema))).into_response()
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, req);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    get,
    path = "/v1/bus/schemas",
    tag = "bus",
    params(ListSchemasParams),
    responses(
        (status = 200, description = "Schema list", body = ListSchemasResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
pub async fn list_schemas(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Query(params): Query<ListSchemasParams>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        if state.bus_backend.is_none() {
            return service_unavailable("bus feature not enabled");
        }
        // Authorize with whatever scope the caller filtered on; when
        // neither namespace nor tenant is given, fall through to caller
        // iteration below — list is a read surface without mutations,
        // but we still gate on ManageSchema so downstream tools don't
        // accidentally expose schema bodies to low-privilege clients.
        let (ns_filter, t_filter) = (params.namespace.as_deref(), params.tenant.as_deref());
        if let (Some(ns), Some(t)) = (ns_filter, t_filter) {
            if let Err(resp) = authorize_bus_op(&identity, t, ns, BusOp::ManageSchema) {
                return resp;
            }
        } else if !identity.role.has_permission(Permission::Dispatch) {
            return (
                StatusCode::FORBIDDEN,
                Json(ErrorResponse {
                    error: "insufficient role for bus.schema listing".to_string(),
                }),
            )
                .into_response();
        }
        let gw = state.gateway.read().await;
        let rows = match gw.state_store().scan_keys_by_kind(KeyKind::BusSchema).await {
            Ok(r) => r,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        };
        let mut schemas: Vec<acteon_core::Schema> = rows
            .into_iter()
            .filter_map(|(_, v)| serde_json::from_str::<acteon_core::Schema>(&v).ok())
            .filter(|s| ns_filter.is_none_or(|n| s.namespace == n))
            .filter(|s| t_filter.is_none_or(|t| s.tenant == t))
            .filter(|s| params.subject.as_deref().is_none_or(|sub| s.subject == sub))
            .collect();
        if params.latest_only {
            let mut by_subject: std::collections::HashMap<
                (String, String, String),
                acteon_core::Schema,
            > = std::collections::HashMap::new();
            for s in schemas.drain(..) {
                let key = (s.namespace.clone(), s.tenant.clone(), s.subject.clone());
                match by_subject.get(&key) {
                    Some(existing) if existing.version >= s.version => {}
                    _ => {
                        by_subject.insert(key, s);
                    }
                }
            }
            schemas = by_subject.into_values().collect();
        }
        schemas.sort_by(|a, b| {
            a.namespace
                .cmp(&b.namespace)
                .then(a.tenant.cmp(&b.tenant))
                .then(a.subject.cmp(&b.subject))
                .then(a.version.cmp(&b.version))
        });
        let responses: Vec<SchemaResponse> = schemas.iter().map(schema_to_response).collect();
        let count = responses.len();
        (
            StatusCode::OK,
            Json(ListSchemasResponse {
                schemas: responses,
                count,
            }),
        )
            .into_response()
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, params);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    get,
    path = "/v1/bus/schemas/{namespace}/{tenant}/{subject}",
    tag = "bus",
    params(
        ("namespace" = String, Path, description = "Topic namespace"),
        ("tenant" = String, Path, description = "Tenant ID"),
        ("subject" = String, Path, description = "Schema subject"),
    ),
    responses(
        (status = 200, description = "All versions of the subject", body = ListSchemasResponse),
        (status = 404, description = "No versions registered", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
pub async fn get_subject_versions(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, subject)): Path<(String, String, String)>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        if state.bus_backend.is_none() {
            return service_unavailable("bus feature not enabled");
        }
        if let Err(resp) = authorize_bus_op(&identity, &tenant, &namespace, BusOp::ManageSchema) {
            return resp;
        }
        let gw = state.gateway.read().await;
        let rows = match gw
            .state_store()
            .scan_keys(
                &namespace,
                &tenant,
                KeyKind::BusSchema,
                Some(&format!("{subject}:")),
            )
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        };
        let mut schemas: Vec<acteon_core::Schema> = rows
            .into_iter()
            .filter_map(|(_, v)| serde_json::from_str::<acteon_core::Schema>(&v).ok())
            .filter(|s| s.subject == subject)
            .collect();
        if schemas.is_empty() {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("no versions of subject '{subject}' registered"),
                }),
            )
                .into_response();
        }
        schemas.sort_by_key(|s| s.version);
        let responses: Vec<SchemaResponse> = schemas.iter().map(schema_to_response).collect();
        let count = responses.len();
        (
            StatusCode::OK,
            Json(ListSchemasResponse {
                schemas: responses,
                count,
            }),
        )
            .into_response()
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, namespace, tenant, subject);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    get,
    path = "/v1/bus/schemas/{namespace}/{tenant}/{subject}/{version}",
    tag = "bus",
    params(
        ("namespace" = String, Path, description = "Topic namespace"),
        ("tenant" = String, Path, description = "Tenant ID"),
        ("subject" = String, Path, description = "Schema subject"),
        ("version" = String, Path, description = "Version number or 'latest'"),
    ),
    responses(
        (status = 200, description = "Schema version", body = SchemaResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
pub async fn get_schema_version(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, subject, version)): Path<(String, String, String, String)>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        if state.bus_backend.is_none() {
            return service_unavailable("bus feature not enabled");
        }
        if let Err(resp) = authorize_bus_op(&identity, &tenant, &namespace, BusOp::ManageSchema) {
            return resp;
        }
        let schema =
            match resolve_schema_version(&state, &namespace, &tenant, &subject, &version).await {
                Ok(s) => s,
                Err(resp) => return resp,
            };
        (StatusCode::OK, Json(schema_to_response(&schema))).into_response()
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, namespace, tenant, subject, version);
        service_unavailable("bus feature not compiled")
    }
}

/// Resolve `"latest"` or a numeric version string to a concrete
/// `Schema`. Shared by `get_schema_version` and `bind_topic_schema`.
#[cfg(feature = "bus")]
async fn resolve_schema_version(
    state: &AppState,
    namespace: &str,
    tenant: &str,
    subject: &str,
    version: &str,
) -> Result<acteon_core::Schema, axum::response::Response> {
    let gw = state.gateway.read().await;
    if version.eq_ignore_ascii_case("latest") {
        let rows = gw
            .state_store()
            .scan_keys(
                namespace,
                tenant,
                KeyKind::BusSchema,
                Some(&format!("{subject}:")),
            )
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response()
            })?;
        let latest = rows
            .into_iter()
            .filter_map(|(_, v)| serde_json::from_str::<acteon_core::Schema>(&v).ok())
            .filter(|s| s.subject == subject)
            .max_by_key(|s| s.version);
        latest.ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("no versions of subject '{subject}' registered"),
                }),
            )
                .into_response()
        })
    } else {
        let v: i32 = version.parse().map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("version '{version}' is not an integer or 'latest'"),
                }),
            )
                .into_response()
        })?;
        let key = StateKey::new(
            namespace.to_string(),
            tenant.to_string(),
            KeyKind::BusSchema,
            format!("{subject}:{v}"),
        );
        match gw.state_store().get(&key).await {
            Ok(Some(raw)) => serde_json::from_str::<acteon_core::Schema>(&raw).map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: format!("corrupt schema record for {subject}:{v}: {e}"),
                    }),
                )
                    .into_response()
            }),
            Ok(None) => Err((
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("schema '{subject}' v{v} not found"),
                }),
            )
                .into_response()),
            Err(e) => Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response()),
        }
    }
}

#[utoipa::path(
    delete,
    path = "/v1/bus/schemas/{namespace}/{tenant}/{subject}/{version}",
    tag = "bus",
    params(
        ("namespace" = String, Path),
        ("tenant" = String, Path),
        ("subject" = String, Path),
        ("version" = i32, Path),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 409, description = "Schema is pinned by a topic; unbind first", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
pub async fn delete_schema_version(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, subject, version)): Path<(String, String, String, i32)>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        if state.bus_backend.is_none() {
            return service_unavailable("bus feature not enabled");
        }
        if let Err(resp) = authorize_bus_op(&identity, &tenant, &namespace, BusOp::ManageSchema) {
            return resp;
        }
        // Block the delete if any topic currently pins this version.
        let gw = state.gateway.read().await;
        let topic_rows = match gw
            .state_store()
            .scan_keys(&namespace, &tenant, KeyKind::BusTopic, None)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        };
        let pinned_by: Vec<String> = topic_rows
            .into_iter()
            .filter_map(|(_, v)| serde_json::from_str::<Topic>(&v).ok())
            .filter(|t| {
                t.schema_subject.as_deref() == Some(subject.as_str())
                    && t.schema_version == Some(version)
            })
            .map(|t| t.kafka_topic_name())
            .collect();
        if !pinned_by.is_empty() {
            return (
                StatusCode::CONFLICT,
                Json(ErrorResponse {
                    error: format!(
                        "schema '{subject}' v{version} is pinned by topics: {}; unbind them first",
                        pinned_by.join(", ")
                    ),
                }),
            )
                .into_response();
        }
        let key = StateKey::new(
            namespace.clone(),
            tenant.clone(),
            KeyKind::BusSchema,
            format!("{subject}:{version}"),
        );
        match gw.state_store().get(&key).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse {
                        error: format!("schema '{subject}' v{version} not found"),
                    }),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        }
        if let Err(e) = gw.state_store().delete(&key).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response();
        }
        drop(gw);
        state
            .bus_schema_validator
            .remove(&namespace, &tenant, &subject, version);
        StatusCode::NO_CONTENT.into_response()
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, namespace, tenant, subject, version);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    put,
    path = "/v1/bus/topics/{namespace}/{tenant}/{name}/schema",
    tag = "bus",
    request_body = BindTopicSchemaRequest,
    params(
        ("namespace" = String, Path),
        ("tenant" = String, Path),
        ("name" = String, Path),
    ),
    responses(
        (status = 200, description = "Binding set", body = BindTopicSchemaResponse),
        (status = 404, description = "Topic or schema not found", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
#[allow(clippy::too_many_lines)]
pub async fn bind_topic_schema(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, name)): Path<(String, String, String)>,
    Json(req): Json<BindTopicSchemaRequest>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        if state.bus_backend.is_none() {
            return service_unavailable("bus feature not enabled");
        }
        if let Err(resp) = authorize_bus_op(&identity, &tenant, &namespace, BusOp::ManageSchema) {
            return resp;
        }
        // Resolve the schema first — returns 404 if missing.
        let schema = match resolve_schema_version(
            &state,
            &namespace,
            &tenant,
            &req.subject,
            &req.version.to_string(),
        )
        .await
        {
            Ok(s) => s,
            Err(resp) => return resp,
        };
        let kafka_name = format!("{namespace}.{tenant}.{name}");
        let topic_key = StateKey::new(
            namespace.clone(),
            tenant.clone(),
            KeyKind::BusTopic,
            &kafka_name,
        );
        let gw = state.gateway.read().await;
        let mut topic: Topic = match gw.state_store().get(&topic_key).await {
            Ok(Some(raw)) => match serde_json::from_str::<Topic>(&raw) {
                Ok(t) => t,
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: format!("corrupt topic record for {kafka_name}: {e}"),
                        }),
                    )
                        .into_response();
                }
            },
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse {
                        error: format!("topic {kafka_name} not found"),
                    }),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        };
        topic.schema_subject = Some(schema.subject.clone());
        topic.schema_version = Some(schema.version);
        topic.updated_at = Utc::now();
        let payload = match serde_json::to_string(&topic) {
            Ok(s) => s,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        };
        if let Err(e) = gw.state_store().set(&topic_key, &payload, None).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response();
        }
        drop(gw);
        // Warm the validator cache so the next publish is a hit.
        let _ = state.bus_schema_validator.register(
            &schema.namespace,
            &schema.tenant,
            &schema.subject,
            schema.version,
            &schema.body,
        );
        (
            StatusCode::OK,
            Json(BindTopicSchemaResponse {
                topic: kafka_name,
                subject: schema.subject,
                version: schema.version,
            }),
        )
            .into_response()
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, namespace, tenant, name, req);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    delete,
    path = "/v1/bus/topics/{namespace}/{tenant}/{name}/schema",
    tag = "bus",
    params(
        ("namespace" = String, Path),
        ("tenant" = String, Path),
        ("name" = String, Path),
    ),
    responses(
        (status = 204, description = "Binding removed"),
        (status = 404, description = "Topic not found", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
pub async fn unbind_topic_schema(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, name)): Path<(String, String, String)>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        if state.bus_backend.is_none() {
            return service_unavailable("bus feature not enabled");
        }
        if let Err(resp) = authorize_bus_op(&identity, &tenant, &namespace, BusOp::ManageSchema) {
            return resp;
        }
        let kafka_name = format!("{namespace}.{tenant}.{name}");
        let topic_key = StateKey::new(
            namespace.clone(),
            tenant.clone(),
            KeyKind::BusTopic,
            &kafka_name,
        );
        let gw = state.gateway.read().await;
        let mut topic: Topic = match gw.state_store().get(&topic_key).await {
            Ok(Some(raw)) => match serde_json::from_str::<Topic>(&raw) {
                Ok(t) => t,
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: format!("corrupt topic record for {kafka_name}: {e}"),
                        }),
                    )
                        .into_response();
                }
            },
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse {
                        error: format!("topic {kafka_name} not found"),
                    }),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        };
        topic.schema_subject = None;
        topic.schema_version = None;
        topic.updated_at = Utc::now();
        let payload = match serde_json::to_string(&topic) {
            Ok(s) => s,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        };
        if let Err(e) = gw.state_store().set(&topic_key, &payload, None).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response();
        }
        StatusCode::NO_CONTENT.into_response()
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, namespace, tenant, name);
        service_unavailable("bus feature not compiled")
    }
}

// =============================================================================
// Phase 4: Agent identity + shared inbox + heartbeat
// =============================================================================

/// Body of `POST /v1/bus/agents`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RegisterAgentRequest {
    pub agent_id: String,
    pub namespace: String,
    pub tenant: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Override inbox topic (defaults to
    /// `{namespace}.{tenant}.agents-inbox`).
    #[serde(default)]
    pub inbox_topic: Option<String>,
    #[serde(default)]
    pub heartbeat_ttl_ms: Option<i64>,
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

/// Body of `PUT /v1/bus/agents/{ns}/{t}/{id}`. Only the mutable fields
/// appear here — `agent_id`, `namespace`, `tenant`, `created_at`, and
/// `inbox_topic` are immutable after registration. (Migrating an
/// agent to a new inbox topic mid-flight would orphan in-flight
/// messages on the old topic; delete and re-register if you need a
/// different inbox.)
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateAgentRequest {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub capabilities: Option<Vec<String>>,
    #[serde(default)]
    pub heartbeat_ttl_ms: Option<i64>,
    #[serde(default)]
    pub labels: Option<HashMap<String, String>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AgentResponse {
    pub agent_id: String,
    pub namespace: String,
    pub tenant: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub capabilities: Vec<String>,
    pub inbox_topic: String,
    pub heartbeat_ttl_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    pub status: String,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ListAgentsResponse {
    pub agents: Vec<AgentResponse>,
    pub count: usize,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ListAgentsParams {
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub tenant: Option<String>,
    /// Filter to agents advertising this capability token.
    #[serde(default)]
    pub capability: Option<String>,
    /// Filter by derived status (`online`, `idle`, `dead`, `unknown`).
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HeartbeatResponse {
    pub agent_id: String,
    pub last_heartbeat_at: DateTime<Utc>,
    pub status: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SendToAgentRequest {
    /// Free-form payload.
    #[schema(value_type = Object)]
    pub payload: serde_json::Value,
    /// Optional operator-set headers. `acteon.*` keys are reserved.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SendToAgentResponse {
    pub inbox_topic: String,
    pub agent_id: String,
    pub partition: i32,
    pub offset: i64,
    pub produced_at: DateTime<Utc>,
}

#[cfg(feature = "bus")]
fn agent_to_response(a: &acteon_core::Agent) -> AgentResponse {
    AgentResponse {
        agent_id: a.agent_id.clone(),
        namespace: a.namespace.clone(),
        tenant: a.tenant.clone(),
        display_name: a.display_name.clone(),
        capabilities: a.capabilities.clone(),
        inbox_topic: a.effective_inbox_topic(),
        heartbeat_ttl_ms: a.heartbeat_ttl_ms,
        last_heartbeat_at: a.last_heartbeat_at,
        status: agent_status_str(a.status()),
        labels: a.labels.clone(),
        created_at: a.created_at,
        updated_at: a.updated_at,
    }
}

#[cfg(feature = "bus")]
fn agent_status_str(s: acteon_core::AgentStatus) -> String {
    match s {
        acteon_core::AgentStatus::Online => "online",
        acteon_core::AgentStatus::Idle => "idle",
        acteon_core::AgentStatus::Dead => "dead",
        acteon_core::AgentStatus::Unknown => "unknown",
    }
    .to_string()
}

/// Ensure the shared inbox topic exists in state + Kafka. Called from
/// `register_agent` and `send_to_agent` — first agent to register a
/// `(namespace, tenant)` causes the topic to be auto-created; every
/// subsequent call is a no-op (state lookup short-circuits).
///
/// Returns `Ok(topic_name)` on success. We deliberately `set(&key)`
/// the row unconditionally after creation: a crash between Kafka
/// create and state insert would otherwise leave the inbox topic in
/// Kafka but unregistered in Acteon, which would fail the publish-
/// edge governance check.
#[cfg(feature = "bus")]
async fn ensure_agent_inbox_topic(
    state: &AppState,
    namespace: &str,
    tenant: &str,
    inbox_topic_name: &str,
) -> Result<(), axum::response::Response> {
    // Defense in depth: every caller-facing handler validates
    // `parse_kafka_name` upstream, but if a future caller forgot,
    // the previous `splitn(3,'.').nth(2).unwrap_or(_)` fallback would
    // silently provision a topic under `{ns}.{tenant}.{full-bad-name}`
    // while the producer side used the bad name unchanged — split
    // brain. Reject unparseable names here too so the contract is
    // enforced at the boundary.
    let (parsed_ns, parsed_tenant, leaf_name) =
        parse_kafka_name(inbox_topic_name).map_err(|msg| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("invalid inbox topic name '{inbox_topic_name}': {msg}"),
                }),
            )
                .into_response()
        })?;
    if parsed_ns != namespace || parsed_tenant != tenant {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!(
                    "inbox topic '{inbox_topic_name}' crosses tenants: must be under {namespace}.{tenant}"
                ),
            }),
        )
            .into_response());
    }
    let key = StateKey::new(
        namespace.to_string(),
        tenant.to_string(),
        KeyKind::BusTopic,
        inbox_topic_name,
    );
    let Some(backend) = state.bus_backend.as_ref() else {
        return Err(service_unavailable("bus feature not enabled"));
    };
    // Same lock-amplification fix as `cas_update`: clone the
    // `Arc<dyn StateStore>` once and drop the gateway read guard
    // before any `.await`, so DB roundtrips don't queue-head-block
    // pending gateway writers.
    let store: std::sync::Arc<dyn acteon_state::StateStore> = {
        let gw = state.gateway.read().await;
        gw.state_store().clone()
    };
    if matches!(store.get(&key).await, Ok(Some(_))) {
        return Ok(());
    }
    let topic = Topic::new(leaf_name, namespace, tenant);
    if let Err(e) = topic.validate() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("invalid inbox topic name '{inbox_topic_name}': {e}"),
            }),
        )
            .into_response());
    }
    let body = match serde_json::to_string(&topic) {
        Ok(b) => b,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response());
        }
    };
    if let Err(e) = store.set(&key, &body, None).await {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response());
    }
    if let Err(e) = backend.create_topic(&topic).await {
        // The shared inbox is a tenant-scoped, agent-shared resource.
        // `TopicAlreadyExists` here means another concurrent
        // registration already provisioned it (or it pre-existed in
        // Kafka). Both cases are benign for the agent we're
        // registering — the inbox row in state is still our canonical
        // record, and `send_to_agent` will work either way. Anything
        // else is a real backend failure but we don't fail the
        // registration; the state row remains and operators can
        // reconcile. `&e` keeps the value live for the trace below.
        if !matches!(&e, acteon_bus::BusError::TopicAlreadyExists(_)) {
            tracing::warn!(
                error = %e,
                topic = %inbox_topic_name,
                "auto-create of agent inbox topic returned a non-AlreadyExists error; continuing with state row as canonical",
            );
        }
    }
    Ok(())
}

#[utoipa::path(
    post,
    path = "/v1/bus/agents",
    tag = "bus",
    request_body = RegisterAgentRequest,
    responses(
        (status = 201, description = "Agent registered", body = AgentResponse),
        (status = 400, description = "Invalid agent definition", body = ErrorResponse),
        (status = 409, description = "Agent already exists", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
#[allow(clippy::too_many_lines)]
pub async fn register_agent(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Json(req): Json<RegisterAgentRequest>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        if state.bus_backend.is_none() {
            return service_unavailable("bus feature not enabled");
        }
        if let Err(resp) =
            authorize_bus_op(&identity, &req.tenant, &req.namespace, BusOp::ManageAgent)
        {
            return resp;
        }
        let mut agent = acteon_core::Agent::new(&req.agent_id, &req.namespace, &req.tenant);
        agent.display_name = req.display_name.clone();
        agent.capabilities = req.capabilities.clone();
        agent.inbox_topic = req.inbox_topic.clone();
        if let Some(ttl) = req.heartbeat_ttl_ms {
            agent.heartbeat_ttl_ms = ttl;
        }
        if let Err(resp) = validate_user_labels(&req.labels) {
            return resp;
        }
        agent.labels = req.labels.clone();
        if let Err(e) = agent.validate() {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response();
        }
        let inbox = agent.effective_inbox_topic();
        // Require that a custom inbox belongs to the same tenant. Same
        // check we enforce on subscriptions so an agent can't hijack
        // another tenant's topic. Fail-closed: an unparseable
        // override is rejected — the previous `if let Ok && ...` form
        // silently accepted malformed names that bypassed the tenant
        // check entirely (Phase 5 review found the same shape there).
        if let Some(override_topic) = req.inbox_topic.as_deref() {
            match parse_kafka_name(override_topic) {
                Ok((topic_ns, topic_t, _)) => {
                    if topic_ns != agent.namespace || topic_t != agent.tenant {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(ErrorResponse {
                                error: format!(
                                    "inbox topic {override_topic} crosses tenants: must be under {}.{}",
                                    agent.namespace, agent.tenant
                                ),
                            }),
                        )
                            .into_response();
                    }
                }
                Err(msg) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse {
                            error: format!("inbox topic '{override_topic}' is invalid: {msg}"),
                        }),
                    )
                        .into_response();
                }
            }
        }
        let key = StateKey::new(
            agent.namespace.clone(),
            agent.tenant.clone(),
            KeyKind::BusAgent,
            agent.id(),
        );
        // Provision the shared inbox topic before reserving the agent
        // id. Inbox provisioning is idempotent across concurrent
        // registrations (any `TopicAlreadyExists` is benign — see
        // `ensure_agent_inbox_topic`); the atomic agent reservation
        // below is what guarantees only one caller wins for a given
        // `agent_id`.
        if let Err(resp) =
            ensure_agent_inbox_topic(&state, &agent.namespace, &agent.tenant, &inbox).await
        {
            return resp;
        }
        let payload = match serde_json::to_string(&agent) {
            Ok(s) => s,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        };
        let gw = state.gateway.read().await;
        match gw.state_store().check_and_set(&key, &payload, None).await {
            Ok(true) => {}
            Ok(false) => {
                return (
                    StatusCode::CONFLICT,
                    Json(ErrorResponse {
                        error: format!(
                            "agent {}.{}.{} already registered",
                            agent.namespace, agent.tenant, agent.agent_id
                        ),
                    }),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        }
        (StatusCode::CREATED, Json(agent_to_response(&agent))).into_response()
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, req);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    get,
    path = "/v1/bus/agents",
    tag = "bus",
    params(ListAgentsParams),
    responses(
        (status = 200, description = "Agent list", body = ListAgentsResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
pub async fn list_agents(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Query(params): Query<ListAgentsParams>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        if state.bus_backend.is_none() {
            return service_unavailable("bus feature not enabled");
        }
        let gw = state.gateway.read().await;
        let rows = match gw.state_store().scan_keys_by_kind(KeyKind::BusAgent).await {
            Ok(r) => r,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        };
        let agents: Vec<acteon_core::Agent> = rows
            .into_iter()
            .filter_map(|(_, v)| serde_json::from_str::<acteon_core::Agent>(&v).ok())
            .filter(|a| params.namespace.as_deref().is_none_or(|n| a.namespace == n))
            .filter(|a| params.tenant.as_deref().is_none_or(|t| a.tenant == t))
            .filter(|a| {
                params
                    .capability
                    .as_deref()
                    .is_none_or(|c| a.capabilities.iter().any(|cap| cap == c))
            })
            .filter(|a| identity.is_authorized(&a.tenant, &a.namespace, "bus", "agent"))
            .filter(|a| {
                params
                    .status
                    .as_deref()
                    .is_none_or(|s| agent_status_str(a.status()).eq_ignore_ascii_case(s))
            })
            .collect();
        let responses: Vec<AgentResponse> = agents.iter().map(agent_to_response).collect();
        let count = responses.len();
        (
            StatusCode::OK,
            Json(ListAgentsResponse {
                agents: responses,
                count,
            }),
        )
            .into_response()
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, params);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    get,
    path = "/v1/bus/agents/{namespace}/{tenant}/{agent_id}",
    tag = "bus",
    params(
        ("namespace" = String, Path),
        ("tenant" = String, Path),
        ("agent_id" = String, Path),
    ),
    responses(
        (status = 200, description = "Agent detail", body = AgentResponse),
        (status = 404, description = "Agent not found", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
pub async fn get_agent(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, agent_id)): Path<(String, String, String)>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        if state.bus_backend.is_none() {
            return service_unavailable("bus feature not enabled");
        }
        if let Err(resp) = authorize_bus_op(&identity, &tenant, &namespace, BusOp::ManageAgent) {
            return resp;
        }
        match load_agent(&state, &namespace, &tenant, &agent_id).await {
            Ok(a) => (StatusCode::OK, Json(agent_to_response(&a))).into_response(),
            Err(resp) => resp,
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, namespace, tenant, agent_id);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    put,
    path = "/v1/bus/agents/{namespace}/{tenant}/{agent_id}",
    tag = "bus",
    request_body = UpdateAgentRequest,
    params(
        ("namespace" = String, Path),
        ("tenant" = String, Path),
        ("agent_id" = String, Path),
    ),
    responses(
        (status = 200, description = "Agent updated", body = AgentResponse),
        (status = 400, description = "Invalid update", body = ErrorResponse),
        (status = 404, description = "Agent not found", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
pub async fn update_agent(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, agent_id)): Path<(String, String, String)>,
    Json(req): Json<UpdateAgentRequest>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        if state.bus_backend.is_none() {
            return service_unavailable("bus feature not enabled");
        }
        if let Err(resp) = authorize_bus_op(&identity, &tenant, &namespace, BusOp::ManageAgent) {
            return resp;
        }
        let key = StateKey::new(
            namespace.clone(),
            tenant.clone(),
            KeyKind::BusAgent,
            &agent_id,
        );
        let missing = format!("agent {namespace}.{tenant}.{agent_id} not found");
        if let Some(labels) = &req.labels
            && let Err(resp) = validate_user_labels(labels)
        {
            return resp;
        }
        let req_display = req.display_name.clone();
        let req_caps = req.capabilities.clone();
        let req_ttl = req.heartbeat_ttl_ms;
        let req_labels = req.labels.clone();
        let result = cas_update::<acteon_core::Agent, _>(&state, &key, &missing, |agent| {
            if let Some(d) = req_display.clone() {
                agent.display_name = Some(d);
            }
            if let Some(c) = req_caps.clone() {
                agent.capabilities = c;
            }
            if let Some(ttl) = req_ttl {
                agent.heartbeat_ttl_ms = ttl;
            }
            if let Some(l) = req_labels.clone() {
                agent.labels = l;
            }
            agent.updated_at = Utc::now();
            agent.validate().map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response()
            })
        })
        .await;
        match result {
            Ok(agent) => (StatusCode::OK, Json(agent_to_response(&agent))).into_response(),
            Err(resp) => resp,
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, namespace, tenant, agent_id, req);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    delete,
    path = "/v1/bus/agents/{namespace}/{tenant}/{agent_id}",
    tag = "bus",
    params(
        ("namespace" = String, Path),
        ("tenant" = String, Path),
        ("agent_id" = String, Path),
    ),
    responses(
        (status = 204, description = "Agent deleted"),
        (status = 404, description = "Agent not found", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
pub async fn delete_agent(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, agent_id)): Path<(String, String, String)>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        if state.bus_backend.is_none() {
            return service_unavailable("bus feature not enabled");
        }
        if let Err(resp) = authorize_bus_op(&identity, &tenant, &namespace, BusOp::ManageAgent) {
            return resp;
        }
        let key = StateKey::new(
            namespace.clone(),
            tenant.clone(),
            KeyKind::BusAgent,
            &agent_id,
        );
        let gw = state.gateway.read().await;
        match gw.state_store().get(&key).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse {
                        error: format!("agent {namespace}.{tenant}.{agent_id} not found"),
                    }),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        }
        if let Err(e) = gw.state_store().delete(&key).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response();
        }
        StatusCode::NO_CONTENT.into_response()
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, namespace, tenant, agent_id);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    post,
    path = "/v1/bus/agents/{namespace}/{tenant}/{agent_id}/heartbeat",
    tag = "bus",
    params(
        ("namespace" = String, Path),
        ("tenant" = String, Path),
        ("agent_id" = String, Path),
    ),
    responses(
        (status = 200, description = "Heartbeat recorded", body = HeartbeatResponse),
        (status = 404, description = "Agent not found", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
pub async fn heartbeat_agent(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, agent_id)): Path<(String, String, String)>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        if state.bus_backend.is_none() {
            return service_unavailable("bus feature not enabled");
        }
        if let Err(resp) = authorize_bus_op(&identity, &tenant, &namespace, BusOp::ManageAgent) {
            return resp;
        }
        let key = StateKey::new(
            namespace.clone(),
            tenant.clone(),
            KeyKind::BusAgent,
            &agent_id,
        );
        let missing = format!("agent {namespace}.{tenant}.{agent_id} not found");
        let result = cas_update::<acteon_core::Agent, _>(&state, &key, &missing, |agent| {
            let now = Utc::now();
            agent.last_heartbeat_at = Some(now);
            agent.updated_at = now;
            Ok(())
        })
        .await;
        match result {
            Ok(agent) => {
                let now = agent.last_heartbeat_at.unwrap_or_else(Utc::now);
                (
                    StatusCode::OK,
                    Json(HeartbeatResponse {
                        agent_id: agent.agent_id.clone(),
                        last_heartbeat_at: now,
                        status: agent_status_str(agent.status_at(now)),
                    }),
                )
                    .into_response()
            }
            Err(resp) => resp,
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, namespace, tenant, agent_id);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    post,
    path = "/v1/bus/agents/{namespace}/{tenant}/{agent_id}/send",
    tag = "bus",
    request_body = SendToAgentRequest,
    params(
        ("namespace" = String, Path),
        ("tenant" = String, Path),
        ("agent_id" = String, Path),
    ),
    responses(
        (status = 200, description = "Message delivered to inbox", body = SendToAgentResponse),
        (status = 400, description = "Reserved header or invalid payload", body = ErrorResponse),
        (status = 404, description = "Agent not found", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
pub async fn send_to_agent(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, agent_id)): Path<(String, String, String)>,
    Json(req): Json<SendToAgentRequest>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        let Some(backend) = state.bus_backend.as_ref() else {
            return service_unavailable("bus feature not enabled");
        };
        // Two-level auth: caller must hold ManageAgent (so random
        // publishers can't target arbitrary agents) and Publish on the
        // inbox topic's tenant (reusing the same gate as direct
        // `/v1/bus/publish`).
        if let Err(resp) = authorize_bus_op(&identity, &tenant, &namespace, BusOp::ManageAgent) {
            return resp;
        }
        if let Err(resp) = authorize_bus_op(&identity, &tenant, &namespace, BusOp::Publish) {
            return resp;
        }
        if let Err(resp) = validate_user_headers(&req.headers) {
            return resp;
        }
        if let Some(reserved) = req.headers.keys().find(|k| k.starts_with("acteon.")) {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!(
                        "header '{reserved}' uses the reserved 'acteon.' prefix; those are set by the server"
                    ),
                }),
            )
                .into_response();
        }
        let agent = match load_agent(&state, &namespace, &tenant, &agent_id).await {
            Ok(a) => a,
            Err(resp) => return resp,
        };
        let inbox = agent.effective_inbox_topic();
        // Defensive: confirm the inbox topic is still registered. In
        // normal flow `register_agent` guarantees this, but operators
        // can delete the topic out from under the agent and we want a
        // clean 404 rather than a downstream Kafka error.
        let inbox_key = StateKey::new(namespace.clone(), tenant.clone(), KeyKind::BusTopic, &inbox);
        {
            let gw = state.gateway.read().await;
            match gw.state_store().get(&inbox_key).await {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return (
                        StatusCode::NOT_FOUND,
                        Json(ErrorResponse {
                            error: format!(
                                "agent inbox topic {inbox} is not registered; re-register the agent or create the topic"
                            ),
                        }),
                    )
                        .into_response();
                }
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: e.to_string(),
                        }),
                    )
                        .into_response();
                }
            }
        }
        // Key by agent_id so Kafka's partitioner routes all messages
        // for a given agent to a stable partition — gives us per-agent
        // FIFO without any extra subscription machinery.
        let mut msg = acteon_bus::BusMessage::new(inbox.clone(), req.payload.clone())
            .with_key(&agent.agent_id);
        for (k, v) in &req.headers {
            msg = msg.with_header(k.clone(), v.clone());
        }
        // Stamp the recipient so subscribers can route locally without
        // parsing the payload. `with_header` silently drops reserved
        // `acteon.*` keys — the server inserts them directly.
        msg.headers
            .insert("acteon.agent.id".into(), agent.agent_id.clone());
        match backend.produce(msg).await {
            Ok(receipt) => (
                StatusCode::OK,
                Json(SendToAgentResponse {
                    inbox_topic: receipt.topic,
                    agent_id: agent.agent_id,
                    partition: receipt.partition,
                    offset: receipt.offset,
                    produced_at: receipt.timestamp,
                }),
            )
                .into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response(),
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, namespace, tenant, agent_id, req);
        service_unavailable("bus feature not compiled")
    }
}

/// Direct `StateKey` lookup of an agent record.
#[cfg(feature = "bus")]
async fn load_agent(
    state: &AppState,
    namespace: &str,
    tenant: &str,
    agent_id: &str,
) -> Result<acteon_core::Agent, axum::response::Response> {
    let key = StateKey::new(
        namespace.to_string(),
        tenant.to_string(),
        KeyKind::BusAgent,
        agent_id.to_string(),
    );
    let gw = state.gateway.read().await;
    match gw.state_store().get(&key).await {
        Ok(Some(raw)) => serde_json::from_str::<acteon_core::Agent>(&raw).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("corrupt agent record for {namespace}.{tenant}.{agent_id}: {e}"),
                }),
            )
                .into_response()
        }),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("agent {namespace}.{tenant}.{agent_id} not found"),
            }),
        )
            .into_response()),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response()),
    }
}

// =============================================================================
// Phase 5: Conversations — multi-agent threads on a shared events topic
// =============================================================================

/// Body of `POST /v1/bus/conversations`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateConversationRequest {
    pub conversation_id: String,
    pub namespace: String,
    pub tenant: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub participants: Vec<String>,
    /// Override the events topic this conversation produces to.
    /// Defaults to `{ns}.{tenant}.conversations-events`.
    #[serde(default)]
    pub events_topic: Option<String>,
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

/// Body of `PUT /v1/bus/conversations/{ns}/{t}/{id}`. Mutable fields
/// only. `id`, `namespace`, `tenant`, `created_at`, `events_topic`,
/// and `state` are immutable from this endpoint — state changes go
/// through `/transition`, and the events topic is set once at
/// registration time. Mid-flight events-topic swaps would orphan
/// in-flight messages on the old topic and have been a topic-
/// injection vector when the validation was incomplete; closed by
/// removing the field.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateConversationRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub participants: Option<Vec<String>>,
    #[serde(default)]
    pub labels: Option<HashMap<String, String>>,
}

/// Body of `POST /v1/bus/conversations/{ns}/{t}/{id}/transition`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ConversationTransitionRequest {
    pub transition: acteon_core::ConversationTransition,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ConversationResponse {
    pub conversation_id: String,
    pub namespace: String,
    pub tenant: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub state: acteon_core::ConversationState,
    pub participants: Vec<String>,
    pub events_topic: String,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ListConversationsResponse {
    pub conversations: Vec<ConversationResponse>,
    pub count: usize,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ListConversationsParams {
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub tenant: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    /// Filter conversations whose participant list contains this `agent_id`.
    #[serde(default)]
    pub participant: Option<String>,
    /// Hard cap on returned rows. Default 100, max 500. The current
    /// implementation scans every row of `KeyKind::BusConversation`
    /// and filters in memory, so this bounds the response payload
    /// while a future state-store cursor primitive is added for true
    /// pagination at scale.
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AppendConversationMessageRequest {
    /// Free-form payload — the same shape as `/v1/bus/publish`.
    #[schema(value_type = Object)]
    pub payload: serde_json::Value,
    /// Optional sender (an `agent_id`). Stamped as
    /// `acteon.conversation.sender` header so subscribers can route
    /// locally without parsing the payload. Server-validated against
    /// the conversation's participant list when one is configured.
    #[serde(default)]
    pub sender: Option<String>,
    /// Operator-supplied headers; `acteon.*` prefix is reserved.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AppendConversationMessageResponse {
    pub events_topic: String,
    pub conversation_id: String,
    pub partition: i32,
    pub offset: i64,
    pub produced_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ReplayConversationParams {
    /// `earliest` (default — full thread) or `latest` (only new
    /// messages, useful as a probe). Ignored when `cursor` is set.
    #[serde(default)]
    pub from: Option<String>,
    /// Hard cap on returned messages. Default 200, max 1000.
    #[serde(default)]
    pub limit: Option<usize>,
    /// How long to wait for messages before returning a partial result.
    /// Default 1500ms; bounds replay latency on quiet topics.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Resume token from a previous response's `cursor`. When set,
    /// `from` is ignored and the scan picks up at the per-partition
    /// offsets the previous call left off at.
    #[serde(default)]
    pub cursor: Option<String>,
    /// `agent_id` the caller is acting as. Required when the
    /// conversation has a non-empty `participants` ACL — must be
    /// on the list. Same V1 read-isolation model as
    /// [`ToolResultLookupParams::as_agent`].
    #[serde(default)]
    pub as_agent: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReplayMessageEntry {
    pub partition: i32,
    pub offset: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[schema(value_type = Object)]
    pub payload: serde_json::Value,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    pub timestamp: DateTime<Utc>,
}

/// Why the replay loop terminated. Distinguishes end-of-stream
/// (operator caller is done) from a partial result (operator must
/// paginate via `cursor`). The previous `limit_reached: bool` shape
/// silently lied on timeout: a timeout-bounded scan that didn't hit
/// the count limit reported `false`, suggesting the thread was
/// drained when in fact the server just gave up early. Now every
/// non-`Complete` reason carries an explicit cursor.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReplayExitReason {
    /// The scan reached or surpassed every partition's high-water
    /// mark **as snapshotted at the moment the request started**.
    /// The caller has every message that existed when they asked.
    ///
    /// **Snapshot semantics**: new messages produced *after* the
    /// scan began are *not* part of this response. For chat-style
    /// usage where the client wants to keep tailing, treat
    /// `Complete` as "I've drained the snapshot — call me again
    /// (with the returned `cursor`, which still encodes the per-
    /// partition tail) for any new messages." Without this caveat,
    /// a high-volume conversation can race ahead of the scan and
    /// produce messages that the next replay starting fresh would
    /// have to re-discover.
    Complete,
    /// Hit the per-request `limit` before scanning the whole topic.
    /// More messages may exist; resume with `cursor`.
    Limit,
    /// Hit the `timeout_ms` budget before exhausting partitions or
    /// the limit. More messages may exist; resume with `cursor`.
    Timeout,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReplayConversationResponse {
    pub conversation_id: String,
    pub events_topic: String,
    pub messages: Vec<ReplayMessageEntry>,
    pub exit_reason: ReplayExitReason,
    /// Opaque resume token, always present. On `Limit` and
    /// `Timeout`, pass it back as `cursor` to keep paginating the
    /// remainder of the snapshot. On `Complete`, the cursor encodes
    /// the per-partition tail position at scan end — tailing clients
    /// pass it back to a follow-up request to pick up messages
    /// produced after this snapshot. Clients doing a one-shot
    /// drain ignore the cursor when `exit_reason == Complete`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[cfg(feature = "bus")]
fn conversation_to_response(c: &acteon_core::Conversation) -> ConversationResponse {
    ConversationResponse {
        conversation_id: c.conversation_id.clone(),
        namespace: c.namespace.clone(),
        tenant: c.tenant.clone(),
        title: c.title.clone(),
        state: c.state,
        participants: c.participants.clone(),
        events_topic: c.effective_events_topic(),
        labels: c.labels.clone(),
        created_at: c.created_at,
        updated_at: c.updated_at,
    }
}

#[cfg(feature = "bus")]
fn conversation_state_str(s: acteon_core::ConversationState) -> String {
    match s {
        acteon_core::ConversationState::Active => "active",
        acteon_core::ConversationState::Resolved => "resolved",
        acteon_core::ConversationState::Archived => "archived",
    }
    .to_string()
}

/// Ensure the shared events topic exists in state + Kafka. Same idea
/// as `ensure_agent_inbox_topic` — first conversation in a tenant
/// provisions the topic, subsequent registrations are no-ops.
#[cfg(feature = "bus")]
async fn ensure_conversation_events_topic(
    state: &AppState,
    namespace: &str,
    tenant: &str,
    events_topic_name: &str,
) -> Result<(), axum::response::Response> {
    // Defense in depth: parse_kafka_name first so an unparseable name
    // can never reach Kafka. Mirrors the agent-inbox check.
    let (parsed_ns, parsed_tenant, leaf_name) =
        parse_kafka_name(events_topic_name).map_err(|msg| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("invalid events topic name '{events_topic_name}': {msg}"),
                }),
            )
                .into_response()
        })?;
    if parsed_ns != namespace || parsed_tenant != tenant {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!(
                    "events topic '{events_topic_name}' crosses tenants: must be under {namespace}.{tenant}"
                ),
            }),
        )
            .into_response());
    }
    let key = StateKey::new(
        namespace.to_string(),
        tenant.to_string(),
        KeyKind::BusTopic,
        events_topic_name,
    );
    let Some(backend) = state.bus_backend.as_ref() else {
        return Err(service_unavailable("bus feature not enabled"));
    };
    // Same lock-amplification fix as `cas_update`: clone the
    // `Arc<dyn StateStore>` once and drop the gateway read guard
    // before any `.await`, so DB roundtrips don't queue-head-block
    // pending gateway writers.
    let store: std::sync::Arc<dyn acteon_state::StateStore> = {
        let gw = state.gateway.read().await;
        gw.state_store().clone()
    };
    if matches!(store.get(&key).await, Ok(Some(_))) {
        return Ok(());
    }
    let topic = Topic::new(leaf_name, namespace, tenant);
    if let Err(e) = topic.validate() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("invalid events topic name '{events_topic_name}': {e}"),
            }),
        )
            .into_response());
    }
    let body = match serde_json::to_string(&topic) {
        Ok(b) => b,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response());
        }
    };
    // Use check_and_set so two concurrent provision attempts can't
    // both write the topic row. The loser silently no-ops.
    if let Err(e) = store.check_and_set(&key, &body, None).await {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response());
    }
    if let Err(e) = backend.create_topic(&topic).await
        && !matches!(&e, acteon_bus::BusError::TopicAlreadyExists(_))
    {
        tracing::warn!(error = %e, topic = %events_topic_name, "auto-create of conversation events topic returned a non-AlreadyExists error; continuing with state row as canonical");
    }
    Ok(())
}

#[utoipa::path(
    post,
    path = "/v1/bus/conversations",
    tag = "bus",
    request_body = CreateConversationRequest,
    responses(
        (status = 201, description = "Conversation registered", body = ConversationResponse),
        (status = 400, description = "Invalid conversation definition", body = ErrorResponse),
        (status = 409, description = "Conversation already exists", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
#[allow(clippy::too_many_lines)]
pub async fn register_conversation(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Json(req): Json<CreateConversationRequest>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        if state.bus_backend.is_none() {
            return service_unavailable("bus feature not enabled");
        }
        if let Err(resp) = authorize_bus_op(
            &identity,
            &req.tenant,
            &req.namespace,
            BusOp::ManageConversation,
        ) {
            return resp;
        }
        if let Err(resp) = validate_user_labels(&req.labels) {
            return resp;
        }
        let mut conv =
            acteon_core::Conversation::new(&req.conversation_id, &req.namespace, &req.tenant);
        conv.title = req.title.clone();
        conv.participants = req.participants.clone();
        conv.events_topic = req.events_topic.clone();
        conv.labels = req.labels.clone();
        if let Err(e) = conv.validate() {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response();
        }
        // Validate any caller-supplied `events_topic` override before
        // we touch state or Kafka. Fail-closed: an unparseable name
        // (zero or too many dots, no namespace.tenant prefix) MUST
        // be rejected. The earlier `if let Ok(...) && ...` form
        // silently *accepted* unparseable names, which let a tenant
        // produce to arbitrary topics — fixed here.
        if let Some(override_topic) = req.events_topic.as_deref() {
            match parse_kafka_name(override_topic) {
                Ok((topic_ns, topic_t, _)) => {
                    if topic_ns != conv.namespace || topic_t != conv.tenant {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(ErrorResponse {
                                error: format!(
                                    "events topic {override_topic} crosses tenants: must be under {}.{}",
                                    conv.namespace, conv.tenant
                                ),
                            }),
                        )
                            .into_response();
                    }
                }
                Err(msg) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse {
                            error: format!("events topic '{override_topic}' is invalid: {msg}"),
                        }),
                    )
                        .into_response();
                }
            }
        }
        let events_topic = conv.effective_events_topic();
        if let Err(resp) =
            ensure_conversation_events_topic(&state, &conv.namespace, &conv.tenant, &events_topic)
                .await
        {
            return resp;
        }
        let key = StateKey::new(
            conv.namespace.clone(),
            conv.tenant.clone(),
            KeyKind::BusConversation,
            conv.id(),
        );
        let payload = match serde_json::to_string(&conv) {
            Ok(s) => s,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        };
        let gw = state.gateway.read().await;
        match gw.state_store().check_and_set(&key, &payload, None).await {
            Ok(true) => {}
            Ok(false) => {
                return (
                    StatusCode::CONFLICT,
                    Json(ErrorResponse {
                        error: format!(
                            "conversation {}.{}.{} already registered",
                            conv.namespace, conv.tenant, conv.conversation_id
                        ),
                    }),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        }
        (StatusCode::CREATED, Json(conversation_to_response(&conv))).into_response()
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, req);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    get,
    path = "/v1/bus/conversations",
    tag = "bus",
    params(ListConversationsParams),
    responses(
        (status = 200, description = "Conversation list", body = ListConversationsResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
pub async fn list_conversations(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Query(params): Query<ListConversationsParams>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        if state.bus_backend.is_none() {
            return service_unavailable("bus feature not enabled");
        }
        let gw = state.gateway.read().await;
        let rows = match gw
            .state_store()
            .scan_keys_by_kind(KeyKind::BusConversation)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        };
        // Bound the response. `scan_keys_by_kind` still pulls every
        // row from the state store for filtering; a future
        // state-store cursor primitive will let us push the bound
        // down to the backend. Until then, the `take(limit)` here
        // protects the response payload and serialization cost from
        // OOM at scale.
        let limit = params.limit.unwrap_or(100).min(500);
        let convs: Vec<acteon_core::Conversation> = rows
            .into_iter()
            .filter_map(|(_, v)| serde_json::from_str::<acteon_core::Conversation>(&v).ok())
            .filter(|c| params.namespace.as_deref().is_none_or(|n| c.namespace == n))
            .filter(|c| params.tenant.as_deref().is_none_or(|t| c.tenant == t))
            .filter(|c| {
                params
                    .state
                    .as_deref()
                    .is_none_or(|s| conversation_state_str(c.state).eq_ignore_ascii_case(s))
            })
            .filter(|c| {
                params
                    .participant
                    .as_deref()
                    .is_none_or(|p| c.participants.iter().any(|x| x == p))
            })
            .filter(|c| identity.is_authorized(&c.tenant, &c.namespace, "bus", "conversation"))
            .take(limit)
            .collect();
        let responses: Vec<ConversationResponse> =
            convs.iter().map(conversation_to_response).collect();
        let count = responses.len();
        (
            StatusCode::OK,
            Json(ListConversationsResponse {
                conversations: responses,
                count,
            }),
        )
            .into_response()
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, params);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    get,
    path = "/v1/bus/conversations/{namespace}/{tenant}/{conversation_id}",
    tag = "bus",
    params(
        ("namespace" = String, Path),
        ("tenant" = String, Path),
        ("conversation_id" = String, Path),
    ),
    responses(
        (status = 200, description = "Conversation detail", body = ConversationResponse),
        (status = 404, description = "Conversation not found", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
pub async fn get_conversation(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, conversation_id)): Path<(String, String, String)>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        if state.bus_backend.is_none() {
            return service_unavailable("bus feature not enabled");
        }
        if let Err(resp) =
            authorize_bus_op(&identity, &tenant, &namespace, BusOp::ManageConversation)
        {
            return resp;
        }
        match load_conversation(&state, &namespace, &tenant, &conversation_id).await {
            Ok(c) => (StatusCode::OK, Json(conversation_to_response(&c))).into_response(),
            Err(resp) => resp,
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, namespace, tenant, conversation_id);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    put,
    path = "/v1/bus/conversations/{namespace}/{tenant}/{conversation_id}",
    tag = "bus",
    request_body = UpdateConversationRequest,
    params(
        ("namespace" = String, Path),
        ("tenant" = String, Path),
        ("conversation_id" = String, Path),
    ),
    responses(
        (status = 200, description = "Conversation updated", body = ConversationResponse),
        (status = 400, description = "Invalid update", body = ErrorResponse),
        (status = 404, description = "Conversation not found", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
pub async fn update_conversation(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, conversation_id)): Path<(String, String, String)>,
    Json(req): Json<UpdateConversationRequest>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        if state.bus_backend.is_none() {
            return service_unavailable("bus feature not enabled");
        }
        if let Err(resp) =
            authorize_bus_op(&identity, &tenant, &namespace, BusOp::ManageConversation)
        {
            return resp;
        }
        let key = StateKey::new(
            namespace.clone(),
            tenant.clone(),
            KeyKind::BusConversation,
            &conversation_id,
        );
        let missing = format!("conversation {namespace}.{tenant}.{conversation_id} not found");
        if let Some(labels) = &req.labels
            && let Err(resp) = validate_user_labels(labels)
        {
            return resp;
        }
        let req_title = req.title.clone();
        let req_participants = req.participants.clone();
        let req_labels = req.labels.clone();
        let result = cas_update::<acteon_core::Conversation, _>(&state, &key, &missing, |conv| {
            // Archived threads are immutable. Allowing edits to title /
            // participants / labels after archive would undermine the
            // audit trail (the participant list is the ACL gate at
            // append time; rewriting it post-archive would let an
            // operator silently change who *appeared* to have access
            // to a closed thread). Operators who need a different
            // shape should reopen via /transition first.
            if !conv.accepts_messages() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: format!(
                            "conversation {} is archived; reopen via /transition before updating",
                            conv.conversation_id
                        ),
                    }),
                )
                    .into_response());
            }
            if let Some(t) = req_title.clone() {
                conv.title = Some(t);
            }
            if let Some(p) = req_participants.clone() {
                conv.participants = p;
            }
            if let Some(l) = req_labels.clone() {
                conv.labels = l;
            }
            conv.updated_at = Utc::now();
            conv.validate().map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response()
            })
        })
        .await;
        match result {
            Ok(conv) => (StatusCode::OK, Json(conversation_to_response(&conv))).into_response(),
            Err(resp) => resp,
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, namespace, tenant, conversation_id, req);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    delete,
    path = "/v1/bus/conversations/{namespace}/{tenant}/{conversation_id}",
    tag = "bus",
    params(
        ("namespace" = String, Path),
        ("tenant" = String, Path),
        ("conversation_id" = String, Path),
    ),
    responses(
        (status = 204, description = "Conversation deleted"),
        (status = 404, description = "Conversation not found", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
pub async fn delete_conversation(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, conversation_id)): Path<(String, String, String)>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        if state.bus_backend.is_none() {
            return service_unavailable("bus feature not enabled");
        }
        if let Err(resp) =
            authorize_bus_op(&identity, &tenant, &namespace, BusOp::ManageConversation)
        {
            return resp;
        }
        let key = StateKey::new(
            namespace.clone(),
            tenant.clone(),
            KeyKind::BusConversation,
            &conversation_id,
        );
        let gw = state.gateway.read().await;
        match gw.state_store().get(&key).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse {
                        error: format!(
                            "conversation {namespace}.{tenant}.{conversation_id} not found"
                        ),
                    }),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        }
        if let Err(e) = gw.state_store().delete(&key).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response();
        }
        StatusCode::NO_CONTENT.into_response()
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, namespace, tenant, conversation_id);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    post,
    path = "/v1/bus/conversations/{namespace}/{tenant}/{conversation_id}/transition",
    tag = "bus",
    request_body = ConversationTransitionRequest,
    params(
        ("namespace" = String, Path),
        ("tenant" = String, Path),
        ("conversation_id" = String, Path),
    ),
    responses(
        (status = 200, description = "Transition applied", body = ConversationResponse),
        (status = 404, description = "Conversation not found", body = ErrorResponse),
        (status = 409, description = "Illegal transition for current state", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
pub async fn transition_conversation(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, conversation_id)): Path<(String, String, String)>,
    Json(req): Json<ConversationTransitionRequest>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        if state.bus_backend.is_none() {
            return service_unavailable("bus feature not enabled");
        }
        if let Err(resp) =
            authorize_bus_op(&identity, &tenant, &namespace, BusOp::ManageConversation)
        {
            return resp;
        }
        let key = StateKey::new(
            namespace.clone(),
            tenant.clone(),
            KeyKind::BusConversation,
            &conversation_id,
        );
        let missing = format!("conversation {namespace}.{tenant}.{conversation_id} not found");
        let transition = req.transition;
        let result = cas_update::<acteon_core::Conversation, _>(&state, &key, &missing, |conv| {
            conv.apply_transition(transition).map(|_| ()).map_err(|e| {
                (
                    StatusCode::CONFLICT,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response()
            })
        })
        .await;
        match result {
            Ok(conv) => (StatusCode::OK, Json(conversation_to_response(&conv))).into_response(),
            Err(resp) => resp,
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, namespace, tenant, conversation_id, req);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    post,
    path = "/v1/bus/conversations/{namespace}/{tenant}/{conversation_id}/messages",
    tag = "bus",
    request_body = AppendConversationMessageRequest,
    params(
        ("namespace" = String, Path),
        ("tenant" = String, Path),
        ("conversation_id" = String, Path),
    ),
    responses(
        (status = 200, description = "Message appended", body = AppendConversationMessageResponse),
        (status = 400, description = "Reserved header, invalid sender, or archived conversation", body = ErrorResponse),
        (status = 404, description = "Conversation not found", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
#[allow(clippy::too_many_lines)]
pub async fn append_conversation_message(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, conversation_id)): Path<(String, String, String)>,
    Json(req): Json<AppendConversationMessageRequest>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        let Some(backend) = state.bus_backend.as_ref() else {
            return service_unavailable("bus feature not enabled");
        };
        if let Err(resp) =
            authorize_bus_op(&identity, &tenant, &namespace, BusOp::ManageConversation)
        {
            return resp;
        }
        if let Err(resp) = authorize_bus_op(&identity, &tenant, &namespace, BusOp::Publish) {
            return resp;
        }
        if let Err(resp) = validate_user_headers(&req.headers) {
            return resp;
        }
        if let Some(reserved) = req.headers.keys().find(|k| k.starts_with("acteon.")) {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!(
                        "header '{reserved}' uses the reserved 'acteon.' prefix; those are set by the server"
                    ),
                }),
            )
                .into_response();
        }
        let conv = match load_conversation(&state, &namespace, &tenant, &conversation_id).await {
            Ok(c) => c,
            Err(resp) => return resp,
        };
        if !conv.accepts_messages() {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!(
                        "conversation {}.{}.{} is archived; reopen via /transition to post",
                        conv.namespace, conv.tenant, conv.conversation_id
                    ),
                }),
            )
                .into_response();
        }
        // Participant ACL: when participants is non-empty, the sender
        // must be present and listed. The earlier `if let Some(sender)`
        // form silently allowed anonymous posts (sender = None) on
        // restricted threads, defeating the gate entirely.
        if !conv.participants.is_empty() {
            match req.sender.as_deref() {
                Some(s) if conv.participants.iter().any(|p| p == s) => {}
                Some(s) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse {
                            error: format!(
                                "sender '{s}' is not a participant of conversation {}",
                                conv.conversation_id
                            ),
                        }),
                    )
                        .into_response();
                }
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse {
                            error: format!(
                                "conversation {} has a participant ACL; `sender` is required",
                                conv.conversation_id
                            ),
                        }),
                    )
                        .into_response();
                }
            }
        }
        let events_topic = conv.effective_events_topic();
        let topic_key = StateKey::new(
            namespace.clone(),
            tenant.clone(),
            KeyKind::BusTopic,
            &events_topic,
        );
        {
            let gw = state.gateway.read().await;
            match gw.state_store().get(&topic_key).await {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return (
                        StatusCode::NOT_FOUND,
                        Json(ErrorResponse {
                            error: format!(
                                "events topic {events_topic} is not registered; re-register the conversation or create the topic"
                            ),
                        }),
                    )
                        .into_response();
                }
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: e.to_string(),
                        }),
                    )
                        .into_response();
                }
            }
        }
        let mut msg = acteon_bus::BusMessage::new(events_topic.clone(), req.payload.clone())
            .with_key(&conv.conversation_id);
        for (k, v) in &req.headers {
            msg = msg.with_header(k.clone(), v.clone());
        }
        msg.headers.insert(
            "acteon.conversation.id".into(),
            conv.conversation_id.clone(),
        );
        if let Some(sender) = &req.sender {
            msg.headers
                .insert("acteon.conversation.sender".into(), sender.clone());
        }
        match backend.produce(msg).await {
            Ok(receipt) => {
                // Bump `updated_at` so list APIs and UI sorts by
                // activity reflect that the thread is alive. Uses CAS
                // so a concurrent transition can't be silently dropped
                // here. Best-effort: a CAS contention failure leaves
                // the timestamp stale but the message itself is
                // already on Kafka, so we don't fail the append.
                let conv_id = conv.conversation_id.clone();
                let conv_key = StateKey::new(
                    namespace.clone(),
                    tenant.clone(),
                    KeyKind::BusConversation,
                    &conv_id,
                );
                let bump = cas_update::<acteon_core::Conversation, _>(
                    &state,
                    &conv_key,
                    "conversation gone",
                    |c| {
                        c.updated_at = Utc::now();
                        Ok(())
                    },
                )
                .await;
                if let Err(_resp) = bump {
                    // Fail-open: the message is already on Kafka so we
                    // don't fail the response, but the state row is now
                    // stale relative to thread activity (UI sort by
                    // `updated_at` will misrank). `warn!` so operators
                    // notice if this becomes chronic — it usually
                    // means either the state store is unreachable or
                    // a single conversation is taking sustained CAS
                    // contention beyond `MAX_CAS_RETRY_ATTEMPTS`.
                    tracing::warn!(
                        conversation_id = %conv_id,
                        "conversation updated_at bump failed (CAS contention exhausted or backend error); message was produced successfully but list/sort will see a stale timestamp",
                    );
                }
                (
                    StatusCode::OK,
                    Json(AppendConversationMessageResponse {
                        events_topic: receipt.topic,
                        conversation_id: conv_id,
                        partition: receipt.partition,
                        offset: receipt.offset,
                        produced_at: receipt.timestamp,
                    }),
                )
                    .into_response()
            }
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response(),
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, namespace, tenant, conversation_id, req);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    get,
    path = "/v1/bus/conversations/{namespace}/{tenant}/{conversation_id}/messages",
    tag = "bus",
    params(
        ("namespace" = String, Path),
        ("tenant" = String, Path),
        ("conversation_id" = String, Path),
        ReplayConversationParams,
    ),
    responses(
        (status = 200, description = "Thread replay", body = ReplayConversationResponse),
        (status = 404, description = "Conversation not found", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
#[allow(clippy::too_many_lines)]
pub async fn replay_conversation_messages(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, conversation_id)): Path<(String, String, String)>,
    Query(params): Query<ReplayConversationParams>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        let Some(backend) = state.bus_backend.clone() else {
            return service_unavailable("bus feature not enabled");
        };
        if let Err(resp) =
            authorize_bus_op(&identity, &tenant, &namespace, BusOp::ManageConversation)
        {
            return resp;
        }
        let conv = match load_conversation(&state, &namespace, &tenant, &conversation_id).await {
            Ok(c) => c,
            Err(resp) => return resp,
        };
        // Read-side participant ACL. Without this, any caller with
        // the tenant grant could replay a private thread by id.
        if let Err(resp) = enforce_conversation_read_acl(&conv, params.as_agent.as_deref()) {
            return resp;
        }
        let limit = params.limit.unwrap_or(200).min(1000);
        let timeout_ms = params.timeout_ms.unwrap_or(1500);
        let events_topic = conv.effective_events_topic();
        // Capture per-partition high-water marks at scan start so we
        // can report `Complete` when the tracked offsets reach them.
        // Without this, an `assign()`-based scan never naturally
        // ends — Kafka holds the stream open waiting for new records,
        // and we'd have no way to distinguish "drained" from "took a
        // 1.5s nap on a quiet topic."
        let watermarks = match backend.scan_topic_watermarks(&events_topic).await {
            Ok(w) => w,
            Err(acteon_bus::BusError::TopicNotFound(_)) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse {
                        error: format!("events topic {events_topic} not found"),
                    }),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        };
        // Resume from the cursor if one was provided; otherwise start
        // at earliest/latest per `from`. `from` is ignored when
        // `cursor` is set so a paginating client doesn't accidentally
        // restart the scan.
        let scan_from = match (&params.cursor, params.from.as_deref()) {
            (Some(cursor_str), _) => match decode_replay_cursor(cursor_str) {
                Ok(offsets) => acteon_bus::ScanFrom::FromOffsets(offsets),
                Err(msg) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse {
                            error: format!("invalid replay cursor: {msg}"),
                        }),
                    )
                        .into_response();
                }
            },
            (None, Some("latest")) => acteon_bus::ScanFrom::Latest,
            (None, Some("earliest") | None) => acteon_bus::ScanFrom::Earliest,
            (None, Some(other)) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: format!("unknown 'from' value '{other}' (expected earliest|latest)"),
                    }),
                )
                    .into_response();
            }
        };
        // Pre-seed the per-partition offset tracker so that *if* the
        // scan exits without consuming any record on a given
        // partition (timeout, or no matches), the cursor still
        // points at a sensible resume position. Without this, an
        // empty `Latest` scan returned `cursor = "{}"`, which
        // decoded back to `ScanFrom::FromOffsets({})` — an empty TPL
        // — and the caller's polling loop would never see a new
        // message. We use `start - 1` because the kafka backend
        // resumes at `last_seen + 1`.
        //
        // For `Earliest`, seed all known partitions with `-1` →
        // resume at offset 0.
        // For `Latest`, seed each partition with `hwm - 1` → resume
        // at `hwm` (next-to-be-produced).
        // For `FromOffsets(map)`, copy the caller's positions and
        // fill any partitions absent from the map with `hwm - 1`
        // (the contract is "caller has already drained them").
        let mut last_offsets: std::collections::BTreeMap<i32, i64> =
            std::collections::BTreeMap::new();
        match &scan_from {
            acteon_bus::ScanFrom::Earliest => {
                for &p in watermarks.high_water_marks.keys() {
                    last_offsets.insert(p, -1);
                }
            }
            acteon_bus::ScanFrom::Latest => {
                for (&p, &hwm) in &watermarks.high_water_marks {
                    last_offsets.insert(p, hwm - 1);
                }
            }
            acteon_bus::ScanFrom::FromOffsets(map) => {
                for (&p, &off) in map {
                    last_offsets.insert(p, off);
                }
                for (&p, &hwm) in &watermarks.high_water_marks {
                    last_offsets.entry(p).or_insert(hwm - 1);
                }
            }
        }
        // `scan_topic` uses Kafka's `assign()` rather than dynamic
        // consumer-group subscribe — no `__consumer_offsets` rows are
        // created so repeated replays don't accumulate dead groups in
        // the cluster.
        let mut stream = match backend.scan_topic(&events_topic, scan_from).await {
            Ok(s) => s,
            Err(acteon_bus::BusError::TopicNotFound(_)) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse {
                        error: format!("events topic {events_topic} not found"),
                    }),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        };
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        let mut messages: Vec<ReplayMessageEntry> = Vec::new();
        // `last_offsets` was pre-seeded above. The loop bumps each
        // partition's entry on every record physically read so the
        // cursor reflects scan position rather than per-conversation
        // position.
        // Each branch terminates the loop with an explicit reason;
        // `loop { break <value>; }` lets the compiler prove
        // exhaustiveness without an unread initial value.
        // Fetch `limit + 1` so we can distinguish "exactly `limit`
        // messages exist" from "more exist" (the latter yields
        // `Limit`). Yield to the scheduler every `SCAN_YIELD_EVERY`
        // skipped records (those that belong to other conversations
        // on the shared events topic) so a busy tenant can't starve
        // other tasks on a single-threaded executor.
        let mut replay_skipped: u32 = 0;
        let exit_reason: ReplayExitReason = loop {
            if messages.len() > limit {
                messages.truncate(limit);
                break ReplayExitReason::Limit;
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                break ReplayExitReason::Timeout;
            }
            // Detect end-of-stream by comparing every partition's
            // tracked offset against its captured high-water mark.
            // `hwm - 1` is "last produced offset" (Kafka's `hwm` is
            // "next-to-be-produced"), so the partition is drained
            // when `last_offset >= hwm - 1`. Empty partitions
            // (hwm == 0) are trivially drained.
            let drained =
                watermarks
                    .high_water_marks
                    .iter()
                    .all(|(p, hwm)| match last_offsets.get(p) {
                        _ if *hwm == 0 => true,
                        Some(seen) => *seen >= *hwm - 1,
                        None => false,
                    });
            if drained {
                break ReplayExitReason::Complete;
            }
            let remaining = deadline - now;
            tokio::select! {
                next = stream.next() => {
                    match next {
                        Some(Ok(msg)) => {
                            let p = msg.partition.unwrap_or(0);
                            let off = msg.offset.unwrap_or(0);
                            // Always advance the per-partition cursor,
                            // even when the message belongs to a
                            // different conversation; a future
                            // pagination must skip past every record
                            // we've physically read.
                            last_offsets
                                .entry(p)
                                .and_modify(|e| {
                                    if off > *e {
                                        *e = off;
                                    }
                                })
                                .or_insert(off);
                            let belongs = msg
                                .headers
                                .get("acteon.conversation.id")
                                .is_some_and(|v| v == &conv.conversation_id);
                            if !belongs {
                                replay_skipped = replay_skipped.wrapping_add(1);
                                if replay_skipped.is_multiple_of(SCAN_YIELD_EVERY) {
                                    tokio::task::yield_now().await;
                                }
                                continue;
                            }
                            messages.push(ReplayMessageEntry {
                                partition: p,
                                offset: off,
                                key: msg.key.clone(),
                                payload: msg.payload.clone(),
                                headers: msg.headers.clone(),
                                timestamp: msg.timestamp.unwrap_or_else(Utc::now),
                            });
                        }
                        Some(Err(_)) | None => {
                            // Stream ended unexpectedly — not at the
                            // high-water mark, but no more messages
                            // from the backend. Treat as a partial
                            // result so the client paginates; safer
                            // than silently lying about completeness.
                            break ReplayExitReason::Timeout;
                        }
                    }
                }
                () = tokio::time::sleep(remaining) => {
                    break ReplayExitReason::Timeout;
                }
            }
        };
        // Always emit a cursor — even on `Complete`. The
        // `Complete` cursor encodes the per-partition tail at scan
        // end, so a tailing client can immediately re-poll to pick
        // up messages produced after this snapshot. Clients that
        // genuinely just want a one-shot drain ignore the cursor
        // when `exit_reason == Complete`.
        let cursor = Some(encode_replay_cursor(&last_offsets));
        (
            StatusCode::OK,
            Json(ReplayConversationResponse {
                conversation_id: conv.conversation_id,
                events_topic,
                messages,
                exit_reason,
                cursor,
            }),
        )
            .into_response()
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, namespace, tenant, conversation_id, params);
        service_unavailable("bus feature not compiled")
    }
}

/// Direct `StateKey` lookup of a conversation record.
#[cfg(feature = "bus")]
async fn load_conversation(
    state: &AppState,
    namespace: &str,
    tenant: &str,
    conversation_id: &str,
) -> Result<acteon_core::Conversation, axum::response::Response> {
    let key = StateKey::new(
        namespace.to_string(),
        tenant.to_string(),
        KeyKind::BusConversation,
        conversation_id.to_string(),
    );
    let gw = state.gateway.read().await;
    match gw.state_store().get(&key).await {
        Ok(Some(raw)) => serde_json::from_str::<acteon_core::Conversation>(&raw).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!(
                        "corrupt conversation record for {namespace}.{tenant}.{conversation_id}: {e}"
                    ),
                }),
            )
                .into_response()
        }),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("conversation {namespace}.{tenant}.{conversation_id} not found"),
            }),
        )
            .into_response()),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response()),
    }
}

// =============================================================================
// Phase 6a: tool-call envelopes layered on conversation messages
// =============================================================================

/// Body of `POST /v1/bus/conversations/{ns}/{t}/{id}/tool-calls`.
/// Wraps a [`acteon_core::ToolCall`] and produces a conversation
/// message with envelope kind `"tool_call"`. The bus stamps routing
/// headers (`acteon.envelope.kind`, `acteon.tool.call_id`,
/// `acteon.correlation_id`, `acteon.reply_to`) so subscribers can
/// filter without parsing the payload.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PostToolCallRequest {
    pub call_id: String,
    pub tool: String,
    /// Tool arguments. Free-form JSON; if the events topic is
    /// schema-bound (Phase 3), validation runs at the underlying
    /// publish edge.
    #[schema(value_type = Object)]
    #[serde(default)]
    pub arguments: serde_json::Value,
    /// Optional correlation token, propagated to the eventual
    /// [`PostToolResultRequest`]. Stamped as `acteon.correlation_id`.
    #[serde(default)]
    pub correlation_id: Option<String>,
    /// Override the conversation the matching result should land in.
    /// Empty / null = "same conversation" (the reply lands here).
    /// Stamped as `acteon.reply_to`.
    #[serde(default)]
    pub reply_to: Option<String>,
    /// Sender agent id. Stamped as both
    /// `acteon.conversation.sender` and on the envelope.
    #[serde(default)]
    pub sender: Option<String>,
    /// Free-form tool metadata. Bounded by the publish path's label
    /// caps (max 32 entries / 256B keys / 4KB values).
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Body of `POST /v1/bus/conversations/{ns}/{t}/{id}/tool-results`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PostToolResultRequest {
    pub call_id: String,
    pub status: acteon_core::ToolResultStatus,
    #[schema(value_type = Object)]
    #[serde(default)]
    pub output: serde_json::Value,
    #[serde(default)]
    pub error_message: Option<String>,
    #[serde(default)]
    pub correlation_id: Option<String>,
    #[serde(default)]
    pub sender: Option<String>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ToolEnvelopeReceipt {
    pub events_topic: String,
    pub conversation_id: String,
    pub call_id: String,
    pub partition: i32,
    pub offset: i64,
    pub produced_at: DateTime<Utc>,
    /// Opaque resume cursor encoding `{partition: offset}`. Pass it
    /// back to `GET /v1/bus/tool-calls/{...}/result` as `?cursor=`
    /// so the lookup scans only messages produced *after* this
    /// envelope. Without a cursor, the lookup defaults to scanning
    /// from the tail (which races with the result landing) or has
    /// to do a full-history scan — neither is acceptable on a busy
    /// cluster.
    pub cursor: String,
}

/// Result of a `GET /v1/bus/tool-calls/{ns}/{t}/{call_id}/result` —
/// either the matching [`acteon_core::ToolResult`] envelope plus a
/// few bus coordinates, or a 408 if the timeout elapsed without a
/// match.
#[derive(Debug, Serialize, ToSchema)]
pub struct ToolResultLookupResponse {
    pub call_id: String,
    pub events_topic: String,
    pub conversation_id: String,
    pub partition: i32,
    pub offset: i64,
    pub produced_at: DateTime<Utc>,
    pub result: acteon_core::ToolResult,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ToolResultLookupParams {
    /// **Required.** The conversation to scan for the matching
    /// result. For `reply_to`-routed flows, set this to the
    /// `reply_to` conversation. Required (rather than defaulting to
    /// the tenant's shared events topic) so a conversation
    /// registered with a custom `events_topic` is scanned at the
    /// right Kafka topic — falling back to the default would have
    /// silently looked at the wrong place.
    pub conversation_id: String,
    /// Resume cursor from the originating
    /// `POST /tool-calls` receipt — encodes the per-partition
    /// offset where the call was produced. Without it, the scan
    /// defaults to `Latest` (cheap on a busy cluster, but races
    /// with the result landing) — passing the receipt's `cursor`
    /// is strongly recommended for production flows.
    #[serde(default)]
    pub cursor: Option<String>,
    /// How long to wait for a matching result. Default 5s; max 30s.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// `agent_id` the caller is acting as. Required when the target
    /// conversation has a non-empty `participants` ACL — must be on
    /// the list, otherwise the request is rejected. Without it (or
    /// when the conversation is open), any caller with the
    /// tenant-level grant can read; this is the V1 read-isolation
    /// model. Future work: derive the agent identity from the
    /// caller's API-key grant rather than a self-asserted query
    /// param.
    #[serde(default)]
    pub as_agent: Option<String>,
}

/// Envelope-kind header values. Server-stamped so subscribers can
/// route on a single header lookup.
#[cfg(feature = "bus")]
const ENVELOPE_KIND_TOOL_CALL: &str = "tool_call";
#[cfg(feature = "bus")]
const ENVELOPE_KIND_TOOL_RESULT: &str = "tool_result";

/// Helper: stamp the standard envelope routing headers on a
/// `BusMessage`. Reserved `acteon.*` keys are inserted directly
/// (`with_header` strips them on user input by design).
#[cfg(feature = "bus")]
fn stamp_tool_envelope_headers(
    msg: &mut acteon_bus::BusMessage,
    kind: &'static str,
    call_id: &str,
    correlation_id: Option<&str>,
    reply_to: Option<&str>,
) {
    msg.headers
        .insert("acteon.envelope.kind".into(), kind.to_string());
    msg.headers
        .insert("acteon.tool.call_id".into(), call_id.to_string());
    if let Some(c) = correlation_id {
        msg.headers
            .insert("acteon.correlation_id".into(), c.to_string());
    }
    if let Some(r) = reply_to {
        msg.headers.insert("acteon.reply_to".into(), r.to_string());
    }
}

#[utoipa::path(
    post,
    path = "/v1/bus/conversations/{namespace}/{tenant}/{conversation_id}/tool-calls",
    tag = "bus",
    request_body = PostToolCallRequest,
    params(
        ("namespace" = String, Path),
        ("tenant" = String, Path),
        ("conversation_id" = String, Path),
    ),
    responses(
        (status = 200, description = "Tool-call envelope appended", body = ToolEnvelopeReceipt),
        (status = 400, description = "Invalid envelope, sender ACL, or archived conversation", body = ErrorResponse),
        (status = 404, description = "Conversation not found", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
#[allow(clippy::too_many_lines)]
pub async fn post_tool_call(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, conversation_id)): Path<(String, String, String)>,
    Json(req): Json<PostToolCallRequest>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        let Some(backend) = state.bus_backend.as_ref() else {
            return service_unavailable("bus feature not enabled");
        };
        // Same two-level auth as `append_conversation_message`: must
        // be allowed to manage conversations *and* publish on the
        // tenant. A registry-only role can't quietly inject tool
        // traffic onto the events topic.
        if let Err(resp) =
            authorize_bus_op(&identity, &tenant, &namespace, BusOp::ManageConversation)
        {
            return resp;
        }
        if let Err(resp) = authorize_bus_op(&identity, &tenant, &namespace, BusOp::Publish) {
            return resp;
        }
        // Construct the typed envelope first so validation runs
        // before we do any bus or state I/O.
        let mut envelope =
            acteon_core::ToolCall::new(&req.call_id, &req.tool, req.arguments.clone());
        envelope.correlation_id = req.correlation_id.clone();
        envelope.reply_to = req.reply_to.clone();
        envelope.sender = req.sender.clone();
        envelope.metadata = req.metadata.clone();
        if let Err(e) = envelope.validate() {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response();
        }
        if let Err(resp) = validate_user_labels(&envelope.metadata) {
            return resp;
        }
        let conv = match load_conversation(&state, &namespace, &tenant, &conversation_id).await {
            Ok(c) => c,
            Err(resp) => return resp,
        };
        if !conv.accepts_messages() {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!(
                        "conversation {} is archived; reopen via /transition before posting tool calls",
                        conv.conversation_id
                    ),
                }),
            )
                .into_response();
        }
        // Same participant ACL as plain conversation messages: a
        // sender outside the participant list is rejected; if the
        // ACL is set and `sender` is omitted, reject explicitly.
        if !conv.participants.is_empty() {
            match envelope.sender.as_deref() {
                Some(s) if conv.participants.iter().any(|p| p == s) => {}
                Some(s) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse {
                            error: format!(
                                "sender '{s}' is not a participant of conversation {}",
                                conv.conversation_id
                            ),
                        }),
                    )
                        .into_response();
                }
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse {
                            error: format!(
                                "conversation {} has a participant ACL; tool-call sender is required",
                                conv.conversation_id
                            ),
                        }),
                    )
                        .into_response();
                }
            }
        }
        let payload = match serde_json::to_value(&envelope) {
            Ok(v) => v,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        };
        let events_topic = conv.effective_events_topic();
        // Phase 3 schema validation. If the conversation events topic
        // has a schema binding, the payload must match — otherwise
        // tool envelopes would silently bypass the publish-edge gate
        // that `/v1/bus/publish` enforces.
        if let Err(resp) = validate_payload_against_topic_schema(
            &state,
            &conv.namespace,
            &conv.tenant,
            &events_topic,
            &payload,
        )
        .await
        {
            return resp;
        }
        let mut msg = acteon_bus::BusMessage::new(events_topic.clone(), payload)
            .with_key(&conv.conversation_id);
        // Conversation-level routing headers, mirroring
        // `append_conversation_message`.
        msg.headers.insert(
            "acteon.conversation.id".into(),
            conv.conversation_id.clone(),
        );
        if let Some(s) = &envelope.sender {
            msg.headers
                .insert("acteon.conversation.sender".into(), s.clone());
        }
        stamp_tool_envelope_headers(
            &mut msg,
            ENVELOPE_KIND_TOOL_CALL,
            &envelope.call_id,
            envelope.correlation_id.as_deref(),
            envelope.reply_to.as_deref(),
        );
        match backend.produce(msg).await {
            Ok(receipt) => {
                // Include a cursor pointing at this envelope's
                // partition+offset so the caller can scan
                // strictly-newer messages on a follow-up
                // `lookup_tool_result` without re-reading topic
                // history. Same encoding as Phase 5 replay cursors.
                let mut cursor_map = std::collections::BTreeMap::new();
                cursor_map.insert(receipt.partition, receipt.offset);
                let cursor = encode_replay_cursor(&cursor_map);
                let conv_id_for_bump = conv.conversation_id.clone();
                let resp = (
                    StatusCode::OK,
                    Json(ToolEnvelopeReceipt {
                        events_topic: receipt.topic,
                        conversation_id: conv.conversation_id,
                        call_id: envelope.call_id,
                        partition: receipt.partition,
                        offset: receipt.offset,
                        produced_at: receipt.timestamp,
                        cursor,
                    }),
                );
                // Bump conversation `updated_at` so list/sort by
                // activity sees tool-driven traffic. Best-effort: if
                // the CAS contends out, the message is already on
                // Kafka; we log and return success rather than fail
                // the produce.
                let bump_key = StateKey::new(
                    namespace.clone(),
                    tenant.clone(),
                    KeyKind::BusConversation,
                    &conv_id_for_bump,
                );
                let bump = cas_update::<acteon_core::Conversation, _>(
                    &state,
                    &bump_key,
                    "conversation gone",
                    |c| {
                        c.updated_at = Utc::now();
                        Ok(())
                    },
                )
                .await;
                if bump.is_err() {
                    tracing::warn!(
                        conversation_id = %conv_id_for_bump,
                        "tool-envelope produced but conversation updated_at bump failed (CAS contention exhausted or backend error); list/sort will see a stale timestamp",
                    );
                }
                resp.into_response()
            }
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response(),
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, namespace, tenant, conversation_id, req);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    post,
    path = "/v1/bus/conversations/{namespace}/{tenant}/{conversation_id}/tool-results",
    tag = "bus",
    request_body = PostToolResultRequest,
    params(
        ("namespace" = String, Path),
        ("tenant" = String, Path),
        ("conversation_id" = String, Path),
    ),
    responses(
        (status = 200, description = "Tool-result envelope appended", body = ToolEnvelopeReceipt),
        (status = 400, description = "Invalid envelope or archived conversation", body = ErrorResponse),
        (status = 404, description = "Conversation not found", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
#[allow(clippy::too_many_lines)]
pub async fn post_tool_result(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, conversation_id)): Path<(String, String, String)>,
    Json(req): Json<PostToolResultRequest>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        let Some(backend) = state.bus_backend.as_ref() else {
            return service_unavailable("bus feature not enabled");
        };
        if let Err(resp) =
            authorize_bus_op(&identity, &tenant, &namespace, BusOp::ManageConversation)
        {
            return resp;
        }
        if let Err(resp) = authorize_bus_op(&identity, &tenant, &namespace, BusOp::Publish) {
            return resp;
        }
        let envelope = acteon_core::ToolResult {
            call_id: req.call_id.clone(),
            status: req.status,
            output: req.output.clone(),
            error_message: req.error_message.clone(),
            correlation_id: req.correlation_id.clone(),
            sender: req.sender.clone(),
            metadata: req.metadata.clone(),
            created_at: Utc::now(),
        };
        if let Err(e) = envelope.validate() {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response();
        }
        if let Err(resp) = validate_user_labels(&envelope.metadata) {
            return resp;
        }
        let conv = match load_conversation(&state, &namespace, &tenant, &conversation_id).await {
            Ok(c) => c,
            Err(resp) => return resp,
        };
        if !conv.accepts_messages() {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!(
                        "conversation {} is archived; reopen via /transition before posting tool results",
                        conv.conversation_id
                    ),
                }),
            )
                .into_response();
        }
        if !conv.participants.is_empty() {
            match envelope.sender.as_deref() {
                Some(s) if conv.participants.iter().any(|p| p == s) => {}
                Some(s) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse {
                            error: format!(
                                "sender '{s}' is not a participant of conversation {}",
                                conv.conversation_id
                            ),
                        }),
                    )
                        .into_response();
                }
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse {
                            error: format!(
                                "conversation {} has a participant ACL; tool-result sender is required",
                                conv.conversation_id
                            ),
                        }),
                    )
                        .into_response();
                }
            }
        }
        // `created_at` was server-stamped at construction (see the
        // `Utc::now()` in the struct literal above). The previous
        // version re-stamped it here as well — a copy-paste artifact
        // from when the request type briefly carried a client-
        // supplied timestamp. Removed.
        let payload = match serde_json::to_value(&envelope) {
            Ok(v) => v,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        };
        let events_topic = conv.effective_events_topic();
        // Same Phase 3 schema-validation gate as `post_tool_call`;
        // see the matching comment there.
        if let Err(resp) = validate_payload_against_topic_schema(
            &state,
            &conv.namespace,
            &conv.tenant,
            &events_topic,
            &payload,
        )
        .await
        {
            return resp;
        }
        let mut msg = acteon_bus::BusMessage::new(events_topic.clone(), payload)
            .with_key(&conv.conversation_id);
        msg.headers.insert(
            "acteon.conversation.id".into(),
            conv.conversation_id.clone(),
        );
        if let Some(s) = &envelope.sender {
            msg.headers
                .insert("acteon.conversation.sender".into(), s.clone());
        }
        stamp_tool_envelope_headers(
            &mut msg,
            ENVELOPE_KIND_TOOL_RESULT,
            &envelope.call_id,
            envelope.correlation_id.as_deref(),
            None, // results don't carry a reply_to
        );
        match backend.produce(msg).await {
            Ok(receipt) => {
                // Include a cursor pointing at this envelope's
                // partition+offset so the caller can scan
                // strictly-newer messages on a follow-up
                // `lookup_tool_result` without re-reading topic
                // history. Same encoding as Phase 5 replay cursors.
                let mut cursor_map = std::collections::BTreeMap::new();
                cursor_map.insert(receipt.partition, receipt.offset);
                let cursor = encode_replay_cursor(&cursor_map);
                let conv_id_for_bump = conv.conversation_id.clone();
                let resp = (
                    StatusCode::OK,
                    Json(ToolEnvelopeReceipt {
                        events_topic: receipt.topic,
                        conversation_id: conv.conversation_id,
                        call_id: envelope.call_id,
                        partition: receipt.partition,
                        offset: receipt.offset,
                        produced_at: receipt.timestamp,
                        cursor,
                    }),
                );
                // Bump conversation `updated_at` so list/sort by
                // activity sees tool-driven traffic. Best-effort: if
                // the CAS contends out, the message is already on
                // Kafka; we log and return success rather than fail
                // the produce.
                let bump_key = StateKey::new(
                    namespace.clone(),
                    tenant.clone(),
                    KeyKind::BusConversation,
                    &conv_id_for_bump,
                );
                let bump = cas_update::<acteon_core::Conversation, _>(
                    &state,
                    &bump_key,
                    "conversation gone",
                    |c| {
                        c.updated_at = Utc::now();
                        Ok(())
                    },
                )
                .await;
                if bump.is_err() {
                    tracing::warn!(
                        conversation_id = %conv_id_for_bump,
                        "tool-envelope produced but conversation updated_at bump failed (CAS contention exhausted or backend error); list/sort will see a stale timestamp",
                    );
                }
                resp.into_response()
            }
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response(),
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, namespace, tenant, conversation_id, req);
        service_unavailable("bus feature not compiled")
    }
}

#[utoipa::path(
    get,
    path = "/v1/bus/tool-calls/{namespace}/{tenant}/{call_id}/result",
    tag = "bus",
    params(
        ("namespace" = String, Path),
        ("tenant" = String, Path),
        ("call_id" = String, Path),
        ToolResultLookupParams,
    ),
    responses(
        (status = 200, description = "Matching tool result", body = ToolResultLookupResponse),
        (status = 408, description = "Timed out before a matching result arrived", body = ErrorResponse),
        (status = 404, description = "Events topic not found", body = ErrorResponse),
        (status = 503, description = "Bus feature disabled", body = ErrorResponse),
    ),
)]
#[allow(clippy::too_many_lines)]
pub async fn lookup_tool_result(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, call_id)): Path<(String, String, String)>,
    Query(params): Query<ToolResultLookupParams>,
) -> impl IntoResponse {
    #[cfg(feature = "bus")]
    {
        let Some(backend) = state.bus_backend.clone() else {
            return service_unavailable("bus feature not enabled");
        };
        if let Err(resp) =
            authorize_bus_op(&identity, &tenant, &namespace, BusOp::ManageConversation)
        {
            return resp;
        }
        if let Err(e) = acteon_core::ToolCall::validate_id_field("call_id", &call_id) {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response();
        }
        // Resolve the conversation's events topic from its actual
        // record. `conversation_id` is required (rather than
        // defaulting to the tenant's shared events topic) because a
        // conversation registered with a custom `events_topic`
        // (Phase 5) would otherwise silently look at the wrong
        // place — the scan would never find a matching record and
        // every lookup would 408.
        let conv =
            match load_conversation(&state, &namespace, &tenant, &params.conversation_id).await {
                Ok(c) => c,
                Err(resp) => return resp,
            };
        // Read-side participant ACL — match the gate the write-side
        // tool handlers already enforce.
        if let Err(resp) = enforce_conversation_read_acl(&conv, params.as_agent.as_deref()) {
            return resp;
        }
        let events_topic = conv.effective_events_topic();
        let timeout_ms = params.timeout_ms.unwrap_or(5_000).min(30_000);
        // Resolve scan position.
        //
        // With a cursor (the typical flow: caller passes back the
        // receipt's `cursor`), the call's partition resumes from
        // the recorded offset; partitions the cursor doesn't cover
        // default to `Latest` *inside the backend* (see the
        // `ScanFrom::FromOffsets` doc). That keeps single-partition
        // cursors valid for `reply_to` flows where the result
        // lands on a different partition.
        //
        // Without a cursor, default to `Latest` — cheap on a busy
        // cluster, but races with the result landing. Earlier the
        // default was `Earliest`, which turned every lookup into a
        // full-history scan and was a straightforward DoS vector.
        let scan_from = match &params.cursor {
            Some(c) => match decode_replay_cursor(c) {
                Ok(offsets) => acteon_bus::ScanFrom::FromOffsets(offsets),
                Err(msg) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse {
                            error: format!("invalid lookup cursor: {msg}"),
                        }),
                    )
                        .into_response();
                }
            },
            None => acteon_bus::ScanFrom::Latest,
        };
        let mut stream = match backend.scan_topic(&events_topic, scan_from).await {
            Ok(s) => s,
            Err(acteon_bus::BusError::TopicNotFound(_)) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse {
                        error: format!("events topic {events_topic} not found"),
                    }),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        };
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        // Yield to the scheduler every `SCAN_YIELD_EVERY` skipped
        // messages so a busy topic with no matches doesn't starve
        // other tasks on a single-threaded executor. `stream.next()`
        // may return `Poll::Ready` repeatedly when the consumer has
        // buffered data; without a manual yield, the loop body has
        // no other `.await` points that surrender the worker.
        let mut skipped: u32 = 0;
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return (
                    StatusCode::REQUEST_TIMEOUT,
                    Json(ErrorResponse {
                        error: format!(
                            "no tool result for call_id '{call_id}' arrived within {timeout_ms}ms"
                        ),
                    }),
                )
                    .into_response();
            }
            let remaining = deadline - now;
            tokio::select! {
                next = stream.next() => {
                    match next {
                        Some(Ok(msg)) => {
                            // Header-only filter: only inspect the
                            // payload when this is *the* tool result
                            // we're waiting for. Saves
                            // serde-deserializing every record on a
                            // busy topic.
                            let is_result = msg
                                .headers
                                .get("acteon.envelope.kind")
                                .is_some_and(|v| v == ENVELOPE_KIND_TOOL_RESULT);
                            let matches_call = msg
                                .headers
                                .get("acteon.tool.call_id")
                                .is_some_and(|v| v == &call_id);
                            if !is_result || !matches_call {
                                skipped = skipped.wrapping_add(1);
                                if skipped.is_multiple_of(SCAN_YIELD_EVERY) {
                                    tokio::task::yield_now().await;
                                }
                                continue;
                            }
                            let result: acteon_core::ToolResult =
                                match serde_json::from_value(msg.payload.clone()) {
                                    Ok(r) => r,
                                    Err(e) => {
                                        // Poison-pill: header said this
                                        // is a tool-result for our
                                        // `call_id`, but the payload
                                        // doesn't parse. Treat as
                                        // garbage and keep scanning —
                                        // a duplicate or malformed
                                        // record must not permanently
                                        // break lookups for that
                                        // `call_id`. Log loudly so
                                        // operators notice.
                                        tracing::warn!(
                                            call_id = %call_id,
                                            partition = ?msg.partition,
                                            offset = ?msg.offset,
                                            error = %e,
                                            "skipping malformed tool-result envelope; payload failed to deserialize",
                                        );
                                        continue;
                                    }
                                };
                            // The scan key carries the conversation
                            // id; surface it on the response so the
                            // caller doesn't have to re-derive it.
                            let conv_id = msg.key.clone().unwrap_or_default();
                            return (
                                StatusCode::OK,
                                Json(ToolResultLookupResponse {
                                    call_id: result.call_id.clone(),
                                    events_topic,
                                    conversation_id: conv_id,
                                    partition: msg.partition.unwrap_or(0),
                                    offset: msg.offset.unwrap_or(0),
                                    produced_at: msg.timestamp.unwrap_or_else(Utc::now),
                                    result,
                                }),
                            )
                                .into_response();
                        }
                        Some(Err(_)) | None => {
                            // Stream ended without a match. Treat as
                            // timeout — clients retry.
                            return (
                                StatusCode::REQUEST_TIMEOUT,
                                Json(ErrorResponse {
                                    error: format!(
                                        "stream ended without a tool result for call_id '{call_id}'"
                                    ),
                                }),
                            )
                                .into_response();
                        }
                    }
                }
                () = tokio::time::sleep(remaining) => {
                    return (
                        StatusCode::REQUEST_TIMEOUT,
                        Json(ErrorResponse {
                            error: format!(
                                "no tool result for call_id '{call_id}' arrived within {timeout_ms}ms"
                            ),
                        }),
                    )
                        .into_response();
                }
            }
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, namespace, tenant, call_id, params);
        service_unavailable("bus feature not compiled")
    }
}
