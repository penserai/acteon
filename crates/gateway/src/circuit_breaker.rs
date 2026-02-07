use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use acteon_state::{DistributedLock, KeyKind, StateKey, StateStore};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// Maximum time (ms) a probe can be in flight before it is considered stale.
/// If a probe request crashes or times out without calling `record_success`
/// or `record_failure`, the probe slot is freed after this interval.
const PROBE_TIMEOUT_MS: i64 = 30_000;

/// TTL for the short-lived distributed mutation lock.
const MUTATION_LOCK_TTL: Duration = Duration::from_secs(5);

/// State of a circuit breaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
///
/// Stored as JSON in the [`StateStore`] so that multiple gateway instances
/// share the same view of provider health.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CircuitData {
    state: CircuitState,
    consecutive_failures: u32,
    consecutive_successes: u32,
    /// Wall-clock time of last failure (ms since Unix epoch).
    #[serde(default)]
    last_failure_time_ms: Option<i64>,
    /// Wall-clock time when the current probe was started (ms since Unix epoch).
    /// Used for thundering-herd prevention: only one probe at a time in `HalfOpen`.
    /// Probes older than [`PROBE_TIMEOUT_MS`] are considered stale.
    #[serde(default)]
    probe_started_at_ms: Option<i64>,
}

impl Default for CircuitData {
    fn default() -> Self {
        Self {
            state: CircuitState::Closed,
            consecutive_failures: 0,
            consecutive_successes: 0,
            last_failure_time_ms: None,
            probe_started_at_ms: None,
        }
    }
}

/// Circuit breaker for a single provider.
///
/// State is persisted in a [`StateStore`] so that multiple gateway instances
/// share circuit state. A [`DistributedLock`] serialises mutations to prevent
/// lost updates.
///
/// Tracks provider health and automatically transitions between states:
/// - `Closed` (normal) -> `Open` (failing) when consecutive failures reach the threshold
/// - `Open` -> `HalfOpen` (probing) after the recovery timeout elapses
/// - `HalfOpen` -> `Closed` after consecutive successes reach the threshold
/// - `HalfOpen` -> `Open` on any failure
pub struct CircuitBreaker {
    provider: String,
    config: CircuitBreakerConfig,
    store: Arc<dyn StateStore>,
    lock: Arc<dyn DistributedLock>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker for the given provider.
    fn new(
        provider: impl Into<String>,
        config: CircuitBreakerConfig,
        store: Arc<dyn StateStore>,
        lock: Arc<dyn DistributedLock>,
    ) -> Self {
        Self {
            provider: provider.into(),
            config,
            store,
            lock,
        }
    }

    /// Build the [`StateKey`] used to persist this breaker's data.
    fn state_key(&self) -> StateKey {
        StateKey::new(
            "_system",
            "_global",
            KeyKind::Custom("circuit_breaker".into()),
            &self.provider,
        )
    }

    /// Build the distributed lock name for mutation serialisation.
    fn lock_name(&self) -> String {
        format!("cb:{}", self.provider)
    }

    /// Load state from the store.  Returns `CircuitData::default()` (Closed)
    /// if the key does not exist or on any error (fail-open).
    async fn load_state(&self) -> CircuitData {
        match self.store.get(&self.state_key()).await {
            Ok(Some(json)) => serde_json::from_str(&json).unwrap_or_default(),
            Ok(None) => CircuitData::default(),
            Err(e) => {
                warn!(
                    provider = %self.provider,
                    error = %e,
                    "failed to load circuit breaker state, using default"
                );
                CircuitData::default()
            }
        }
    }

    /// Persist state to the store.  Errors are logged but not propagated
    /// (fail-open).
    async fn save_state(&self, data: &CircuitData) {
        let json = match serde_json::to_string(data) {
            Ok(j) => j,
            Err(e) => {
                warn!(
                    provider = %self.provider,
                    error = %e,
                    "failed to serialise circuit breaker state"
                );
                return;
            }
        };
        if let Err(e) = self.store.set(&self.state_key(), &json, None).await {
            warn!(
                provider = %self.provider,
                error = %e,
                "failed to save circuit breaker state"
            );
        }
    }

    /// Try to acquire the short-lived mutation lock.
    async fn acquire_mutation_lock(&self) -> Option<Box<dyn acteon_state::LockGuard>> {
        match self
            .lock
            .try_acquire(&self.lock_name(), MUTATION_LOCK_TTL)
            .await
        {
            Ok(guard) => guard,
            Err(e) => {
                warn!(
                    provider = %self.provider,
                    error = %e,
                    "failed to acquire circuit breaker mutation lock"
                );
                None
            }
        }
    }

