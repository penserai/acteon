use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use tracing::{debug, info};

/// State of a circuit breaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation — requests flow through.
    Closed,
    /// Provider is failing — requests are rejected immediately.
    Open,
    /// Recovery probe — limited requests are allowed to test provider health.
    HalfOpen,
}

impl std::fmt::Display for CircuitState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => write!(f, "closed"),
            Self::Open => write!(f, "open"),
            Self::HalfOpen => write!(f, "half_open"),
        }
    }
}

/// Configuration for a per-provider circuit breaker.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening the circuit.
    pub failure_threshold: u32,
    /// Number of consecutive successes in `HalfOpen` state to close the circuit.
    pub success_threshold: u32,
    /// How long to wait in `Open` state before transitioning to `HalfOpen`.
    pub recovery_timeout: Duration,
    /// Optional fallback provider to use when the circuit is open.
    pub fallback_provider: Option<String>,
}

impl CircuitBreakerConfig {
    /// Validate configuration values.
    ///
    /// Returns `Err` with a description if any value is invalid:
    /// - `failure_threshold` must be >= 1
    /// - `success_threshold` must be >= 1
    ///
    /// `recovery_timeout = 0` is intentionally allowed (useful for testing).
    pub fn validate(&self) -> Result<(), String> {
        if self.failure_threshold < 1 {
            return Err("failure_threshold must be >= 1".into());
        }
        if self.success_threshold < 1 {
            return Err("success_threshold must be >= 1".into());
        }
        Ok(())
    }
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            success_threshold: 2,
            recovery_timeout: Duration::from_secs(60),
            fallback_provider: None,
        }
    }
}

/// Internal mutable state for a single circuit breaker.
struct CircuitData {
    state: CircuitState,
    consecutive_failures: u32,
    consecutive_successes: u32,
    last_failure_time: Option<Instant>,
    /// Whether a probe request is currently in flight during `HalfOpen` state.
    /// This prevents the thundering herd problem by allowing only one probe at a time.
    probe_in_flight: bool,
}

impl CircuitData {
    fn new() -> Self {
        Self {
            state: CircuitState::Closed,
            consecutive_failures: 0,
            consecutive_successes: 0,
            last_failure_time: None,
            probe_in_flight: false,
        }
    }
}

/// Circuit breaker for a single provider.
///
/// Tracks provider health and automatically transitions between states:
/// - `Closed` (normal) -> `Open` (failing) when consecutive failures reach the threshold
/// - `Open` -> `HalfOpen` (probing) after the recovery timeout elapses
/// - `HalfOpen` -> `Closed` after consecutive successes reach the threshold
/// - `HalfOpen` -> `Open` on any failure
pub struct CircuitBreaker {
    provider: String,
    config: CircuitBreakerConfig,
    data: RwLock<CircuitData>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker for the given provider.
    fn new(provider: impl Into<String>, config: CircuitBreakerConfig) -> Self {
        Self {
            provider: provider.into(),
            config,
            data: RwLock::new(CircuitData::new()),
        }
    }

    /// Check if a request should be allowed through.
    ///
    /// This may trigger a transition from `Open` to `HalfOpen` if the
    /// recovery timeout has elapsed. In `HalfOpen` state, only one probe
    /// request is allowed at a time to prevent the thundering herd problem.
    ///
    /// Returns `(state, Option<(from, to)>)` where the second element is
    /// `Some` when a state transition occurred.
    pub fn check(&self) -> (CircuitState, Option<(CircuitState, CircuitState)>) {
        let mut data = self
            .data
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if data.state == CircuitState::Open
            && let Some(last_failure) = data.last_failure_time
            && last_failure.elapsed() >= self.config.recovery_timeout
        {
            debug!(
                provider = %self.provider,
                "circuit breaker transitioning from open to half-open"
            );
            data.state = CircuitState::HalfOpen;
            data.consecutive_successes = 0;
            data.probe_in_flight = true;
            return (
                CircuitState::HalfOpen,
                Some((CircuitState::Open, CircuitState::HalfOpen)),
            );
        }

        // In HalfOpen state, reject if a probe is already in flight.
        if data.state == CircuitState::HalfOpen && data.probe_in_flight {
            return (CircuitState::Open, None);
        }

        // In HalfOpen state with no probe in flight, allow the next probe.
        if data.state == CircuitState::HalfOpen {
            data.probe_in_flight = true;
        }

        (data.state, None)
    }

