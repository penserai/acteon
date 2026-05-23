use serde::{Deserialize, Serialize};

#[cfg(feature = "utoipa")]
use utoipa::ToSchema;

/// Summary of a single circuit breaker's current state and configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
pub struct CircuitBreakerStatus {
    /// Provider name.
    #[cfg_attr(feature = "utoipa", schema(example = "email"))]
    pub provider: String,
    /// Current circuit state ("closed", "open", "`half_open`").
    #[cfg_attr(feature = "utoipa", schema(example = "closed"))]
    pub state: String,
    /// Number of consecutive failures before opening the circuit.
    #[cfg_attr(feature = "utoipa", schema(example = 5))]
    pub failure_threshold: u32,
    /// Number of consecutive successes in half-open state to close the circuit.
    #[cfg_attr(feature = "utoipa", schema(example = 2))]
    pub success_threshold: u32,
    /// Recovery timeout in seconds.
    #[cfg_attr(feature = "utoipa", schema(example = 60))]
    pub recovery_timeout_seconds: u64,
    /// Optional fallback provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "utoipa", schema(example = "sms"))]
    pub fallback_provider: Option<String>,
}

/// Response for listing all circuit breakers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
pub struct ListCircuitBreakersResponse {
    /// List of circuit breaker statuses.
    pub circuit_breakers: Vec<CircuitBreakerStatus>,
}

/// Response after tripping or resetting a circuit breaker.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
pub struct CircuitBreakerActionResponse {
    /// Provider name.
    #[cfg_attr(feature = "utoipa", schema(example = "email"))]
    pub provider: String,
    /// New circuit state after the action.
    #[cfg_attr(feature = "utoipa", schema(example = "open"))]
    pub state: String,
    /// Human-readable status message.
    #[cfg_attr(feature = "utoipa", schema(example = "circuit breaker tripped"))]
    pub message: String,
}
