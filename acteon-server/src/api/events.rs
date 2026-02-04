//! Event state API endpoints.
//!
//! Provides endpoints for querying and managing event lifecycle states.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use acteon_state::{KeyKind, StateKey};

use crate::auth::identity::CallerIdentity;
use crate::auth::role::Permission;
use crate::error::ServerError;

use super::AppState;
use super::schemas::ErrorResponse;

/// Response for getting an event's state.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EventStateResponse {
    /// The event fingerprint.
    #[schema(example = "abc123")]
    pub fingerprint: String,
    /// Current state of the event.
    #[schema(example = "open")]
    pub state: String,
    /// The action type that created this event.
    #[schema(example = "alert")]
    pub action_type: Option<String>,
    /// When the state was last updated.
    #[schema(example = "2024-01-15T10:30:00Z")]
    pub updated_at: Option<String>,
}

/// Request body for transitioning an event to a new state.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TransitionRequest {
    /// The target state to transition to.
    #[schema(example = "investigating")]
    pub to: String,
    /// Namespace for the event.
    #[schema(example = "notifications")]
    pub namespace: String,
    /// Tenant for the event.
    #[schema(example = "tenant-1")]
    pub tenant: String,
}

/// Response after transitioning an event.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TransitionResponse {
    /// The event fingerprint.
    #[schema(example = "abc123")]
    pub fingerprint: String,
    /// The previous state.
    #[schema(example = "open")]
    pub previous_state: String,
    /// The new state.
    #[schema(example = "investigating")]
    pub new_state: String,
    /// Whether the transition triggered a notification.
    #[schema(example = true)]
    pub notify: bool,
}

/// Query parameters for listing events.
#[derive(Debug, Deserialize, ToSchema)]
pub struct EventQueryParams {
    /// Filter by namespace.
    pub namespace: String,
    /// Filter by tenant.
    pub tenant: String,
    /// Filter by state (e.g., "open", "closed").
    #[allow(dead_code)]
    pub status: Option<String>,
    /// Maximum number of results to return.
    #[serde(default = "default_limit")]
    #[allow(dead_code)]
    pub limit: usize,
}

fn default_limit() -> usize {
    100
}

/// `GET /v1/events/{fingerprint}` -- get the current state of an event.
#[utoipa::path(
    get,
    path = "/v1/events/{fingerprint}",
    tag = "Events",
    summary = "Get event state",
    description = "Retrieves the current lifecycle state of an event by its fingerprint.",
    params(
        ("fingerprint" = String, Path, description = "Event fingerprint"),
        ("namespace" = String, Query, description = "Event namespace"),
        ("tenant" = String, Query, description = "Event tenant"),
    ),
    responses(
        (status = 200, description = "Event state retrieved", body = EventStateResponse),
        (status = 404, description = "Event not found", body = ErrorResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse),
    )
)]
pub async fn get_event(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(fingerprint): Path<String>,
    Query(params): Query<EventQueryParams>,
) -> Result<impl IntoResponse, ServerError> {
    // Check role permission.
    if !identity.role.has_permission(Permission::AuditRead) {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!(ErrorResponse {
                error: "insufficient permissions".into(),
            })),
        ));
    }

    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    let state_key = StateKey::new(
        params.namespace.as_str(),
        params.tenant.as_str(),
        KeyKind::EventState,
        &fingerprint,
    );

    match state_store.get(&state_key).await {
        Ok(Some(value)) => {
            let parsed: serde_json::Value =
                serde_json::from_str(&value).unwrap_or(serde_json::json!({"state": value}));

            let response = EventStateResponse {
                fingerprint,
                state: parsed
                    .get("state")
                    .and_then(|s| s.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                action_type: parsed
                    .get("action_type")
                    .and_then(|s| s.as_str())
                    .map(String::from),
                updated_at: parsed
                    .get("updated_at")
                    .and_then(|s| s.as_str())
                    .map(String::from),
            };

            Ok((StatusCode::OK, Json(serde_json::json!(response))))
        }
        Ok(None) => Ok((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                error: format!("event not found: {fingerprint}"),
            })),
        )),
        Err(e) => Ok((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse {
                error: e.to_string(),
            })),
        )),
    }
}

/// `PUT /v1/events/{fingerprint}/transition` -- transition an event to a new state.
#[utoipa::path(
    put,
    path = "/v1/events/{fingerprint}/transition",
    tag = "Events",
    summary = "Transition event state",
    description = "Transitions an event to a new lifecycle state.",
    params(
        ("fingerprint" = String, Path, description = "Event fingerprint"),
    ),
    request_body(content = TransitionRequest, description = "Transition details"),
    responses(
        (status = 200, description = "Event transitioned", body = TransitionResponse),
        (status = 400, description = "Invalid transition", body = ErrorResponse),
        (status = 404, description = "Event not found", body = ErrorResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse),
    )
)]
pub async fn transition_event(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(fingerprint): Path<String>,
    Json(request): Json<TransitionRequest>,
) -> Result<impl IntoResponse, ServerError> {
    // Check role permission.
    if !identity.role.has_permission(Permission::Dispatch) {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!(ErrorResponse {
                error: "insufficient permissions".into(),
            })),
        ));
    }

    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    let state_key = StateKey::new(
        request.namespace.as_str(),
        request.tenant.as_str(),
        KeyKind::EventState,
        &fingerprint,
    );

    // Get current state
    let current_state = match state_store.get(&state_key).await {
        Ok(Some(value)) => {
            let parsed: serde_json::Value =
                serde_json::from_str(&value).unwrap_or(serde_json::json!({"state": value}));
            parsed
                .get("state")
                .and_then(|s| s.as_str())
                .unwrap_or("unknown")
                .to_string()
        }
        Ok(None) => {
            return Ok((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!(ErrorResponse {
                    error: format!("event not found: {fingerprint}"),
                })),
            ));
        }
        Err(e) => {
            return Ok((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse {
                    error: e.to_string(),
                })),
            ));
        }
    };

    // Update state
    let new_state_value = serde_json::json!({
        "state": &request.to,
        "fingerprint": &fingerprint,
        "updated_at": chrono::Utc::now().to_rfc3339(),
    });

    if let Err(e) = state_store
        .set(&state_key, &new_state_value.to_string(), None)
        .await
    {
        return Ok((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse {
                error: e.to_string(),
            })),
        ));
    }

    let response = TransitionResponse {
        fingerprint,
        previous_state: current_state,
        new_state: request.to,
        notify: false, // Could be determined by state machine config
    };

    Ok((StatusCode::OK, Json(serde_json::json!(response))))
}