    fn now_ms() -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    /// Check whether a probe is currently active (not stale).
    fn is_probe_active(data: &CircuitData) -> bool {
        data.probe_started_at_ms
            .is_some_and(|t| (Self::now_ms() - t) < PROBE_TIMEOUT_MS)
    }

    /// Acquire permission to send a request through this circuit breaker.
    ///
    /// This may trigger a transition from `Open` to `HalfOpen` if the
    /// recovery timeout has elapsed. In `HalfOpen` state, only one probe
    /// request is allowed at a time to prevent the thundering herd problem.
    ///
    /// **Side-effects:** In `HalfOpen` state, records `probe_started_at_ms`
    /// to reserve the probe slot. The caller **must** follow up with
    /// [`record_success`](Self::record_success) or
    /// [`record_failure`](Self::record_failure) to release the slot.
    ///
    /// Returns `(effective_state, Option<(from, to)>)` where the second
    /// element is `Some` when a state transition occurred.
    pub async fn try_acquire_permit(&self) -> (CircuitState, Option<(CircuitState, CircuitState)>) {
        let Some(guard) = self.acquire_mutation_lock().await else {
            // Cannot acquire lock — read state without mutation.
            let data = self.load_state().await;
            // If HalfOpen without lock, reject to be safe.
            if data.state == CircuitState::HalfOpen {
                return (CircuitState::Open, None);
            }
            return (data.state, None);
        };

        let mut data = self.load_state().await;
        let result;

        match data.state {
            CircuitState::Open => {
                let now = Self::now_ms();
                let elapsed_ms = data
                    .last_failure_time_ms
                    .map_or(i64::MAX, |t| (now - t).max(0));
                #[allow(clippy::cast_possible_truncation)]
                let timeout_ms = self.config.recovery_timeout.as_millis() as i64;

                if elapsed_ms >= timeout_ms {
                    debug!(
                        provider = %self.provider,
                        "circuit breaker transitioning from open to half-open"
                    );
                    data.state = CircuitState::HalfOpen;
                    data.consecutive_successes = 0;
                    data.probe_started_at_ms = Some(now);
                    self.save_state(&data).await;
                    result = (
                        CircuitState::HalfOpen,
                        Some((CircuitState::Open, CircuitState::HalfOpen)),
                    );
                } else {
                    result = (CircuitState::Open, None);
                }
            }
            CircuitState::HalfOpen => {
                if Self::is_probe_active(&data) {
                    // Probe in flight — reject.
                    result = (CircuitState::Open, None);
                } else {
                    // No active probe — allow this request as the new probe.
                    data.probe_started_at_ms = Some(Self::now_ms());
                    self.save_state(&data).await;
                    result = (CircuitState::HalfOpen, None);
                }
            }
            CircuitState::Closed => {
                result = (CircuitState::Closed, None);
            }
        }

        let _ = guard.release().await;
        result
    }

    /// Record a successful execution.
    ///
    /// Returns `Some((from, to))` if a state transition occurred.
    pub async fn record_success(&self) -> Option<(CircuitState, CircuitState)> {
        let guard = self.acquire_mutation_lock().await?;

        let mut data = self.load_state().await;
        let transition;

        match data.state {
            CircuitState::HalfOpen => {
                data.consecutive_successes += 1;
                data.probe_started_at_ms = None;
                if data.consecutive_successes >= self.config.success_threshold {
                    info!(
                        provider = %self.provider,
                        successes = data.consecutive_successes,
                        "circuit breaker closing after successful probes"
                    );
                    data.state = CircuitState::Closed;
                    data.consecutive_failures = 0;
                    data.consecutive_successes = 0;
                    transition = Some((CircuitState::HalfOpen, CircuitState::Closed));
                } else {
                    transition = None;
                }
                self.save_state(&data).await;
            }
            CircuitState::Closed => {
                if data.consecutive_failures > 0 {
                    data.consecutive_failures = 0;
                    self.save_state(&data).await;
                }
                transition = None;
            }
            CircuitState::Open => {
                transition = None;
            }
        }

        let _ = guard.release().await;
        transition
    }

