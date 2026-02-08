use axum::Json;
use axum::extract::{self, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use tracing::info;
use utoipa::ToSchema;

use crate::auth::identity::CallerIdentity;
use crate::auth::role::Permission;

use super::AppState;
use super::schemas::ErrorResponse;

/// Summary of a single circuit breaker's current state and configuration.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CircuitBreakerStatus {
    /// Provider name.
    #[schema(example = "email")]
    pub provider: String,
    /// Current circuit state.
    #[schema(example = "closed")]
    pub state: String,
    /// Number of consecutive failures before opening the circuit.
    #[schema(example = 5)]
    pub failure_threshold: u32,
    /// Number of consecutive successes in half-open state to close the circuit.
    #[schema(example = 2)]
    pub success_threshold: u32,
    /// Recovery timeout in seconds.
    #[schema(example = 60)]
    pub recovery_timeout_seconds: u64,
    /// Optional fallback provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "sms")]
    pub fallback_provider: Option<String>,
}

/// Response for listing all circuit breakers.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ListCircuitBreakersResponse {
    /// List of circuit breaker statuses.
    pub circuit_breakers: Vec<CircuitBreakerStatus>,
}

/// Response after tripping or resetting a circuit breaker.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CircuitBreakerActionResponse {
    /// Provider name.
    #[schema(example = "email")]
    pub provider: String,
    /// New circuit state after the action.
    #[schema(example = "open")]
    pub state: String,
    /// Human-readable status message.
    #[schema(example = "circuit breaker tripped")]
    pub message: String,
}

/// `GET /admin/circuit-breakers` -- list all circuit breakers with current state.
#[utoipa::path(
    get,
    path = "/admin/circuit-breakers",
    tag = "Circuit Breakers",
    summary = "List circuit breakers",
    description = "Returns all registered circuit breakers with their current state and configuration.",
    responses(
        (status = 200, description = "List of circuit breakers", body = ListCircuitBreakersResponse),
        (status = 404, description = "Circuit breakers not enabled", body = ErrorResponse)
    )
)]
pub async fn list_circuit_breakers(
    State(state): State<AppState>,
    axum::Extension(_identity): axum::Extension<CallerIdentity>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let Some(registry) = gw.circuit_breakers() else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                error: "circuit breakers are not enabled".into(),
            })),
        );
    };

    let mut breakers = Vec::new();
    for name in registry.providers() {
        if let Some(cb) = registry.get(name) {
            let current_state = cb.state().await;
            let config = cb.config();
            breakers.push(CircuitBreakerStatus {
                provider: name.to_owned(),
                state: current_state.to_string(),
                failure_threshold: config.failure_threshold,
                success_threshold: config.success_threshold,
                recovery_timeout_seconds: config.recovery_timeout.as_secs(),
                fallback_provider: config.fallback_provider.clone(),
            });
        }
    }

    (
        StatusCode::OK,
        Json(serde_json::json!(ListCircuitBreakersResponse {
            circuit_breakers: breakers,
        })),
    )
}

/// `POST /admin/circuit-breakers/{provider}/trip` -- force a circuit breaker open.
#[utoipa::path(
    post,
    path = "/admin/circuit-breakers/{provider}/trip",
    tag = "Circuit Breakers",
    summary = "Trip circuit breaker",
    description = "Force-opens a circuit breaker, immediately rejecting all requests to the provider.",
    params(
        ("provider" = String, Path, description = "Provider name")
    ),
    responses(
        (status = 200, description = "Circuit breaker tripped", body = CircuitBreakerActionResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse),
        (status = 404, description = "Circuit breaker not found", body = ErrorResponse)
    )
)]
pub async fn trip_circuit_breaker(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    extract::Path(provider): extract::Path<String>,
) -> impl IntoResponse {
    if !identity
        .role
        .has_permission(Permission::CircuitBreakerManage)
    {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!(ErrorResponse {
                error: "insufficient permissions: circuit breaker management requires admin or operator role".into(),
            })),
        );
    }

    let gw = state.gateway.read().await;
    let Some(registry) = gw.circuit_breakers() else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                error: "circuit breakers are not enabled".into(),
            })),
        );
    };

    let Some(cb) = registry.get(&provider) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                error: format!("circuit breaker not found: {provider}"),
            })),
        );
    };

    cb.trip().await;
    info!(provider = %provider, "circuit breaker manually tripped");

    (
        StatusCode::OK,
        Json(serde_json::json!(CircuitBreakerActionResponse {
            provider,
            state: "open".into(),
            message: "circuit breaker tripped".into(),
        })),
    )
}

/// `POST /admin/circuit-breakers/{provider}/reset` -- force a circuit breaker closed.
#[utoipa::path(
    post,
    path = "/admin/circuit-breakers/{provider}/reset",
    tag = "Circuit Breakers",
    summary = "Reset circuit breaker",
    description = "Force-closes a circuit breaker, restoring normal request flow to the provider.",
    params(
        ("provider" = String, Path, description = "Provider name")
    ),
    responses(
        (status = 200, description = "Circuit breaker reset", body = CircuitBreakerActionResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse),
        (status = 404, description = "Circuit breaker not found", body = ErrorResponse)
    )
)]
pub async fn reset_circuit_breaker(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    extract::Path(provider): extract::Path<String>,
) -> impl IntoResponse {
    if !identity
        .role
        .has_permission(Permission::CircuitBreakerManage)
    {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!(ErrorResponse {
                error: "insufficient permissions: circuit breaker management requires admin or operator role".into(),
            })),
        );
    }

    let gw = state.gateway.read().await;
    let Some(registry) = gw.circuit_breakers() else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                error: "circuit breakers are not enabled".into(),
            })),
        );
    };

    let Some(cb) = registry.get(&provider) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                error: format!("circuit breaker not found: {provider}"),
            })),
        );
    };

    cb.reset().await;
    info!(provider = %provider, "circuit breaker manually reset");

    (
        StatusCode::OK,
        Json(serde_json::json!(CircuitBreakerActionResponse {
            provider,
            state: "closed".into(),
            message: "circuit breaker reset".into(),
        })),
    )
}