    /// Record a successful execution.
    ///
    /// Returns `Some((from, to))` if a state transition occurred.
    pub fn record_success(&self) -> Option<(CircuitState, CircuitState)> {
        let mut data = self
            .data
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        match data.state {
            CircuitState::HalfOpen => {
                data.consecutive_successes += 1;
                data.probe_in_flight = false;
                if data.consecutive_successes >= self.config.success_threshold {
                    info!(
                        provider = %self.provider,
                        successes = data.consecutive_successes,
                        "circuit breaker closing after successful probes"
                    );
                    data.state = CircuitState::Closed;
                    data.consecutive_failures = 0;
                    data.consecutive_successes = 0;
                    Some((CircuitState::HalfOpen, CircuitState::Closed))
                } else {
                    None
                }
            }
            CircuitState::Closed => {
                data.consecutive_failures = 0;
                None
            }
            CircuitState::Open => None,
        }
    }

    /// Record a failed execution.
    ///
    /// Returns `Some((from, to))` if a state transition occurred.
    pub fn record_failure(&self) -> Option<(CircuitState, CircuitState)> {
        let mut data = self
            .data
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        match data.state {
            CircuitState::Closed => {
                data.consecutive_failures += 1;
                data.last_failure_time = Some(Instant::now());
                if data.consecutive_failures >= self.config.failure_threshold {
                    info!(
                        provider = %self.provider,
                        failures = data.consecutive_failures,
                        threshold = self.config.failure_threshold,
                        "circuit breaker opening"
                    );
                    data.state = CircuitState::Open;
                    Some((CircuitState::Closed, CircuitState::Open))
                } else {
                    None
                }
            }
            CircuitState::HalfOpen => {
                info!(
                    provider = %self.provider,
                    "circuit breaker re-opening after half-open probe failure"
                );
                data.state = CircuitState::Open;
                data.last_failure_time = Some(Instant::now());
                data.consecutive_successes = 0;
                data.probe_in_flight = false;
                Some((CircuitState::HalfOpen, CircuitState::Open))
            }
            CircuitState::Open => {
                data.last_failure_time = Some(Instant::now());
                None
            }
        }
    }

    /// Get current state without triggering transitions.
    pub fn state(&self) -> CircuitState {
        self.data
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .state
    }

    /// Get the configuration for this circuit breaker.
    pub fn config(&self) -> &CircuitBreakerConfig {
        &self.config
    }

    /// Get the provider name.
    pub fn provider_name(&self) -> &str {
        &self.provider
    }

    /// Reset the circuit breaker to `Closed` state.
    pub fn reset(&self) {
        let mut data = self
            .data
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        data.state = CircuitState::Closed;
        data.consecutive_failures = 0;
        data.consecutive_successes = 0;
        data.last_failure_time = None;
        data.probe_in_flight = false;
    }
}

impl std::fmt::Debug for CircuitBreaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let data = self
            .data
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f.debug_struct("CircuitBreaker")
            .field("provider", &self.provider)
            .field("config", &self.config)
            .field("state", &data.state)
            .field("consecutive_failures", &data.consecutive_failures)
            .field("consecutive_successes", &data.consecutive_successes)
            .finish_non_exhaustive()
    }
}

/// Registry managing circuit breakers for multiple providers.
///
/// Built once at gateway construction time and then used immutably for
/// circuit state lookups. Individual [`CircuitBreaker`] instances handle
/// their own internal mutability.
pub struct CircuitBreakerRegistry {
    breakers: HashMap<String, CircuitBreaker>,
}

