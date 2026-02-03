use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use utoipa::ToSchema;

use super::schemas::ErrorResponse;
use super::AppState;

#[derive(Deserialize, ToSchema)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// `POST /v1/auth/login` -- authenticate with username/password and receive a JWT.
#[utoipa::path(
    post,
    path = "/v1/auth/login",
    tag = "Auth",
    summary = "Login",
    description = "Authenticate with username and password to receive a JWT token.",
    request_body(content = LoginRequest, description = "Login credentials"),
    responses(
        (status = 200, description = "Login successful"),
        (status = 401, description = "Invalid credentials", body = ErrorResponse)
    )
)]
pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> impl IntoResponse {
    let Some(ref auth) = state.auth else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                error: "authentication is not enabled".into(),
            })),
        );
    };

    match auth.login(&body.username, &body.password).await {
        Ok((token, expires_in)) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "token": token,
                "expires_in": expires_in,
            })),
        ),
        Err(e) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!(ErrorResponse { error: e })),
        ),
    }
}

/// `POST /v1/auth/logout` -- revoke the current JWT token.
#[utoipa::path(
    post,
    path = "/v1/auth/logout",
    tag = "Auth",
    summary = "Logout",
    description = "Revoke the current JWT token, making it immediately invalid.",
    responses(
        (status = 200, description = "Logged out successfully"),
        (status = 401, description = "Invalid or missing token", body = ErrorResponse)
    )
)]
pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(ref auth) = state.auth else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                error: "authentication is not enabled".into(),
            })),
        );
    };

    // Extract Bearer token from Authorization header.
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));

    let Some(token) = token else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!(ErrorResponse {
                error: "missing Bearer token".into(),
            })),
        );
    };

    match auth.revoke_jwt(token).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "logged_out" })),
        ),
        Err(e) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!(ErrorResponse { error: e })),
        ),
    }
}