    /// Record a failed execution.
    ///
    /// Returns `Some((from, to))` if a state transition occurred.
    pub async fn record_failure(&self) -> Option<(CircuitState, CircuitState)> {
        let guard = self.acquire_mutation_lock().await?;

        let mut data = self.load_state().await;
        let now = Self::now_ms();
        let transition;

        match data.state {
            CircuitState::Closed => {
                data.consecutive_failures += 1;
                data.last_failure_time_ms = Some(now);
                if data.consecutive_failures >= self.config.failure_threshold {
                    info!(
                        provider = %self.provider,
                        failures = data.consecutive_failures,
                        threshold = self.config.failure_threshold,
                        "circuit breaker opening"
                    );
                    data.state = CircuitState::Open;
                    transition = Some((CircuitState::Closed, CircuitState::Open));
                } else {
                    transition = None;
                }
                self.save_state(&data).await;
            }
            CircuitState::HalfOpen => {
                info!(
                    provider = %self.provider,
                    "circuit breaker re-opening after half-open probe failure"
                );
                data.state = CircuitState::Open;
                data.last_failure_time_ms = Some(now);
                data.consecutive_successes = 0;
                data.probe_started_at_ms = None;
                transition = Some((CircuitState::HalfOpen, CircuitState::Open));
                self.save_state(&data).await;
            }
            CircuitState::Open => {
                data.last_failure_time_ms = Some(now);
                transition = None;
                self.save_state(&data).await;
            }
        }

        let _ = guard.release().await;
        transition
    }

    /// Get current state without triggering transitions.
    pub async fn state(&self) -> CircuitState {
        self.load_state().await.state
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
    pub async fn reset(&self) {
        if let Some(guard) = self.acquire_mutation_lock().await {
            self.save_state(&CircuitData::default()).await;
            let _ = guard.release().await;
        }
    }
}

impl std::fmt::Debug for CircuitBreaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CircuitBreaker")
            .field("provider", &self.provider)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// Registry managing circuit breakers for multiple providers.
///
/// Built once at gateway construction time and then used immutably for
/// circuit state lookups. Individual [`CircuitBreaker`] instances handle
/// their own internal mutability through the shared [`StateStore`] and
/// [`DistributedLock`].
pub struct CircuitBreakerRegistry {
    breakers: HashMap<String, CircuitBreaker>,
    store: Arc<dyn StateStore>,
    lock: Arc<dyn DistributedLock>,
}

impl CircuitBreakerRegistry {
    /// Create an empty registry backed by the given state store and lock.
    pub fn new(store: Arc<dyn StateStore>, lock: Arc<dyn DistributedLock>) -> Self {
        Self {
            breakers: HashMap::new(),
            store,
            lock,
        }
    }

    /// Register a circuit breaker for a provider.
    pub fn register(&mut self, provider: impl Into<String>, config: CircuitBreakerConfig) {
        let name = provider.into();
        self.breakers.insert(
            name.clone(),
            CircuitBreaker::new(
                name,
                config,
                Arc::clone(&self.store),
                Arc::clone(&self.lock),
            ),
        );
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
    use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

    fn default_config() -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_threshold: 3,
            success_threshold: 2,
            recovery_timeout: Duration::from_secs(60),
            fallback_provider: None,
        }
    }

    /// Create a circuit breaker with in-memory state for testing.
    fn create_cb(config: CircuitBreakerConfig) -> CircuitBreaker {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let lock: Arc<dyn DistributedLock> = Arc::new(MemoryDistributedLock::new());
        CircuitBreaker::new("test", config, store, lock)
    }

    /// Helper: call `try_acquire_permit` and return only the effective state.
    async fn try_permit_state(cb: &CircuitBreaker) -> CircuitState {
        cb.try_acquire_permit().await.0
    }

    // -- CircuitState tests ---------------------------------------------------

    #[test]
    fn circuit_state_display() {
        assert_eq!(CircuitState::Closed.to_string(), "closed");
        assert_eq!(CircuitState::Open.to_string(), "open");
        assert_eq!(CircuitState::HalfOpen.to_string(), "half_open");
    }

    #[test]
    fn circuit_state_serde_roundtrip() {
        let json = serde_json::to_string(&CircuitState::HalfOpen).unwrap();
        assert_eq!(json, "\"half_open\"");
        let deserialized: CircuitState = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, CircuitState::HalfOpen);
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

    // -- CircuitData serde tests ----------------------------------------------

    #[test]
    fn circuit_data_default_is_closed() {
        let data = CircuitData::default();
        assert_eq!(data.state, CircuitState::Closed);
        assert_eq!(data.consecutive_failures, 0);
        assert_eq!(data.consecutive_successes, 0);
        assert!(data.last_failure_time_ms.is_none());
        assert!(data.probe_started_at_ms.is_none());
    }

