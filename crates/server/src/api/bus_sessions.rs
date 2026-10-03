//! Receipt-based, owner-bound HTTP sessions. The registry lives on this server instance.
use super::{AppState, schemas::ErrorResponse};
#[cfg(feature = "bus")]
use crate::{
    auth::identity::CallerIdentity,
    bus_sessions::{SessionError, SessionHandle},
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenSessionRequest {
    #[schema(value_type = String, format = "uuid")]
    pub request_id: uuid::Uuid,
}
#[derive(Debug, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ReceiveRequest {
    pub max_messages: usize,
    pub wait_ms: u64,
}
impl Default for ReceiveRequest {
    fn default() -> Self {
        Self {
            max_messages: 64,
            wait_ms: 10_000,
        }
    }
}
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReceiptRequest {
    #[schema(value_type = Vec<String>)]
    pub receipt_ids: Vec<uuid::Uuid>,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SessionResponse {
    #[schema(value_type = String, format = "uuid")]
    pub session_id: uuid::Uuid,
    pub consumer_group: String,
    pub phase: String,
    pub assignment_epoch: u64,
    pub partitions: Vec<i32>,
    pub pending: usize,
    pub buffered_bytes: usize,
    pub closed_reason: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SessionDelivery {
    #[schema(value_type = String, format = "uuid")]
    pub receipt_id: uuid::Uuid,
    pub message: serde_json::Value,
    pub partition: i32,
    /// Last consumed offset; acknowledgement accepts only the opaque receipt ID.
    pub offset: i64,
    pub assignment_epoch: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReceiveResponse {
    pub deliveries: Vec<SessionDelivery>,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReceiptPosition {
    pub partition: i32,
    pub offset: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReceiptResponse {
    pub consumer_group: String,
    pub positions: Vec<ReceiptPosition>,
    pub remaining_in_flight: usize,
}

#[cfg(feature = "bus")]
impl IntoResponse for SessionError {
    fn into_response(self) -> axum::response::Response {
        (
            self.status,
            Json(ErrorResponse {
                error: self.message,
            }),
        )
            .into_response()
    }
}

#[cfg(feature = "bus")]
#[allow(clippy::result_large_err)] // Return the existing HTTP rejection without heap indirection.
async fn lookup(
    state: &AppState,
    identity: &CallerIdentity,
    ns: &str,
    tenant: &str,
    id: &str,
    session: uuid::Uuid,
) -> Result<std::sync::Arc<SessionHandle>, axum::response::Response> {
    super::bus::authorize_bus_op(identity, tenant, ns, super::bus::BusOp::Subscribe)?;
    if state.bus_backend.is_none() {
        return Err(disabled());
    }
    let sub = super::bus::load_subscription(state, ns, tenant, id).await?;
    state
        .bus_sessions
        .get(&sub, identity, session)
        .map_err(IntoResponse::into_response)
}

#[utoipa::path(post, path="/v1/bus/subscriptions/{namespace}/{tenant}/{id}/sessions", tag="bus", request_body=OpenSessionRequest, responses((status=200, body=SessionResponse), (status=409, body=ErrorResponse), (status=429, body=ErrorResponse)))]
pub async fn open(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((ns, tenant, id)): Path<(String, String, String)>,
    Json(req): Json<OpenSessionRequest>,
) -> axum::response::Response {
    #[cfg(feature = "bus")]
    {
        if let Err(e) =
            super::bus::authorize_bus_op(&identity, &tenant, &ns, super::bus::BusOp::Subscribe)
        {
            return e;
        }
        let sub = match super::bus::load_subscription(&state, &ns, &tenant, &id).await {
            Ok(s) => s,
            Err(e) => return e,
        };
        let Some(backend) = state.bus_backend.clone() else {
            return disabled();
        };
        let handle = match state
            .bus_sessions
            .open(sub.clone(), &identity, req.request_id, backend)
        {
            Ok(h) => h,
            Err(e) => return e.into_response(),
        };
        // Registration precedes this second lookup: deletion/recreation during opening
        // cannot leave a usable session pinned to an obsolete definition.
        match super::bus::load_subscription(&state, &ns, &tenant, &id).await {
            Ok(current) if handle.matches(&current) => {}
            _ => {
                handle.close();
                return (
                    StatusCode::CONFLICT,
                    Json(ErrorResponse {
                        error: "subscription changed while opening".into(),
                    }),
                )
                    .into_response();
            }
        }
        match handle.ready().await {
            Ok(s) => Json(s).into_response(),
            Err(e) => e.into_response(),
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, ns, tenant, id, req);
        disabled()
    }
}

#[utoipa::path(get, path="/v1/bus/subscriptions/{namespace}/{tenant}/{id}/sessions/{session}", tag="bus", responses((status=200, body=SessionResponse), (status=404, body=ErrorResponse)))]
pub async fn get(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((ns, tenant, id, session)): Path<(String, String, String, uuid::Uuid)>,
) -> axum::response::Response {
    #[cfg(feature = "bus")]
    {
        match lookup(&state, &identity, &ns, &tenant, &id, session).await {
            Ok(h) => Json(h.snapshot()).into_response(),
            Err(e) => e,
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, ns, tenant, id, session);
        disabled()
    }
}
#[utoipa::path(delete, path="/v1/bus/subscriptions/{namespace}/{tenant}/{id}/sessions/{session}", tag="bus", responses((status=204), (status=404, body=ErrorResponse)))]
pub async fn close(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((ns, tenant, id, session)): Path<(String, String, String, uuid::Uuid)>,
) -> axum::response::Response {
    #[cfg(feature = "bus")]
    {
        match lookup(&state, &identity, &ns, &tenant, &id, session).await {
            Ok(h) => {
                h.close();
                StatusCode::NO_CONTENT.into_response()
            }
            Err(e) => e,
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, ns, tenant, id, session);
        disabled()
    }
}
#[utoipa::path(post, path="/v1/bus/subscriptions/{namespace}/{tenant}/{id}/sessions/{session}/receive", tag="bus", request_body=ReceiveRequest, responses((status=200, body=ReceiveResponse), (status=410, body=ErrorResponse)))]
pub async fn receive(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((ns, tenant, id, session)): Path<(String, String, String, uuid::Uuid)>,
    Json(req): Json<ReceiveRequest>,
) -> axum::response::Response {
    #[cfg(feature = "bus")]
    {
        let h = match lookup(&state, &identity, &ns, &tenant, &id, session).await {
            Ok(h) => h,
            Err(e) => return e,
        };
        match h.receive(req).await {
            Ok(r) => Json(r).into_response(),
            Err(e) => e.into_response(),
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, ns, tenant, id, session, req);
        disabled()
    }
}
#[utoipa::path(post, path="/v1/bus/subscriptions/{namespace}/{tenant}/{id}/sessions/{session}/validate", tag="bus", request_body=ReceiptRequest, responses((status=200, body=ReceiptResponse), (status=409, body=ErrorResponse)))]
pub async fn validate(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((ns, tenant, id, session)): Path<(String, String, String, uuid::Uuid)>,
    Json(req): Json<ReceiptRequest>,
) -> axum::response::Response {
    #[cfg(feature = "bus")]
    {
        let h = match lookup(&state, &identity, &ns, &tenant, &id, session).await {
            Ok(h) => h,
            Err(e) => return e,
        };
        match h.receipts(req.receipt_ids, false).await {
            Ok(r) => Json(r).into_response(),
            Err(e) => e.into_response(),
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, ns, tenant, id, session, req);
        disabled()
    }
}
#[utoipa::path(post, path="/v1/bus/subscriptions/{namespace}/{tenant}/{id}/sessions/{session}/ack", tag="bus", request_body=ReceiptRequest, responses((status=200, body=ReceiptResponse), (status=409, body=ErrorResponse)))]
pub async fn ack(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((ns, tenant, id, session)): Path<(String, String, String, uuid::Uuid)>,
    Json(req): Json<ReceiptRequest>,
) -> axum::response::Response {
    #[cfg(feature = "bus")]
    {
        let h = match lookup(&state, &identity, &ns, &tenant, &id, session).await {
            Ok(h) => h,
            Err(e) => return e,
        };
        match h.receipts(req.receipt_ids, true).await {
            Ok(r) => Json(r).into_response(),
            Err(e) => e.into_response(),
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, ns, tenant, id, session, req);
        disabled()
    }
}
fn disabled() -> axum::response::Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ErrorResponse {
            error: "bus feature not enabled".into(),
        }),
    )
        .into_response()
}
