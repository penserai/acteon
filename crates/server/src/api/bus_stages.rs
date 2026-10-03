//! Tenant-scoped durable stage operations. Kafka availability is not required.
#![allow(clippy::result_large_err)]
use super::AppState;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}
fn default_limit() -> usize {
    50
}
fn error(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(super::schemas::ErrorResponse {
            error: message.into(),
        }),
    )
        .into_response()
}
#[cfg(feature = "bus")]
async fn load(
    state: &AppState,
    identity: &crate::auth::identity::CallerIdentity,
    ns: &str,
    tenant: &str,
    id: &str,
    operation: super::bus::BusOp,
) -> Result<acteon_bus::stage::StreamStageOperator, Response> {
    super::bus::authorize_bus_op(identity, tenant, ns, operation)?;
    // State stores encode scope components with ':' separators. Never permit
    // an authorized spelling to alias another namespace/tenant's durable row.
    if [ns, tenant]
        .iter()
        .any(|s| s.trim().is_empty() || s.contains(':') || s.chars().any(char::is_control))
        || id.trim().is_empty()
        || id.len() > 4096
        || id.chars().any(char::is_control)
    {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid stage scope or processor ID",
        ));
    }
    let store = state.gateway.read().await.state_store().clone();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        acteon_bus::stage::StreamStageOperator::load(
            store,
            acteon_bus::stream_checkpoint_key(ns, tenant, id),
        ),
    )
    .await
    .map_err(|_| error(StatusCode::GATEWAY_TIMEOUT, "stage storage timeout"))?
    .map_err(|e| match e {
        acteon_bus::StreamStageError::MissingManagedState => {
            error(StatusCode::CONFLICT, "checkpoint is not a managed stage")
        }
        _ => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "cannot load stage checkpoint",
        ),
    })?
    .ok_or_else(|| error(StatusCode::NOT_FOUND, "stage not found"))
}
#[utoipa::path(get, path="/v1/bus/stages/{namespace}/{tenant}/{id}", tag="bus", params(("namespace"=String, Path, description="Stage namespace"),("tenant"=String, Path, description="Stage tenant"),("id"=String, Path, description="Processor ID")), responses((status=200, description="Durable stage metrics", body=serde_json::Value),(status=403, description="Forbidden"),(status=404, description="Stage not found")))]
pub async fn status(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<
        crate::auth::identity::CallerIdentity,
    >,
    Path((ns, tenant, id)): Path<(String, String, String)>,
) -> Response {
    #[cfg(feature = "bus")]
    {
        match load(
            &state,
            &identity,
            &ns,
            &tenant,
            &id,
            super::bus::BusOp::StageRead,
        )
        .await
        {
            Ok(op) => Json(op.status()).into_response(),
            Err(e) => e,
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, ns, tenant, id);
        error(StatusCode::SERVICE_UNAVAILABLE, "bus feature disabled")
    }
}
#[utoipa::path(get, path="/v1/bus/stages/{namespace}/{tenant}/{id}/quarantine", tag="bus", params(("namespace"=String, Path, description="Stage namespace"),("tenant"=String, Path, description="Stage tenant"),("id"=String, Path, description="Processor ID"),("limit"=Option<usize>, Query, description="Page size 1..100, default 50"),("after"=Option<String>, Query, description="Last retained input UUID from previous page")), responses((status=200, description="Bounded metadata page without message payloads", body=serde_json::Value)))]
pub async fn list(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<
        crate::auth::identity::CallerIdentity,
    >,
    Path((ns, tenant, id)): Path<(String, String, String)>,
    Query(page): Query<Page>,
) -> Response {
    if !(1..=100).contains(&page.limit) {
        return error(StatusCode::BAD_REQUEST, "limit must be 1..=100");
    }
    #[cfg(feature = "bus")]
    {
        let op = match load(
            &state,
            &identity,
            &ns,
            &tenant,
            &id,
            super::bus::BusOp::StageRead,
        )
        .await
        {
            Ok(op) => op,
            Err(e) => return e,
        };
        let inputs = op.quarantined_inputs();
        let start = match page.after {
            None => 0,
            Some(id) => match inputs.iter().position(|e| e.id == id) {
                Some(i) => i + 1,
                None => {
                    return error(
                        StatusCode::CONFLICT,
                        "cursor no longer retained; restart listing",
                    );
                }
            },
        };
        let entries: Vec<_> = inputs.iter().skip(start).take(page.limit).map(|e| serde_json::json!({"id":e.id,"position":e.position,"failed_at":e.failed_at,"failure":e.failure,"contract_sha256":e.contract_sha256,"reason":e.reason})).collect();
        let next = if start + entries.len() < inputs.len() {
            entries.last().and_then(|e| e["id"].as_str())
        } else {
            None
        };
        Json(serde_json::json!({"entries":entries,"next_after":next,"checkpoint_generation":op.status().checkpoint_generation})).into_response()
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, ns, tenant, id, page);
        error(StatusCode::SERVICE_UNAVAILABLE, "bus feature disabled")
    }
}
#[utoipa::path(get, path="/v1/bus/stages/{namespace}/{tenant}/{id}/quarantine/{entry}", tag="bus", params(("namespace"=String, Path, description="Stage namespace"),("tenant"=String, Path, description="Stage tenant"),("id"=String, Path, description="Processor ID"),("entry"=String, Path, description="Quarantined input UUID")), responses((status=200, description="Retained original input; may contain sensitive telemetry", body=serde_json::Value),(status=404, description="Not retained")))]
pub async fn get(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<
        crate::auth::identity::CallerIdentity,
    >,
    Path((ns, tenant, id, entry)): Path<(String, String, String, String)>,
) -> Response {
    #[cfg(feature = "bus")]
    {
        match load(
            &state,
            &identity,
            &ns,
            &tenant,
            &id,
            super::bus::BusOp::StageRead,
        )
        .await
        {
            Ok(op) => match op.quarantined_inputs().iter().find(|e| e.id == entry) {
                Some(e) => Json(e).into_response(),
                None => error(StatusCode::NOT_FOUND, "input not retained"),
            },
            Err(e) => e,
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, ns, tenant, id, entry);
        error(StatusCode::SERVICE_UNAVAILABLE, "bus feature disabled")
    }
}
#[utoipa::path(delete, path="/v1/bus/stages/{namespace}/{tenant}/{id}/quarantine/{entry}", tag="bus", params(("namespace"=String, Path, description="Stage namespace"),("tenant"=String, Path, description="Stage tenant"),("id"=String, Path, description="Processor ID"),("entry"=String, Path, description="Quarantined input UUID")), responses((status=200, description="Idempotent explicit discard result", body=serde_json::Value)))]
pub async fn discard(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<
        crate::auth::identity::CallerIdentity,
    >,
    Path((ns, tenant, id, entry)): Path<(String, String, String, String)>,
) -> Response {
    #[cfg(feature = "bus")]
    {
        let mut op = match load(
            &state,
            &identity,
            &ns,
            &tenant,
            &id,
            super::bus::BusOp::StageManage,
        )
        .await
        {
            Ok(op) => op,
            Err(e) => return e,
        };
        match tokio::time::timeout(std::time::Duration::from_secs(5), op.discard(&entry)).await {
            Ok(Ok(discarded)) => Json(serde_json::json!({"discarded":discarded})).into_response(),
            Ok(Err(
                acteon_bus::stage::StreamStageError::Fenced
                | acteon_bus::stage::StreamStageError::ReplayConflict,
            )) => error(
                StatusCode::CONFLICT,
                "pending replay or concurrent checkpoint updates",
            ),
            Ok(Err(_)) => error(StatusCode::INTERNAL_SERVER_ERROR, "cannot discard input"),
            Err(_) => error(
                StatusCode::GATEWAY_TIMEOUT,
                "stage storage timeout; inspect before retry",
            ),
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, ns, tenant, id, entry);
        error(StatusCode::SERVICE_UNAVAILABLE, "bus feature disabled")
    }
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplayRequest {
    #[schema(value_type = String)]
    pub request_id: uuid::Uuid,
    pub reason: String,
    pub payload: serde_json::Value,
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ControlRequest {
    #[schema(value_type = String)]
    pub request_id: uuid::Uuid,
    /// `halt` or `resume`.
    pub command: String,
    pub expected_control_revision: u64,
    pub reason: String,
    #[serde(default)]
    pub reset_retry_budget: bool,
}
#[utoipa::path(post, path="/v1/bus/stages/{namespace}/{tenant}/{id}/control", tag="bus", params(("namespace"=String, Path, description="Stage namespace"),("tenant"=String, Path, description="Stage tenant"),("id"=String, Path, description="Processor ID")), request_body=ControlRequest, responses((status=200, description="Committed control audit", body=serde_json::Value),(status=409, description="Stale revision or conflicting request"),(status=429, description="Audit retention full")))]
pub async fn control(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<
        crate::auth::identity::CallerIdentity,
    >,
    Path((ns, tenant, id)): Path<(String, String, String)>,
    Json(request): Json<ControlRequest>,
) -> Response {
    #[cfg(feature = "bus")]
    {
        let mut op = match load(
            &state,
            &identity,
            &ns,
            &tenant,
            &id,
            super::bus::BusOp::StageControl,
        )
        .await
        {
            Ok(op) => op,
            Err(e) => return e,
        };
        let command = match request.command.as_str() {
            "halt" => acteon_bus::StreamStageCommand::Halt,
            "resume" => acteon_bus::StreamStageCommand::Resume,
            _ => return error(StatusCode::BAD_REQUEST, "unknown control command"),
        };
        let request = acteon_bus::StreamStageControlRequest {
            request_id: request.request_id,
            command,
            expected_control_revision: request.expected_control_revision,
            reason: request.reason,
            reset_retry_budget: request.reset_retry_budget,
        };
        let actor = format!("{}:{}", identity.auth_method, identity.id);
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            op.control(&actor, &request),
        )
        .await
        {
            Ok(Ok(audit)) => Json(audit).into_response(),
            Ok(Err(acteon_bus::StreamStageError::InvalidConfig(_))) => {
                error(StatusCode::BAD_REQUEST, "invalid control request")
            }
            Ok(Err(acteon_bus::StreamStageError::ControlConflict)) => error(
                StatusCode::CONFLICT,
                "stale control revision or conflicting request",
            ),
            Ok(Err(acteon_bus::StreamStageError::ControlCapacity)) => error(
                StatusCode::TOO_MANY_REQUESTS,
                "control audit retention capacity reached",
            ),
            Ok(Err(_)) => error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot commit control; retry the same request ID",
            ),
            Err(_) => error(
                StatusCode::GATEWAY_TIMEOUT,
                "storage timeout; retry the same control request ID",
            ),
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, ns, tenant, id, request);
        error(StatusCode::SERVICE_UNAVAILABLE, "bus feature disabled")
    }
}
#[utoipa::path(get, path="/v1/bus/stages/{namespace}/{tenant}/{id}/controls/{request}", tag="bus", params(("namespace"=String, Path, description="Stage namespace"),("tenant"=String, Path, description="Stage tenant"),("id"=String, Path, description="Processor ID"),("request"=String, Path, description="Control request UUID")), responses((status=200, description="Control audit", body=serde_json::Value),(status=404, description="Unknown request")))]
pub async fn control_audit(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<
        crate::auth::identity::CallerIdentity,
    >,
    Path((ns, tenant, id, request)): Path<(String, String, String, uuid::Uuid)>,
) -> Response {
    #[cfg(feature = "bus")]
    {
        match load(
            &state,
            &identity,
            &ns,
            &tenant,
            &id,
            super::bus::BusOp::StageRead,
        )
        .await
        {
            Ok(op) => match op
                .control_audits()
                .iter()
                .find(|a| a.request.request_id == request)
            {
                Some(a) => Json(a).into_response(),
                None => error(StatusCode::NOT_FOUND, "control request not found"),
            },
            Err(e) => e,
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, ns, tenant, id, request);
        error(StatusCode::SERVICE_UNAVAILABLE, "bus feature disabled")
    }
}
#[utoipa::path(post, path="/v1/bus/stages/{namespace}/{tenant}/{id}/quarantine/{entry}/replay", tag="bus", params(("namespace"=String, Path, description="Stage namespace"),("tenant"=String, Path, description="Stage tenant"),("id"=String, Path, description="Processor ID"),("entry"=String, Path, description="Quarantined input UUID")), request_body=ReplayRequest, responses((status=202, description="Durable replay audit", body=serde_json::Value),(status=409, description="Conflicting request")))]
pub async fn replay(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<
        crate::auth::identity::CallerIdentity,
    >,
    Path((ns, tenant, id, entry)): Path<(String, String, String, String)>,
    Json(request): Json<ReplayRequest>,
) -> Response {
    #[cfg(feature = "bus")]
    {
        let mut op = match load(
            &state,
            &identity,
            &ns,
            &tenant,
            &id,
            super::bus::BusOp::StageReplay,
        )
        .await
        {
            Ok(op) => op,
            Err(e) => return e,
        };
        let actor = format!("{}:{}", identity.auth_method, identity.id);
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            op.request_replay(
                request.request_id,
                &entry,
                &actor,
                &request.reason,
                request.payload,
            ),
        )
        .await
        {
            Ok(Ok(audit)) => (StatusCode::ACCEPTED, Json(audit)).into_response(),
            Ok(Err(acteon_bus::StreamStageError::InvalidConfig(_))) => error(
                StatusCode::BAD_REQUEST,
                "invalid repair, request ID or reason",
            ),
            Ok(Err(acteon_bus::StreamStageError::ReplayCapacity)) => error(
                StatusCode::TOO_MANY_REQUESTS,
                "replay audit retention capacity reached",
            ),
            Ok(Err(
                acteon_bus::StreamStageError::ReplayConflict | acteon_bus::StreamStageError::Fenced,
            )) => error(
                StatusCode::CONFLICT,
                "replay request conflicts with durable state",
            ),
            Ok(Err(_)) => error(StatusCode::INTERNAL_SERVER_ERROR, "cannot enqueue replay"),
            Err(_) => error(
                StatusCode::GATEWAY_TIMEOUT,
                "storage timeout; retry the same replay request ID",
            ),
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, ns, tenant, id, entry, request);
        error(StatusCode::SERVICE_UNAVAILABLE, "bus feature disabled")
    }
}
#[utoipa::path(get, path="/v1/bus/stages/{namespace}/{tenant}/{id}/replays/{request}", tag="bus", params(("namespace"=String, Path, description="Stage namespace"),("tenant"=String, Path, description="Stage tenant"),("id"=String, Path, description="Processor ID"),("request"=String, Path, description="Replay request UUID")), responses((status=200, description="Replay audit without payload", body=serde_json::Value),(status=404, description="Unknown request")))]
pub async fn replay_audit(
    State(state): State<AppState>,
    #[cfg(feature = "bus")] axum::Extension(identity): axum::Extension<
        crate::auth::identity::CallerIdentity,
    >,
    Path((ns, tenant, id, request)): Path<(String, String, String, uuid::Uuid)>,
) -> Response {
    #[cfg(feature = "bus")]
    {
        match load(
            &state,
            &identity,
            &ns,
            &tenant,
            &id,
            super::bus::BusOp::StageRead,
        )
        .await
        {
            Ok(op) => match op
                .replay_audits()
                .into_iter()
                .find(|a| a.request_id == request)
            {
                Some(a) => Json(a).into_response(),
                None => error(StatusCode::NOT_FOUND, "replay request not found"),
            },
            Err(e) => e,
        }
    }
    #[cfg(not(feature = "bus"))]
    {
        let _ = (state, ns, tenant, id, request);
        error(StatusCode::SERVICE_UNAVAILABLE, "bus feature disabled")
    }
}