    #[test]
    fn circuit_data_serde_roundtrip() {
        let data = CircuitData {
            state: CircuitState::Open,
            consecutive_failures: 5,
            consecutive_successes: 0,
            last_failure_time_ms: Some(1_700_000_000_000),
            probe_started_at_ms: None,
        };
        let json = serde_json::to_string(&data).unwrap();
        let parsed: CircuitData = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.state, CircuitState::Open);
        assert_eq!(parsed.consecutive_failures, 5);
        assert_eq!(parsed.last_failure_time_ms, Some(1_700_000_000_000));
    }

    #[test]
    fn circuit_data_deserializes_with_missing_optional_fields() {
        let json = r#"{"state":"closed","consecutive_failures":0,"consecutive_successes":0}"#;
        let data: CircuitData = serde_json::from_str(json).unwrap();
        assert!(data.last_failure_time_ms.is_none());
        assert!(data.probe_started_at_ms.is_none());
    }

    // -- CircuitBreaker state transition tests --------------------------------

    #[tokio::test]
    async fn starts_closed() {
        let cb = create_cb(default_config());
        assert_eq!(cb.state().await, CircuitState::Closed);
        assert_eq!(try_permit_state(&cb).await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn opens_after_failure_threshold() {
        let cb = create_cb(default_config());

        // Two failures - still closed (threshold is 3)
        cb.record_failure().await;
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Closed);

        // Third failure trips the circuit
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);
    }

    #[tokio::test]
    async fn success_resets_failure_count() {
        let cb = create_cb(default_config());

        cb.record_failure().await;
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Closed);

        // Success resets the consecutive failure counter
        cb.record_success().await;

        // Need 3 more consecutive failures to trip
        cb.record_failure().await;
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Closed);

        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);
    }

    #[tokio::test]
    async fn does_not_open_below_threshold() {
        let cb = create_cb(default_config());

        // Alternating failure/success never reaches threshold
        for _ in 0..10 {
            cb.record_failure().await;
            cb.record_failure().await;
            cb.record_success().await;
        }
        assert_eq!(cb.state().await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn single_failure_threshold() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            ..default_config()
        };
        let cb = create_cb(config);

        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);
    }

    #[tokio::test]
    async fn half_open_to_closed_after_successes() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 2,
            recovery_timeout: Duration::ZERO, // Immediate transition for testing
            fallback_provider: None,
        };
        let cb = create_cb(config);

        // Trip the circuit
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);

        // Recovery timeout is zero, so try_acquire_permit transitions to HalfOpen
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);

        // First success - still half-open, clears probe
        cb.record_success().await;
        assert_eq!(cb.state().await, CircuitState::HalfOpen);

        // Allow the next probe through
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);

        // Second success - closes the circuit
        cb.record_success().await;
        assert_eq!(cb.state().await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn half_open_to_open_on_failure() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 2,
            recovery_timeout: Duration::ZERO,
            fallback_provider: None,
        };
        let cb = create_cb(config);

        // Trip and transition to half-open
        cb.record_failure().await;
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);

        // Failure in half-open goes back to open
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);
    }

    #[tokio::test]
    async fn open_stays_open_before_timeout() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_secs(3600), // Very long
            ..default_config()
        };
        let cb = create_cb(config);

        cb.record_failure().await;
        assert_eq!(try_permit_state(&cb).await, CircuitState::Open);
    }

    #[tokio::test]
    async fn reset_returns_to_closed() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            ..default_config()
        };
        let cb = create_cb(config);

        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);

        cb.reset().await;
        assert_eq!(cb.state().await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn full_lifecycle() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            success_threshold: 1,
            recovery_timeout: Duration::ZERO,
            fallback_provider: None,
        };
        let cb = create_cb(config);

        // Closed -> Open
        assert_eq!(try_permit_state(&cb).await, CircuitState::Closed);
        cb.record_failure().await;
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);

        // Open -> HalfOpen (recovery timeout is zero)
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);

        // HalfOpen -> Closed
        cb.record_success().await;
        assert_eq!(cb.state().await, CircuitState::Closed);

        // Back to normal operation
        cb.record_success().await;
        assert_eq!(cb.state().await, CircuitState::Closed);
    }

    #[test]
    fn debug_format() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let lock: Arc<dyn DistributedLock> = Arc::new(MemoryDistributedLock::new());
        let cb = CircuitBreaker::new("email", default_config(), store, lock);
        let debug = format!("{cb:?}");
        assert!(debug.contains("email"));
    }

    // -- CircuitBreakerRegistry tests -----------------------------------------

    #[test]
    fn empty_registry() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let lock: Arc<dyn DistributedLock> = Arc::new(MemoryDistributedLock::new());
        let reg = CircuitBreakerRegistry::new(store, lock);
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert!(reg.providers().is_empty());
    }

    #[test]
    fn register_and_get() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let lock: Arc<dyn DistributedLock> = Arc::new(MemoryDistributedLock::new());
        let mut reg = CircuitBreakerRegistry::new(store, lock);
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
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let lock: Arc<dyn DistributedLock> = Arc::new(MemoryDistributedLock::new());
        let mut reg = CircuitBreakerRegistry::new(store, lock);
        reg.register("slack", default_config());
        reg.register("email", default_config());
        reg.register("webhook", default_config());

        assert_eq!(reg.providers(), vec!["email", "slack", "webhook"]);
    }

    // -- Concurrency tests ----------------------------------------------------

    #[tokio::test]
    async fn concurrent_record_operations() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let lock: Arc<dyn DistributedLock> = Arc::new(MemoryDistributedLock::new());
        let cb = Arc::new(CircuitBreaker::new(
            "test",
            CircuitBreakerConfig {
                failure_threshold: 100,
                ..default_config()
            },
            store,
            lock,
        ));

        let mut handles = Vec::new();

        // Spawn tasks that record failures concurrently
        for _ in 0..10 {
            let cb = Arc::clone(&cb);
            handles.push(tokio::spawn(async move {
                for _ in 0..10 {
                    cb.record_failure().await;
                }
            }));
        }

        for handle in handles {
            handle.await.expect("task should not panic");
        }

        // 10 tasks * 10 failures = 100, which equals the threshold.
        // Some may be lost under contention, so just verify valid state.
        let state = cb.state().await;
        assert!(
            state == CircuitState::Closed || state == CircuitState::Open,
            "state should be closed or open, got {state}"
        );
    }

    #[tokio::test]
    async fn concurrent_check_and_record() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let lock: Arc<dyn DistributedLock> = Arc::new(MemoryDistributedLock::new());
        let cb = Arc::new(CircuitBreaker::new(
            "test",
            CircuitBreakerConfig {
                failure_threshold: 5,
                recovery_timeout: Duration::ZERO,
                ..default_config()
            },
            store,
            lock,
        ));

        let mut handles = Vec::new();

        // Mix of checks and failures
        for i in 0..20 {
            let cb = Arc::clone(&cb);
            handles.push(tokio::spawn(async move {
                if i % 2 == 0 {
                    cb.try_acquire_permit().await;
                } else {
                    cb.record_failure().await;
                }
            }));
        }

        for handle in handles {
            handle.await.expect("task should not panic");
        }

        // State should be valid (no panics, no corruption)
        let state = cb.state().await;
        assert!(
            state == CircuitState::Closed
                || state == CircuitState::Open
                || state == CircuitState::HalfOpen
        );
    }

    // -- Edge case tests ------------------------------------------------------

    #[tokio::test]
    async fn large_failure_threshold() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1_000_000,
            ..default_config()
        };
        let cb = create_cb(config);

        // A few failures shouldn't trip a very large threshold.
        for _ in 0..100 {
            cb.record_failure().await;
        }
        assert_eq!(cb.state().await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn rapid_alternation_never_trips() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            ..default_config()
        };
        let cb = create_cb(config);

        // Alternating: fail, succeed, fail, succeed, ...
        // Success resets the consecutive failure count, so we never reach 3.
        for _ in 0..100 {
            cb.record_failure().await;
            cb.record_success().await;
        }
        assert_eq!(cb.state().await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn rapid_failure_success_pattern_two_then_reset() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            ..default_config()
        };
        let cb = create_cb(config);

        // Two failures then a success: doesn't trip (2 < 3).
        for _ in 0..50 {
            cb.record_failure().await;
            cb.record_failure().await;
            cb.record_success().await;
        }
        assert_eq!(cb.state().await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn exact_threshold_boundary() {
        let config = CircuitBreakerConfig {
            failure_threshold: 5,
            ..default_config()
        };
        let cb = create_cb(config);

        // 4 failures: not yet open.
        for _ in 0..4 {
            cb.record_failure().await;
        }
        assert_eq!(cb.state().await, CircuitState::Closed);

        // 5th failure: opens.
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);
    }

    #[tokio::test]
    async fn half_open_requires_exact_success_threshold() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 3,
            recovery_timeout: Duration::ZERO,
            fallback_provider: None,
        };
        let cb = create_cb(config);

        // Trip and move to HalfOpen (probe 1 allowed).
        cb.record_failure().await;
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);

        // 1st success: still half-open (need 3), clears probe.
        cb.record_success().await;
        assert_eq!(cb.state().await, CircuitState::HalfOpen);

        // Allow probe 2.
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);
        cb.record_success().await;
        assert_eq!(cb.state().await, CircuitState::HalfOpen);

        // Allow probe 3.
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);
        cb.record_success().await;
        assert_eq!(cb.state().await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn half_open_failure_resets_success_count() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 3,
            recovery_timeout: Duration::ZERO,
            fallback_provider: None,
        };
        let cb = create_cb(config);

        // Trip and move to HalfOpen (probe 1 allowed).
        cb.record_failure().await;
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);

        // 1st success, allow probe 2, then 2nd success.
        cb.record_success().await;
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);
        cb.record_success().await;

        // Failure on probe 3: goes back to Open.
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);

        // Transition to HalfOpen again.
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);

        // Need full 3 successes again (previous progress was reset).
        cb.record_success().await;
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);
        cb.record_success().await;
        assert_eq!(cb.state().await, CircuitState::HalfOpen);
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);
        cb.record_success().await;
        assert_eq!(cb.state().await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn multiple_providers_independent_in_registry() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let lock: Arc<dyn DistributedLock> = Arc::new(MemoryDistributedLock::new());
        let mut reg = CircuitBreakerRegistry::new(Arc::clone(&store), Arc::clone(&lock));
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
        email.record_failure().await;
        email.record_failure().await;
        assert_eq!(email.state().await, CircuitState::Open);

        // Slack should be unaffected.
        assert_eq!(slack.state().await, CircuitState::Closed);

        // Slack needs 3 failures.
        slack.record_failure().await;
        slack.record_failure().await;
        assert_eq!(slack.state().await, CircuitState::Closed);
        slack.record_failure().await;
        assert_eq!(slack.state().await, CircuitState::Open);
    }

    #[tokio::test]
    async fn success_in_open_state_does_nothing() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_secs(3600),
            ..default_config()
        };
        let cb = create_cb(config);

        // Trip the circuit.
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);

        // Recording success while Open should not change state.
        cb.record_success().await;
        assert_eq!(cb.state().await, CircuitState::Open);
    }

    #[tokio::test]
    async fn failure_in_open_state_updates_last_failure_time() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_secs(3600),
            ..default_config()
        };
        let cb = create_cb(config);

        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);

        // Additional failures should keep it Open without panicking.
        cb.record_failure().await;
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);
    }

    #[test]
    fn provider_name_accessor() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let lock: Arc<dyn DistributedLock> = Arc::new(MemoryDistributedLock::new());
        let cb = CircuitBreaker::new("my-provider", default_config(), store, lock);
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
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let lock: Arc<dyn DistributedLock> = Arc::new(MemoryDistributedLock::new());
        let cb = CircuitBreaker::new("test", config, store, lock);

        assert_eq!(cb.config().failure_threshold, 42);
        assert_eq!(cb.config().success_threshold, 7);
        assert_eq!(cb.config().recovery_timeout, Duration::from_secs(999));
        assert_eq!(cb.config().fallback_provider.as_deref(), Some("backup"));
    }

    #[tokio::test]
    async fn reset_from_half_open() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::ZERO,
            ..default_config()
        };
        let cb = create_cb(config);

        cb.record_failure().await;
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);

        cb.reset().await;
        assert_eq!(cb.state().await, CircuitState::Closed);
    }

    #[test]
    fn registry_debug_format() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let lock: Arc<dyn DistributedLock> = Arc::new(MemoryDistributedLock::new());
        let mut reg = CircuitBreakerRegistry::new(store, lock);
        reg.register("alpha", default_config());
        reg.register("beta", default_config());

        let debug = format!("{reg:?}");
        assert!(debug.contains("alpha"));
        assert!(debug.contains("beta"));
    }

    #[tokio::test]
    async fn registry_overwrite_existing_provider() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let lock: Arc<dyn DistributedLock> = Arc::new(MemoryDistributedLock::new());
        let mut reg = CircuitBreakerRegistry::new(Arc::clone(&store), Arc::clone(&lock));
        reg.register(
            "email",
            CircuitBreakerConfig {
                failure_threshold: 3,
                ..default_config()
            },
        );

        // Trip the first circuit breaker.
        let cb = reg.get("email").unwrap();
        cb.record_failure().await;
        cb.record_failure().await;
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);

        // Re-register with different config — should replace.
        // Note: old state persists in the store under the same key.
        reg.register(
            "email",
            CircuitBreakerConfig {
                failure_threshold: 10,
                ..default_config()
            },
        );

        let cb2 = reg.get("email").unwrap();
        // State comes from the store, which still has the old data.
        // But config is new.
        assert_eq!(cb2.config().failure_threshold, 10);
    }

    #[tokio::test]
    async fn full_lifecycle_multiple_cycles() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            success_threshold: 1,
            recovery_timeout: Duration::ZERO,
            fallback_provider: None,
        };
        let cb = create_cb(config);

        // Cycle 1: Closed -> Open -> HalfOpen -> Closed
        cb.record_failure().await;
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);
        cb.record_success().await;
        assert_eq!(cb.state().await, CircuitState::Closed);

        // Cycle 2: Trip again
        cb.record_failure().await;
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);

        // Cycle 2: Fail probe -> back to Open
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);

        // Cycle 2: Recover
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);
        cb.record_success().await;
        assert_eq!(cb.state().await, CircuitState::Closed);

        // Cycle 3: Immediate trip and recovery
        cb.record_failure().await;
        cb.record_failure().await;
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);
        cb.record_success().await;
        assert_eq!(cb.state().await, CircuitState::Closed);
    }

    // -- Time-based tests ----------------------------------------------------

    #[tokio::test]
    async fn recovery_timeout_zero_transitions_immediately() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 1,
            recovery_timeout: Duration::ZERO,
            fallback_provider: None,
        };
        let cb = create_cb(config);

        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);

        // With zero timeout, try_acquire_permit immediately transitions to HalfOpen.
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);
    }

    #[tokio::test]
    async fn long_recovery_timeout_stays_open() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 1,
            recovery_timeout: Duration::from_secs(3600),
            fallback_provider: None,
        };
        let cb = create_cb(config);

        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);

        // With a very long timeout, try_acquire_permit should not transition.
        assert_eq!(try_permit_state(&cb).await, CircuitState::Open);
        assert_eq!(try_permit_state(&cb).await, CircuitState::Open);
    }

    #[tokio::test]
    async fn half_open_probe_failure_requires_fresh_recovery_wait() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 1,
            recovery_timeout: Duration::ZERO,
            fallback_provider: None,
        };
        let cb = create_cb(config);

        // Trip and transition to HalfOpen.
        cb.record_failure().await;
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);

        // Fail the probe -> back to Open with fresh last_failure_time.
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);

        // With ZERO timeout, immediately back to HalfOpen.
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);

        // Succeed the probe this time.
        cb.record_success().await;
        assert_eq!(cb.state().await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn very_short_recovery_timeout_with_sleep() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 1,
            recovery_timeout: Duration::from_millis(10),
            fallback_provider: None,
        };
        let cb = create_cb(config);

        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);

        // Wait longer than the recovery timeout.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);

        cb.record_success().await;
        assert_eq!(cb.state().await, CircuitState::Closed);
    }

    // -- Concurrent stress tests ----------------------------------------------

    #[tokio::test]
    async fn concurrent_successes_in_half_open() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let lock: Arc<dyn DistributedLock> = Arc::new(MemoryDistributedLock::new());
        let cb = Arc::new(CircuitBreaker::new(
            "test",
            CircuitBreakerConfig {
                failure_threshold: 1,
                success_threshold: 2,
                recovery_timeout: Duration::ZERO,
                fallback_provider: None,
            },
            store,
            lock,
        ));

        // Trip and move to HalfOpen.
        cb.record_failure().await;
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);

        let mut handles = Vec::new();

        for _ in 0..10 {
            let cb = Arc::clone(&cb);
            handles.push(tokio::spawn(async move {
                cb.record_success().await;
            }));
        }

        for handle in handles {
            handle.await.expect("task should not panic");
        }

        // After many successes, should be Closed.
        assert_eq!(cb.state().await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn concurrent_mixed_operations_no_panic() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let lock: Arc<dyn DistributedLock> = Arc::new(MemoryDistributedLock::new());
        let cb = Arc::new(CircuitBreaker::new(
            "test",
            CircuitBreakerConfig {
                failure_threshold: 3,
                success_threshold: 2,
                recovery_timeout: Duration::ZERO,
                fallback_provider: None,
            },
            store,
            lock,
        ));

        let mut handles = Vec::new();

        for i in 0..50 {
            let cb = Arc::clone(&cb);
            handles.push(tokio::spawn(async move {
                match i % 4 {
                    0 => {
                        cb.record_failure().await;
                    }
                    1 => {
                        cb.record_success().await;
                    }
                    2 => {
                        cb.try_acquire_permit().await;
                    }
                    3 => cb.reset().await,
                    _ => unreachable!(),
                }
            }));
        }

        for handle in handles {
            handle.await.expect("task should not panic");
        }

        // No assertions on final state needed — just verify no panics.
        let _ = cb.state().await;
    }

    // -- Probe limiting tests -------------------------------------------------

    #[tokio::test]
    async fn half_open_rejects_concurrent_probes() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 1,
            recovery_timeout: Duration::ZERO,
            fallback_provider: None,
        };
        let cb = create_cb(config);

        // Trip the circuit.
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);

        // First check transitions to HalfOpen and allows the probe.
        let (state, transition) = cb.try_acquire_permit().await;
        assert_eq!(state, CircuitState::HalfOpen);
        assert_eq!(
            transition,
            Some((CircuitState::Open, CircuitState::HalfOpen))
        );

        // Second check while probe is in flight returns Open (rejected).
        let (state, transition) = cb.try_acquire_permit().await;
        assert_eq!(state, CircuitState::Open);
        assert!(transition.is_none());

        // Complete the probe successfully -> closes the circuit.
        let transition = cb.record_success().await;
        assert_eq!(
            transition,
            Some((CircuitState::HalfOpen, CircuitState::Closed))
        );
        assert_eq!(cb.state().await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn probe_in_flight_cleared_on_failure() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 1,
            recovery_timeout: Duration::ZERO,
            fallback_provider: None,
        };
        let cb = create_cb(config);

        cb.record_failure().await;
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);

        // Probe fails -> back to Open, probe cleared.
        let transition = cb.record_failure().await;
        assert_eq!(
            transition,
            Some((CircuitState::HalfOpen, CircuitState::Open))
        );

        // Can transition to HalfOpen again and allow a new probe.
        assert_eq!(try_permit_state(&cb).await, CircuitState::HalfOpen);
        cb.record_success().await;
        assert_eq!(cb.state().await, CircuitState::Closed);
    }

    // -- Transition return value tests ----------------------------------------

    #[tokio::test]
    async fn record_failure_returns_transition_on_threshold() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            ..default_config()
        };
        let cb = create_cb(config);

        // First failure: no transition.
        assert!(cb.record_failure().await.is_none());
        // Second failure: Closed -> Open transition.
        let t = cb.record_failure().await;
        assert_eq!(t, Some((CircuitState::Closed, CircuitState::Open)));
    }

    #[tokio::test]
    async fn record_success_returns_transition_on_close() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 1,
            recovery_timeout: Duration::ZERO,
            fallback_provider: None,
        };
        let cb = create_cb(config);

        cb.record_failure().await;
        try_permit_state(&cb).await; // -> HalfOpen

        let t = cb.record_success().await;
        assert_eq!(t, Some((CircuitState::HalfOpen, CircuitState::Closed)));
    }

    #[tokio::test]
    async fn record_success_in_closed_returns_none() {
        let cb = create_cb(default_config());
        assert!(cb.record_success().await.is_none());
    }

    #[tokio::test]
    async fn record_failure_in_open_returns_none() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_secs(3600),
            ..default_config()
        };
        let cb = create_cb(config);

        cb.record_failure().await; // trips to Open
        // Additional failure in Open -> no transition.
        assert!(cb.record_failure().await.is_none());
    }

    // -- Distributed state tests ----------------------------------------------

    #[tokio::test]
    async fn state_persists_across_circuit_breaker_instances() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let lock: Arc<dyn DistributedLock> = Arc::new(MemoryDistributedLock::new());

        // Instance 1: trip the circuit
        let cb1 = CircuitBreaker::new(
            "email",
            CircuitBreakerConfig {
                failure_threshold: 2,
                ..default_config()
            },
            Arc::clone(&store),
            Arc::clone(&lock),
        );
        cb1.record_failure().await;
        cb1.record_failure().await;
        assert_eq!(cb1.state().await, CircuitState::Open);

        // Instance 2: same provider, same store — should see Open state
        let cb2 = CircuitBreaker::new(
            "email",
            CircuitBreakerConfig {
                failure_threshold: 2,
                ..default_config()
            },
            Arc::clone(&store),
            Arc::clone(&lock),
        );
        assert_eq!(cb2.state().await, CircuitState::Open);
    }

    #[tokio::test]
    async fn separate_providers_have_independent_state() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let lock: Arc<dyn DistributedLock> = Arc::new(MemoryDistributedLock::new());

        let email = CircuitBreaker::new(
            "email",
            CircuitBreakerConfig {
                failure_threshold: 1,
                ..default_config()
            },
            Arc::clone(&store),
            Arc::clone(&lock),
        );
        let sms = CircuitBreaker::new(
            "sms",
            CircuitBreakerConfig {
                failure_threshold: 1,
                ..default_config()
            },
            Arc::clone(&store),
            Arc::clone(&lock),
        );

        email.record_failure().await;
        assert_eq!(email.state().await, CircuitState::Open);
        assert_eq!(sms.state().await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn cross_instance_probe_coordination() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let lock: Arc<dyn DistributedLock> = Arc::new(MemoryDistributedLock::new());
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 1,
            recovery_timeout: Duration::ZERO,
            fallback_provider: None,
        };

        let cb1 = CircuitBreaker::new(
            "email",
            config.clone(),
            Arc::clone(&store),
            Arc::clone(&lock),
        );
        let cb2 = CircuitBreaker::new("email", config, Arc::clone(&store), Arc::clone(&lock));

        // Instance 1 trips the circuit
        cb1.record_failure().await;

        // Instance 1 starts a probe
        let (state, _) = cb1.try_acquire_permit().await;
        assert_eq!(state, CircuitState::HalfOpen);

        // Instance 2 tries to probe — should be rejected (probe in flight)
        let (state, _) = cb2.try_acquire_permit().await;
        assert_eq!(state, CircuitState::Open);

        // Instance 1 completes the probe successfully
        cb1.record_success().await;
        assert_eq!(cb2.state().await, CircuitState::Closed);
    }
}
