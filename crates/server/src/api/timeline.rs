//! Tenant-scoped timeline ingestion and queries.
use super::AppState;
use crate::auth::identity::CallerIdentity;
use acteon_core::timeline::{TimelineChange, TimelineQuery, TimelineScope};
use acteon_timeline::TimelineService;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct IngestRequest {
    pub scope: TimelineScope,
    pub change: TimelineChange,
}
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BatchRequest {
    pub scope: TimelineScope,
    pub changes: Vec<TimelineChange>,
}
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryRequest {
    pub scope: TimelineScope,
    pub query: TimelineQuery,
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReplayStatus {
    pub scope: TimelineScope,
    pub id: String,
    pub status: String,
    pub processed: u64,
    pub error: Option<String>,
}

pub struct ApiError {
    status: StatusCode,
    message: String,
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({"error": self.message})),
        )
            .into_response()
    }
}
type ApiResult = Result<Response, ApiError>;
#[allow(clippy::needless_pass_by_value)]
fn error(e: acteon_timeline::Error) -> ApiError {
    let status = match e {
        acteon_timeline::Error::Invalid(_) | acteon_timeline::Error::Json(_) => {
            StatusCode::BAD_REQUEST
        }
        acteon_timeline::Error::Conflict => StatusCode::CONFLICT,
        acteon_timeline::Error::Capacity => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    let message = if status == StatusCode::SERVICE_UNAVAILABLE {
        tracing::error!(error=%e,"timeline request failed");
        "timeline storage unavailable".into()
    } else {
        e.to_string()
    };
    ApiError { status, message }
}
fn service(
    state: &AppState,
    caller: &CallerIdentity,
    scope: &TimelineScope,
) -> Result<Arc<TimelineService>, ApiError> {
    acteon_timeline::search::validate_scope(scope).map_err(error)?;
    if !caller.can_manage_scope(&scope.tenant, &scope.namespace) {
        return Err(ApiError {
            status: StatusCode::FORBIDDEN,
            message: "scope not authorized".into(),
        });
    }
    state.timeline.clone().ok_or_else(|| ApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        message: "timeline disabled".into(),
    })
}
#[utoipa::path(post,path="/v1/timeline/changes",tag="Timeline",request_body=IngestRequest,responses((status=200,body=acteon_core::timeline::ChangeReceipt)))]
pub async fn ingest(
    State(state): State<AppState>,
    axum::Extension(caller): axum::Extension<CallerIdentity>,
    Json(req): Json<IngestRequest>,
) -> ApiResult {
    let service = service(&state, &caller, &req.scope)?;
    Ok(Json(
        service
            .ingest(&req.scope, req.change)
            .await
            .map_err(error)?,
    )
    .into_response())
}
#[utoipa::path(post,path="/v1/timeline/changes/batch",tag="Timeline",request_body=BatchRequest,responses((status=200,description="Ordered per-item receipt or error")))]
pub async fn batch(
    State(state): State<AppState>,
    axum::Extension(caller): axum::Extension<CallerIdentity>,
    Json(req): Json<BatchRequest>,
) -> ApiResult {
    let service = service(&state, &caller, &req.scope)?;
    if req.changes.is_empty() || req.changes.len() > 500 {
        return Err(error(acteon_timeline::Error::Invalid(
            "batch size must be 1..500".into(),
        )));
    }
    let mut results = vec![];
    for change in req.changes {
        let result = match service.ingest(&req.scope, change).await {
            Ok(receipt) => serde_json::json!({"receipt":receipt}),
            Err(e) => {
                let response = error(e);
                serde_json::json!({"error":"change failed","status":response.status.as_u16()})
            }
        };
        results.push(result);
    }
    Ok(Json(serde_json::json!({"results":results})).into_response())
}
#[utoipa::path(post,path="/v1/timeline/query",tag="Timeline",request_body=QueryRequest,responses((status=200,body=acteon_core::timeline::TimelinePage)))]
pub async fn query(
    State(state): State<AppState>,
    axum::Extension(caller): axum::Extension<CallerIdentity>,
    Json(req): Json<QueryRequest>,
) -> ApiResult {
    let service = service(&state, &caller, &req.scope)?;
    Ok(Json(service.query(&req.scope, &req.query).await.map_err(error)?).into_response())
}
#[utoipa::path(get,path="/v1/timeline/events/{source}/{eventId}",tag="Timeline",params(("source"=String,Path),("eventId"=String,Path),("namespace"=String,Query),("tenant"=String,Query)),responses((status=200,body=acteon_core::timeline::TimelineEvent),(status=404)))]
pub async fn get(
    State(state): State<AppState>,
    axum::Extension(caller): axum::Extension<CallerIdentity>,
    Query(scope): Query<TimelineScope>,
    Path((source, id)): Path<(String, String)>,
) -> ApiResult {
    let service = service(&state, &caller, &scope)?;
    Ok(
        match service
            .store
            .get(&scope, &source, &id)
            .await
            .map_err(error)?
        {
            Some(event) => Json(event).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        },
    )
}
#[utoipa::path(get,path="/v1/timeline/ledger/status",tag="Timeline",params(("namespace"=String,Query),("tenant"=String,Query)),responses((status=200,description="Tenant ledger status")))]
pub async fn status(
    State(state): State<AppState>,
    axum::Extension(caller): axum::Extension<CallerIdentity>,
    Query(scope): Query<TimelineScope>,
) -> ApiResult {
    let service = service(&state, &caller, &scope)?;
    Ok(Json(service.ledger.status(&scope).await.map_err(error)?).into_response())
}
pub(crate) fn replay_key(scope: &TimelineScope, id: &str) -> acteon_state::StateKey {
    acteon_state::StateKey::new(
        scope.namespace.clone(),
        scope.tenant.clone(),
        acteon_state::KeyKind::Custom("timeline_replay".into()),
        id,
    )
}
#[utoipa::path(post,path="/v1/timeline/replays",tag="Timeline",request_body=TimelineScope,responses((status=202,body=ReplayStatus)))]
pub async fn replay(
    State(state): State<AppState>,
    axum::Extension(caller): axum::Extension<CallerIdentity>,
    Json(scope): Json<TimelineScope>,
) -> ApiResult {
    let service = service(&state, &caller, &scope)?;
    let job = ReplayStatus {
        scope: scope.clone(),
        id: uuid::Uuid::new_v4().to_string(),
        status: "running".into(),
        processed: 0,
        error: None,
    };
    let key = replay_key(&scope, &job.id);
    service
        .state
        .set(
            &key,
            &serde_json::to_string(&job).map_err(|e| error(e.into()))?,
            None,
        )
        .await
        .map_err(|e| error(e.into()))?;
    let id = job.id.clone();
    tokio::spawn(async move {
        let result = service.replay_local(&scope).await;
        let job = match result {
            Ok(processed) => ReplayStatus {
                scope: scope.clone(),
                id,
                status: "complete".into(),
                processed,
                error: None,
            },
            Err(e) => {
                tracing::error!(error=%e,"timeline replay failed");
                ReplayStatus {
                    scope: scope.clone(),
                    id,
                    status: "failed".into(),
                    processed: 0,
                    error: Some("replay failed; inspect server logs".into()),
                }
            }
        };
        if let Err(e) = service
            .state
            .set(
                &key,
                &serde_json::to_string(&job).expect("replay status"),
                None,
            )
            .await
        {
            tracing::error!(error=%e,"timeline replay status persistence failed");
        }
    });
    Ok((StatusCode::ACCEPTED, Json(job)).into_response())
}
#[utoipa::path(get,path="/v1/timeline/replays/{id}",tag="Timeline",params(("id"=String,Path),("namespace"=String,Query),("tenant"=String,Query)),responses((status=200,body=ReplayStatus),(status=404)))]
pub async fn replay_status(
    State(state): State<AppState>,
    axum::Extension(caller): axum::Extension<CallerIdentity>,
    Query(scope): Query<TimelineScope>,
    Path(id): Path<String>,
) -> ApiResult {
    let service = service(&state, &caller, &scope)?;
    let value = service
        .state
        .get(&replay_key(&scope, &id))
        .await
        .map_err(|e| error(e.into()))?;
    Ok(match value {
        Some(value) => {
            Json(serde_json::from_str::<ReplayStatus>(&value).map_err(|e| error(e.into()))?)
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    })
}