impl CircuitBreakerRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            breakers: HashMap::new(),
        }
    }

    /// Register a circuit breaker for a provider.
    pub fn register(&mut self, provider: impl Into<String>, config: CircuitBreakerConfig) {
        let name = provider.into();
        self.breakers
            .insert(name.clone(), CircuitBreaker::new(name, config));
    }

    /// Look up the circuit breaker for a provider.
    pub fn get(&self, provider: &str) -> Option<&CircuitBreaker> {
        self.breakers.get(provider)
    }

    /// Return a sorted list of all provider names that have circuit breakers.
    pub fn providers(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.breakers.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }

    /// Return the number of registered circuit breakers.
    pub fn len(&self) -> usize {
        self.breakers.len()
    }

    /// Return `true` if no circuit breakers are registered.
    pub fn is_empty(&self) -> bool {
        self.breakers.is_empty()
    }
}

impl Default for CircuitBreakerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for CircuitBreakerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CircuitBreakerRegistry")
            .field("providers", &self.providers())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_threshold: 3,
            success_threshold: 2,
            recovery_timeout: Duration::from_secs(60),
            fallback_provider: None,
        }
    }

    /// Helper: call `check()` and return only the effective state.
    fn check_state(cb: &CircuitBreaker) -> CircuitState {
        cb.check().0
    }

    // -- CircuitState tests ---------------------------------------------------

    #[test]
    fn circuit_state_display() {
        assert_eq!(CircuitState::Closed.to_string(), "closed");
        assert_eq!(CircuitState::Open.to_string(), "open");
        assert_eq!(CircuitState::HalfOpen.to_string(), "half_open");
    }

    // -- CircuitBreakerConfig tests -------------------------------------------

    #[test]
    fn default_config_values() {
        let cfg = CircuitBreakerConfig::default();
        assert_eq!(cfg.failure_threshold, 5);
        assert_eq!(cfg.success_threshold, 2);
        assert_eq!(cfg.recovery_timeout, Duration::from_secs(60));
        assert!(cfg.fallback_provider.is_none());
    }

    #[test]
    fn config_validation_rejects_zero_failure_threshold() {
        let config = CircuitBreakerConfig {
            failure_threshold: 0,
            ..default_config()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn config_validation_rejects_zero_success_threshold() {
        let config = CircuitBreakerConfig {
            success_threshold: 0,
            ..default_config()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn config_validation_accepts_valid_config() {
        assert!(default_config().validate().is_ok());
    }

    #[test]
    fn config_validation_allows_zero_recovery_timeout() {
        let config = CircuitBreakerConfig {
            recovery_timeout: Duration::ZERO,
            ..default_config()
        };
        assert!(config.validate().is_ok());
    }

    // -- CircuitBreaker state transition tests --------------------------------

    #[test]
    fn starts_closed() {
        let cb = CircuitBreaker::new("test", default_config());
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(check_state(&cb), CircuitState::Closed);
    }

    #[test]
    fn opens_after_failure_threshold() {
        let cb = CircuitBreaker::new("test", default_config());

        // Two failures - still closed (threshold is 3)
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);

        // Third failure trips the circuit
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn success_resets_failure_count() {
        let cb = CircuitBreaker::new("test", default_config());

        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);

        // Success resets the consecutive failure counter
        cb.record_success();

        // Need 3 more consecutive failures to trip
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn does_not_open_below_threshold() {
        let cb = CircuitBreaker::new("test", default_config());

        // Alternating failure/success never reaches threshold
        for _ in 0..10 {
            cb.record_failure();
            cb.record_failure();
            cb.record_success();
        }
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn single_failure_threshold() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            ..default_config()
        };
        let cb = CircuitBreaker::new("test", config);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn half_open_to_closed_after_successes() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 2,
            recovery_timeout: Duration::ZERO, // Immediate transition for testing
            fallback_provider: None,
        };
        let cb = CircuitBreaker::new("test", config);

        // Trip the circuit
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Recovery timeout is zero, so check() transitions to HalfOpen (probe 1 allowed)
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);

        // First success - still half-open, clears probe_in_flight
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Allow the next probe through
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);

        // Second success - closes the circuit
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn half_open_to_open_on_failure() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 2,
            recovery_timeout: Duration::ZERO,
            fallback_provider: None,
        };
        let cb = CircuitBreaker::new("test", config);

        // Trip and transition to half-open
        cb.record_failure();
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);

        // Failure in half-open goes back to open
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn open_stays_open_before_timeout() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_secs(3600), // Very long
            ..default_config()
        };
        let cb = CircuitBreaker::new("test", config);

        cb.record_failure();
        assert_eq!(check_state(&cb), CircuitState::Open);
    }

    #[test]
    fn reset_returns_to_closed() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            ..default_config()
        };
        let cb = CircuitBreaker::new("test", config);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        cb.reset();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn full_lifecycle() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            success_threshold: 1,
            recovery_timeout: Duration::ZERO,
            fallback_provider: None,
        };
        let cb = CircuitBreaker::new("test", config);

        // Closed -> Open
        assert_eq!(check_state(&cb), CircuitState::Closed);
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Open -> HalfOpen (recovery timeout is zero)
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);

        // HalfOpen -> Closed
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);

        // Back to normal operation
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn debug_format() {
        let cb = CircuitBreaker::new("email", default_config());
        let debug = format!("{cb:?}");
        assert!(debug.contains("email"));
        assert!(debug.contains("Closed"));
    }

    // -- CircuitBreakerRegistry tests -----------------------------------------

    #[test]
    fn empty_registry() {
        let reg = CircuitBreakerRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert!(reg.providers().is_empty());
    }

    #[test]
    fn register_and_get() {
        let mut reg = CircuitBreakerRegistry::new();
        reg.register("email", default_config());
        reg.register("slack", default_config());

        assert_eq!(reg.len(), 2);
        assert!(!reg.is_empty());
        assert!(reg.get("email").is_some());
        assert!(reg.get("slack").is_some());
        assert!(reg.get("pagerduty").is_none());
    }

    #[test]
    fn registry_providers_sorted() {
        let mut reg = CircuitBreakerRegistry::new();
        reg.register("slack", default_config());
        reg.register("email", default_config());
        reg.register("webhook", default_config());

        assert_eq!(reg.providers(), vec!["email", "slack", "webhook"]);
    }

    #[test]
    fn registry_default_is_empty() {
        let reg = CircuitBreakerRegistry::default();
        assert!(reg.is_empty());
    }

    // -- Concurrency tests ----------------------------------------------------

    #[test]
    fn concurrent_record_operations() {
        use std::sync::Arc;

        let cb = Arc::new(CircuitBreaker::new(
            "test",
            CircuitBreakerConfig {
                failure_threshold: 100,
                ..default_config()
            },
        ));

        let mut handles = Vec::new();

        // Spawn threads that record failures concurrently
        for _ in 0..10 {
            let cb = Arc::clone(&cb);
            handles.push(std::thread::spawn(move || {
                for _ in 0..10 {
                    cb.record_failure();
                }
            }));
        }

        for handle in handles {
            handle.join().expect("thread should not panic");
        }

        // 10 threads * 10 failures = 100, which equals the threshold
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn concurrent_check_and_record() {
        use std::sync::Arc;

        let cb = Arc::new(CircuitBreaker::new(
            "test",
            CircuitBreakerConfig {
                failure_threshold: 5,
                recovery_timeout: Duration::ZERO,
                ..default_config()
            },
        ));

        let mut handles = Vec::new();

        // Mix of checks and failures
        for i in 0..20 {
            let cb = Arc::clone(&cb);
            handles.push(std::thread::spawn(move || {
                if i % 2 == 0 {
                    cb.check();
                } else {
                    cb.record_failure();
                }
            }));
        }

        for handle in handles {
            handle.join().expect("thread should not panic");
        }

        // State should be valid (no panics, no corruption)
        let state = cb.state();
        assert!(
            state == CircuitState::Closed
                || state == CircuitState::Open
                || state == CircuitState::HalfOpen
        );
    }

    // -- Edge case tests ------------------------------------------------------

    #[test]
    fn large_failure_threshold() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1_000_000,
            ..default_config()
        };
        let cb = CircuitBreaker::new("test", config);

        // A few failures shouldn't trip a very large threshold.
        for _ in 0..100 {
            cb.record_failure();
        }
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    // zero_failure_threshold test removed: config validation now rejects
    // failure_threshold < 1 (see config_validation_rejects_zero_failure_threshold).

    #[test]
    fn rapid_alternation_never_trips() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            ..default_config()
        };
        let cb = CircuitBreaker::new("test", config);

        // Alternating: fail, succeed, fail, succeed, ...
        // Success resets the consecutive failure count, so we never reach 3.
        for _ in 0..100 {
            cb.record_failure();
            cb.record_success();
        }
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn rapid_failure_success_pattern_two_then_reset() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            ..default_config()
        };
        let cb = CircuitBreaker::new("test", config);

        // Two failures then a success: doesn't trip (2 < 3).
        for _ in 0..50 {
            cb.record_failure();
            cb.record_failure();
            cb.record_success();
        }
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn exact_threshold_boundary() {
        let config = CircuitBreakerConfig {
            failure_threshold: 5,
            ..default_config()
        };
        let cb = CircuitBreaker::new("test", config);

        // 4 failures: not yet open.
        for _ in 0..4 {
            cb.record_failure();
        }
        assert_eq!(cb.state(), CircuitState::Closed);

        // 5th failure: opens.
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn half_open_requires_exact_success_threshold() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 3,
            recovery_timeout: Duration::ZERO,
            fallback_provider: None,
        };
        let cb = CircuitBreaker::new("test", config);

        // Trip and move to HalfOpen (probe 1 allowed).
        cb.record_failure();
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);

        // 1st success: still half-open (need 3), clears probe_in_flight.
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Allow probe 2.
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Allow probe 3.
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn half_open_failure_resets_success_count() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 3,
            recovery_timeout: Duration::ZERO,
            fallback_provider: None,
        };
        let cb = CircuitBreaker::new("test", config);

        // Trip and move to HalfOpen (probe 1 allowed).
        cb.record_failure();
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);

        // 1st success, allow probe 2, then 2nd success.
        cb.record_success();
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);
        cb.record_success();

        // Failure on probe 3: goes back to Open.
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Transition to HalfOpen again.
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);

        // Need full 3 successes again (previous progress was reset).
        cb.record_success();
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::HalfOpen);
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn multiple_providers_independent_in_registry() {
        let mut reg = CircuitBreakerRegistry::new();
        reg.register(
            "email",
            CircuitBreakerConfig {
                failure_threshold: 2,
                ..default_config()
            },
        );
        reg.register(
            "slack",
            CircuitBreakerConfig {
                failure_threshold: 3,
                ..default_config()
            },
        );

        let email = reg.get("email").unwrap();
        let slack = reg.get("slack").unwrap();

        // Trip email circuit (threshold 2).
        email.record_failure();
        email.record_failure();
        assert_eq!(email.state(), CircuitState::Open);

        // Slack should be unaffected.
        assert_eq!(slack.state(), CircuitState::Closed);

        // Slack needs 3 failures.
        slack.record_failure();
        slack.record_failure();
        assert_eq!(slack.state(), CircuitState::Closed);
        slack.record_failure();
        assert_eq!(slack.state(), CircuitState::Open);
    }

    #[test]
    fn success_in_open_state_does_nothing() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_secs(3600),
            ..default_config()
        };
        let cb = CircuitBreaker::new("test", config);

        // Trip the circuit.
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Recording success while Open should not change state.
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn failure_in_open_state_updates_last_failure_time() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_secs(3600),
            ..default_config()
        };
        let cb = CircuitBreaker::new("test", config);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Additional failures should keep it Open without panicking.
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn provider_name_accessor() {
        let cb = CircuitBreaker::new("my-provider", default_config());
        assert_eq!(cb.provider_name(), "my-provider");
    }

    #[test]
    fn config_accessor() {
        let config = CircuitBreakerConfig {
            failure_threshold: 42,
            success_threshold: 7,
            recovery_timeout: Duration::from_secs(999),
            fallback_provider: Some("backup".into()),
        };
        let cb = CircuitBreaker::new("test", config);

        assert_eq!(cb.config().failure_threshold, 42);
        assert_eq!(cb.config().success_threshold, 7);
        assert_eq!(cb.config().recovery_timeout, Duration::from_secs(999));
        assert_eq!(cb.config().fallback_provider.as_deref(), Some("backup"));
    }

    #[test]
    fn reset_from_half_open() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::ZERO,
            ..default_config()
        };
        let cb = CircuitBreaker::new("test", config);

        cb.record_failure();
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);

        cb.reset();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn registry_debug_format() {
        let mut reg = CircuitBreakerRegistry::new();
        reg.register("alpha", default_config());
        reg.register("beta", default_config());

        let debug = format!("{reg:?}");
        assert!(debug.contains("alpha"));
        assert!(debug.contains("beta"));
    }

    #[test]
    fn registry_overwrite_existing_provider() {
        let mut reg = CircuitBreakerRegistry::new();
        reg.register(
            "email",
            CircuitBreakerConfig {
                failure_threshold: 3,
                ..default_config()
            },
        );

        // Trip the first circuit breaker.
        let cb = reg.get("email").unwrap();
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Re-register with different config — should replace.
        reg.register(
            "email",
            CircuitBreakerConfig {
                failure_threshold: 10,
                ..default_config()
            },
        );

        let cb2 = reg.get("email").unwrap();
        assert_eq!(cb2.state(), CircuitState::Closed);
        assert_eq!(cb2.config().failure_threshold, 10);
    }

    #[test]
    fn full_lifecycle_multiple_cycles() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            success_threshold: 1,
            recovery_timeout: Duration::ZERO,
            fallback_provider: None,
        };
        let cb = CircuitBreaker::new("test", config);

        // Cycle 1: Closed -> Open -> HalfOpen -> Closed
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);

        // Cycle 2: Trip again
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);

        // Cycle 2: Fail probe -> back to Open
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Cycle 2: Recover
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);

        // Cycle 3: Immediate trip and recovery
        cb.record_failure();
        cb.record_failure();
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    // -- Time-based tests ----------------------------------------------------
    //
    // The circuit breaker uses `std::time::Instant`, so `tokio::time::pause()`
    // has no effect. We use `Duration::ZERO` for deterministic instant
    // transitions and `Duration::from_secs(3600)` for "never expires in a
    // test" scenarios.

    #[test]
    fn recovery_timeout_zero_transitions_immediately() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 1,
            recovery_timeout: Duration::ZERO,
            fallback_provider: None,
        };
        let cb = CircuitBreaker::new("test", config);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // With zero timeout, check() immediately transitions to HalfOpen.
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);
    }

    #[test]
    fn long_recovery_timeout_stays_open() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 1,
            recovery_timeout: Duration::from_secs(3600),
            fallback_provider: None,
        };
        let cb = CircuitBreaker::new("test", config);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // With a very long timeout, check() should not transition.
        assert_eq!(check_state(&cb), CircuitState::Open);
        assert_eq!(check_state(&cb), CircuitState::Open);
    }

    #[test]
    fn half_open_probe_failure_requires_fresh_recovery_wait() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 1,
            recovery_timeout: Duration::ZERO,
            fallback_provider: None,
        };
        let cb = CircuitBreaker::new("test", config);

        // Trip and transition to HalfOpen.
        cb.record_failure();
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);

        // Fail the probe -> back to Open with fresh last_failure_time.
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // With ZERO timeout, immediately back to HalfOpen.
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);

        // Succeed the probe this time.
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn very_short_recovery_timeout_with_sleep() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 1,
            recovery_timeout: Duration::from_millis(10),
            fallback_provider: None,
        };
        let cb = CircuitBreaker::new("test", config);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Wait longer than the recovery timeout.
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);

        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    // -- Concurrent stress tests ----------------------------------------------

    #[test]
    fn concurrent_successes_in_half_open() {
        use std::sync::Arc;

        let cb = Arc::new(CircuitBreaker::new(
            "test",
            CircuitBreakerConfig {
                failure_threshold: 1,
                success_threshold: 2,
                recovery_timeout: Duration::ZERO,
                fallback_provider: None,
            },
        ));

        // Trip and move to HalfOpen.
        cb.record_failure();
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);

        let mut handles = Vec::new();

        for _ in 0..10 {
            let cb = Arc::clone(&cb);
            handles.push(std::thread::spawn(move || {
                cb.record_success();
            }));
        }

        for handle in handles {
            handle.join().expect("thread should not panic");
        }

        // After many successes, should be Closed.
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn concurrent_mixed_operations_no_panic() {
        use std::sync::Arc;

        let cb = Arc::new(CircuitBreaker::new(
            "test",
            CircuitBreakerConfig {
                failure_threshold: 3,
                success_threshold: 2,
                recovery_timeout: Duration::ZERO,
                fallback_provider: None,
            },
        ));

        let mut handles = Vec::new();

        for i in 0..50 {
            let cb = Arc::clone(&cb);
            handles.push(std::thread::spawn(move || match i % 4 {
                0 => {
                    cb.record_failure();
                }
                1 => {
                    cb.record_success();
                }
                2 => {
                    cb.check();
                }
                3 => cb.reset(),
                _ => unreachable!(),
            }));
        }

        for handle in handles {
            handle.join().expect("thread should not panic");
        }

        // No assertions on final state needed — just verify no panics.
        let _ = cb.state();
    }

    // -- Probe limiting tests -------------------------------------------------

    #[test]
    fn half_open_rejects_concurrent_probes() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 1,
            recovery_timeout: Duration::ZERO,
            fallback_provider: None,
        };
        let cb = CircuitBreaker::new("test", config);

        // Trip the circuit.
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // First check transitions to HalfOpen and allows the probe.
        let (state, transition) = cb.check();
        assert_eq!(state, CircuitState::HalfOpen);
        assert_eq!(
            transition,
            Some((CircuitState::Open, CircuitState::HalfOpen))
        );

        // Second check while probe is in flight returns Open (rejected).
        let (state, transition) = cb.check();
        assert_eq!(state, CircuitState::Open);
        assert!(transition.is_none());

        // Complete the probe successfully -> closes the circuit.
        let transition = cb.record_success();
        assert_eq!(
            transition,
            Some((CircuitState::HalfOpen, CircuitState::Closed))
        );
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn probe_in_flight_cleared_on_failure() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 1,
            recovery_timeout: Duration::ZERO,
            fallback_provider: None,
        };
        let cb = CircuitBreaker::new("test", config);

        cb.record_failure();
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);

        // Probe fails -> back to Open, probe_in_flight cleared.
        let transition = cb.record_failure();
        assert_eq!(
            transition,
            Some((CircuitState::HalfOpen, CircuitState::Open))
        );

        // Can transition to HalfOpen again and allow a new probe.
        assert_eq!(check_state(&cb), CircuitState::HalfOpen);
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    // -- Transition return value tests ----------------------------------------

    #[test]
    fn record_failure_returns_transition_on_threshold() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            ..default_config()
        };
        let cb = CircuitBreaker::new("test", config);

        // First failure: no transition.
        assert!(cb.record_failure().is_none());
        // Second failure: Closed -> Open transition.
        let t = cb.record_failure();
        assert_eq!(t, Some((CircuitState::Closed, CircuitState::Open)));
    }

    #[test]
    fn record_success_returns_transition_on_close() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 1,
            recovery_timeout: Duration::ZERO,
            fallback_provider: None,
        };
        let cb = CircuitBreaker::new("test", config);

        cb.record_failure();
        check_state(&cb); // -> HalfOpen

        let t = cb.record_success();
        assert_eq!(t, Some((CircuitState::HalfOpen, CircuitState::Closed)));
    }

    #[test]
    fn record_success_in_closed_returns_none() {
        let cb = CircuitBreaker::new("test", default_config());
        assert!(cb.record_success().is_none());
    }

    #[test]
    fn record_failure_in_open_returns_none() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_secs(3600),
            ..default_config()
        };
        let cb = CircuitBreaker::new("test", config);

        cb.record_failure(); // trips to Open
        // Additional failure in Open -> no transition.
        assert!(cb.record_failure().is_none());
    }
}
