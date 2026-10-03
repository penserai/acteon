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
    manage: bool,
) -> Result<acteon_bus::stage::StreamStageOperator, Response> {
    super::bus::authorize_bus_op(
        identity,
        tenant,
        ns,
        if manage {
            super::bus::BusOp::StageManage
        } else {
            super::bus::BusOp::StageRead
        },
    )?;
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
        match load(&state, &identity, &ns, &tenant, &id, false).await {
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
        let op = match load(&state, &identity, &ns, &tenant, &id, false).await {
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
        match load(&state, &identity, &ns, &tenant, &id, false).await {
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
        let mut op = match load(&state, &identity, &ns, &tenant, &id, true).await {
            Ok(op) => op,
            Err(e) => return e,
        };
        match tokio::time::timeout(std::time::Duration::from_secs(5), op.discard(&entry)).await {
            Ok(Ok(discarded)) => Json(serde_json::json!({"discarded":discarded})).into_response(),
            Ok(Err(acteon_bus::stage::StreamStageError::Fenced)) => {
                error(StatusCode::CONFLICT, "concurrent checkpoint updates; retry")
            }
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
