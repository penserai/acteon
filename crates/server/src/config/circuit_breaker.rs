use std::collections::HashMap;

use serde::Deserialize;

/// Configuration for provider circuit breakers.
///
/// When enabled, circuit breakers track provider health and automatically
/// open the circuit when failure rates exceed the threshold, routing to
/// fallback providers during outages.
///
/// # Example
///
/// ```toml
/// [circuit_breaker]
/// enabled = true
/// failure_threshold = 5
/// success_threshold = 2
/// recovery_timeout_seconds = 60
///
/// [circuit_breaker.providers.email]
/// failure_threshold = 10
/// recovery_timeout_seconds = 120
/// fallback_provider = "webhook"
/// ```
#[derive(Debug, Deserialize)]
pub struct CircuitBreakerServerConfig {
    /// Whether circuit breakers are enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Default number of consecutive failures before opening the circuit.
    #[serde(default = "default_cb_failure_threshold")]
    pub failure_threshold: u32,
    /// Default number of consecutive successes in half-open state to close the circuit.
    #[serde(default = "default_cb_success_threshold")]
    pub success_threshold: u32,
    /// Default recovery timeout in seconds before transitioning from open to half-open.
    #[serde(default = "default_cb_recovery_timeout")]
    pub recovery_timeout_seconds: u64,
    /// Per-provider configuration overrides.
    #[serde(default)]
    pub providers: HashMap<String, CircuitBreakerProviderConfig>,
}

impl Default for CircuitBreakerServerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            failure_threshold: default_cb_failure_threshold(),
            success_threshold: default_cb_success_threshold(),
            recovery_timeout_seconds: default_cb_recovery_timeout(),
            providers: HashMap::new(),
        }
    }
}

fn default_cb_failure_threshold() -> u32 {
    5
}

fn default_cb_success_threshold() -> u32 {
    2
}

fn default_cb_recovery_timeout() -> u64 {
    60
}

/// Per-provider circuit breaker overrides.
#[derive(Debug, Deserialize)]
pub struct CircuitBreakerProviderConfig {
    /// Number of consecutive failures before opening the circuit.
    pub failure_threshold: Option<u32>,
    /// Number of consecutive successes in half-open state to close the circuit.
    pub success_threshold: Option<u32>,
    /// Recovery timeout in seconds.
    pub recovery_timeout_seconds: Option<u64>,
    /// Fallback provider to route to when the circuit is open.
    pub fallback_provider: Option<String>,
}
