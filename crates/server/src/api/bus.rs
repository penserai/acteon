//! Phase 1 bus API: topics CRUD + publish + subscribe.
//!
//! All endpoints return `503 Service Unavailable` when the server was
//! compiled without the `bus` feature or when `[bus].enabled = false`
//! in the TOML config. On the feature-enabled path they interact with
//! an `acteon_bus::SharedBackend` held in [`super::AppState`]. The
//! reference is a plain code span rather than a rustdoc link because
//! `acteon_bus` is only in scope when built with `--features bus`, and
//! CI's `cargo doc` runs with default features.

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
        match store.get(&key).await {
            Ok(Some(_)) => {
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
            Ok(None) => {}
        }

        let Ok(body) = serde_json::to_string(&topic) else {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "failed to serialize topic".into(),
                }),
            )
                .into_response();
        };
        if let Err(e) = store.set(&key, &body, None).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response();
        }
        drop(gw);

        // Now create in Kafka.
        if let Err(e) = backend.create_topic(&topic).await {
            // Best-effort rollback of the state row on Kafka failure.
            // If the rollback itself fails — e.g. state store temporary
            // outage — Acteon carries a dangling record that doesn't
            // exist in Kafka. Log loudly so operators can reconcile.
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
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
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
        (status = 400, description = "Invalid topic or reserved header prefix", body = ErrorResponse),
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
        // — future schema validation (Phase 3) would have no hook.
        let topic_key = StateKey::new(ns, tenant, KeyKind::BusTopic, &topic_name);
        {
            let gw = state.gateway.read().await;
            match gw.state_store().get(&topic_key).await {
                Ok(Some(_)) => {}
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
        match store.get(&sub_key).await {
            Ok(Some(_)) => {
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
            Ok(None) => {}
        }
        let Ok(body) = serde_json::to_string(&sub) else {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "failed to serialize subscription".into(),
                }),
            )
                .into_response();
        };
        if let Err(e) = store.set(&sub_key, &body, None).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response();
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
