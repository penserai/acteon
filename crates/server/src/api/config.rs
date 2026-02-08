use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;

use super::AppState;
use crate::config::ConfigSnapshot;

/// `GET /admin/config` -- returns a sanitized view of the server configuration.
///
/// All secrets (API keys, HMAC secrets, approval keys) are masked so the
/// response is safe to display in admin dashboards.
#[allow(clippy::unused_async)]
pub async fn get_config(State(state): State<AppState>) -> impl IntoResponse {
    (StatusCode::OK, Json::<ConfigSnapshot>(state.config.clone()))
}
