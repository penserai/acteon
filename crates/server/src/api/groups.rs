//! Event group API endpoints.
//!
//! Provides endpoints for querying and managing event groups.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use acteon_core::EventGroup;

use crate::auth::identity::CallerIdentity;
use crate::auth::role::Permission;
use crate::error::ServerError;

use super::AppState;
use super::schemas::ErrorResponse;

/// Summary of an event group.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct GroupSummary {
    /// Unique identifier for the group.
    #[schema(example = "group-abc123")]
    pub group_id: String,
    /// Hash key used to group events.
    #[schema(example = "hash-xyz")]
    pub group_key: String,
    /// Number of events in the group.
    #[schema(example = 5)]
    pub event_count: usize,
    /// Current state of the group.
    #[schema(example = "pending")]
    pub state: String,
    /// When the group will be flushed.
    #[schema(example = "2024-01-15T10:30:00Z")]
    pub notify_at: String,
    /// When the group was created.
    #[schema(example = "2024-01-15T10:00:00Z")]
    pub created_at: String,
}

impl From<&EventGroup> for GroupSummary {
    fn from(group: &EventGroup) -> Self {
        Self {
            group_id: group.group_id.clone(),
            group_key: group.group_key.clone(),
            event_count: group.size(),
            state: format!("{:?}", group.state).to_lowercase(),
            notify_at: group.notify_at.to_rfc3339(),
            created_at: group.created_at.to_rfc3339(),
        }
    }
}

/// Response for listing groups.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ListGroupsResponse {
    /// List of groups.
    pub groups: Vec<GroupSummary>,
    /// Total number of active groups.
    #[schema(example = 10)]
    pub total: usize,
}

/// Response for getting a single group.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct GroupDetailResponse {
    /// The group details.
    pub group: GroupSummary,
    /// Common labels for all events in the group.
    pub labels: std::collections::HashMap<String, String>,
    /// Event fingerprints in this group.
    #[serde(rename = "events")]
    pub event_fingerprints: Vec<String>,
}

/// Response after flushing/deleting a group.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct FlushGroupResponse {
    /// The group ID that was flushed.
    #[schema(example = "group-abc123")]
    pub group_id: String,
    /// Number of events in the flushed group.
    #[schema(example = 5)]
    pub event_count: usize,
    /// Status message.
    #[schema(example = "flushed")]
    pub status: String,
}

/// `GET /v1/groups` -- list all active event groups.
#[utoipa::path(
    get,
    path = "/v1/groups",
    tag = "Groups",
    summary = "List groups",
    description = "Lists all active event groups awaiting notification.",
    responses(
        (status = 200, description = "List of groups", body = ListGroupsResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse),
    )
)]
pub async fn list_groups(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
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
    let group_manager = gw.group_manager();

    let pending = group_manager.list_pending_groups();
    let total = group_manager.active_group_count();

    let groups: Vec<GroupSummary> = pending.iter().map(GroupSummary::from).collect();

    let response = ListGroupsResponse { groups, total };

    Ok((StatusCode::OK, Json(serde_json::json!(response))))
}

/// `GET /v1/groups/{group_key}` -- get details of a specific group.
#[utoipa::path(
    get,
    path = "/v1/groups/{group_key}",
    tag = "Groups",
    summary = "Get group",
    description = "Retrieves details of a specific event group by its key.",
    params(
        ("group_key" = String, Path, description = "Group key (hash)"),
    ),
    responses(
        (status = 200, description = "Group details", body = GroupDetailResponse),
        (status = 404, description = "Group not found", body = ErrorResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse),
    )
)]
pub async fn get_group(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(group_key): Path<String>,
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
    let group_manager = gw.group_manager();

    match group_manager.get_group(&group_key) {
        Some(group) => {
            let event_fingerprints: Vec<String> = group
                .events
                .iter()
                .filter_map(|e| e.fingerprint.clone())
                .collect();

            let response = GroupDetailResponse {
                group: GroupSummary::from(&group),
                labels: group.labels.clone(),
                event_fingerprints,
            };

            Ok((StatusCode::OK, Json(serde_json::json!(response))))
        }
        None => Ok((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                error: format!("group not found: {group_key}"),
            })),
        )),
    }
}

/// `DELETE /v1/groups/{group_key}` -- force flush/close a group.
#[utoipa::path(
    delete,
    path = "/v1/groups/{group_key}",
    tag = "Groups",
    summary = "Flush group",
    description = "Force flushes a group, marking it as notified and removing it from active groups.",
    params(
        ("group_key" = String, Path, description = "Group key (hash)"),
    ),
    responses(
        (status = 200, description = "Group flushed", body = FlushGroupResponse),
        (status = 404, description = "Group not found", body = ErrorResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse),
    )
)]
pub async fn flush_group(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path(group_key): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    // Check role permission (requires dispatch/write permissions).
    if !identity.role.has_permission(Permission::Dispatch) {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!(ErrorResponse {
                error: "insufficient permissions".into(),
            })),
        ));
    }

    let gw = state.gateway.read().await;
    let group_manager = gw.group_manager();

    // Try to flush the group
    match group_manager.flush_group(&group_key) {
        Some(flushed_group) => {
            let event_count = flushed_group.size();
            let group_id = flushed_group.group_id.clone();

            // Remove the group after flushing
            group_manager.remove_group(&group_key);

            let response = FlushGroupResponse {
                group_id,
                event_count,
                status: "flushed".to_string(),
            };

            Ok((StatusCode::OK, Json(serde_json::json!(response))))
        }
        None => {
            // Check if group exists but is already notified
            if group_manager.get_group(&group_key).is_some() {
                Ok((
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!(ErrorResponse {
                        error: "group already notified or not in pending state".into(),
                    })),
                ))
            } else {
                Ok((
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!(ErrorResponse {
                        error: format!("group not found: {group_key}"),
                    })),
                ))
            }
        }
    }
}
