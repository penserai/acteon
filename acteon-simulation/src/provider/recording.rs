//! Recording provider that captures all calls for verification.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acteon_core::{Action, ProviderResponse, ResponseStatus};
use acteon_provider::{DynProvider, ProviderError};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;

/// A provider that records all calls for later verification.
///
/// This is useful for testing that actions are dispatched correctly
/// and that the correct parameters are passed to providers.
/// Type alias for the response function.
type ResponseFn = dyn Fn(&Action) -> Result<ProviderResponse, ProviderError> + Send + Sync;

pub struct RecordingProvider {
    name: String,
    calls: Arc<Mutex<Vec<CapturedCall>>>,
    call_count: AtomicUsize,
    response_fn: Option<Arc<ResponseFn>>,
    delay: Option<Duration>,
    failure_mode: FailureMode,
}

impl std::fmt::Debug for RecordingProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordingProvider")
            .field("name", &self.name)
            .field("call_count", &self.call_count.load(Ordering::SeqCst))
            .field("delay", &self.delay)
            .field("failure_mode", &self.failure_mode)
            .finish_non_exhaustive()
    }
}

/// A captured call to the provider.
#[derive(Debug, Clone)]
pub struct CapturedCall {
    /// Timestamp when the call was made.
    pub timestamp: DateTime<Utc>,
    /// The action that was executed.
    pub action: Action,
    /// The result of the call.
    pub response: Result<ProviderResponse, String>,
    /// Duration of the call.
    pub duration: Duration,
}

/// Mode for simulating failures.
#[derive(Debug, Clone, Default)]
pub enum FailureMode {
    /// Never fail.
    #[default]
    None,
    /// Fail every N calls.
    EveryN(usize),
    /// Fail with probability p (0.0 to 1.0).
    Probabilistic(f64),
    /// Fail the first N calls.
    FirstN(usize),
    /// Always fail.
    Always,
}

impl RecordingProvider {
    /// Create a new recording provider with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            calls: Arc::new(Mutex::new(Vec::new())),
            call_count: AtomicUsize::new(0),
            response_fn: None,
            delay: None,
            failure_mode: FailureMode::None,
        }
    }

    /// Set a custom response function.
    #[must_use]
    pub fn with_response_fn<F>(mut self, f: F) -> Self
    where
        F: Fn(&Action) -> Result<ProviderResponse, ProviderError> + Send + Sync + 'static,
    {
        self.response_fn = Some(Arc::new(f));
        self
    }

    /// Set a delay before responding.
    #[must_use]
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }

    /// Set the failure mode.
    #[must_use]
    pub fn with_failure_mode(mut self, mode: FailureMode) -> Self {
        self.failure_mode = mode;
        self
    }

    /// Get all captured calls.
    pub fn calls(&self) -> Vec<CapturedCall> {
        self.calls.lock().clone()
    }

    /// Get the number of calls.
    pub fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }

    /// Clear all captured calls.
    pub fn clear(&self) {
        self.calls.lock().clear();
        self.call_count.store(0, Ordering::SeqCst);
    }

    /// Assert that the provider was called exactly N times.
    ///
    /// # Panics
    ///
    /// Panics if the provider was not called exactly N times.
    pub fn assert_called(&self, n: usize) {
        let count = self.call_count();
        assert_eq!(
            count, n,
            "expected {} calls to provider '{}', got {}",
            n, self.name, count
        );
    }

    /// Assert that the provider was called at least N times.
    ///
    /// # Panics
    ///
    /// Panics if the provider was called fewer than N times.
    pub fn assert_called_at_least(&self, n: usize) {
        let count = self.call_count();
        assert!(
            count >= n,
            "expected at least {} calls to provider '{}', got {}",
            n,
            self.name,
            count
        );
    }

    /// Assert that the provider was not called.
    ///
    /// # Panics
    ///
    /// Panics if the provider was called.
    pub fn assert_not_called(&self) {
        let count = self.call_count();
        assert_eq!(
            count, 0,
            "expected no calls to provider '{}', got {}",
            self.name, count
        );
    }

    /// Get the last captured call, if any.
    pub fn last_call(&self) -> Option<CapturedCall> {
        self.calls.lock().last().cloned()
    }

    /// Get the last action that was executed, if any.
    pub fn last_action(&self) -> Option<Action> {
        self.last_call().map(|c| c.action)
    }

    /// Check if the provider should fail for this call number.
    fn should_fail(&self, call_number: usize) -> bool {
        match &self.failure_mode {
            FailureMode::None => false,
            FailureMode::EveryN(n) => call_number.is_multiple_of(*n),
            FailureMode::Probabilistic(p) => rand::random::<f64>() < *p,
            FailureMode::FirstN(n) => call_number <= *n,
            FailureMode::Always => true,
        }
    }
}

#[async_trait]
impl DynProvider for RecordingProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        let start = std::time::Instant::now();
        let call_number = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;

        // Apply delay if configured
        if let Some(delay) = self.delay {
            tokio::time::sleep(delay).await;
        }

        // Check failure mode
        let result = if self.should_fail(call_number) {
            Err(ProviderError::ExecutionFailed(format!(
                "simulated failure on call #{call_number}"
            )))
        } else if let Some(ref response_fn) = self.response_fn {
            response_fn(action)
        } else {
            Ok(ProviderResponse {
                status: ResponseStatus::Success,
                body: serde_json::json!({
                    "provider": self.name,
                    "action_id": action.id.to_string(),
                    "simulated": true
                }),
                headers: std::collections::HashMap::new(),
            })
        };

        let duration = start.elapsed();

        // Record the call - capture before returning
        let response_for_capture = match &result {
            Ok(resp) => Ok(resp.clone()),
            Err(e) => Err(e.to_string()),
        };

        let captured = CapturedCall {
            timestamp: Utc::now(),
            action: action.clone(),
            response: response_for_capture,
            duration,
        };
        self.calls.lock().push(captured);

        result
    }

    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_action() -> Action {
        Action::new(
            "test-ns",
            "test-tenant",
            "test-provider",
            "test-action",
            serde_json::json!({"key": "value"}),
        )
    }

    #[tokio::test]
    async fn records_calls() {
        let provider = RecordingProvider::new("test");

        provider.execute(&test_action()).await.unwrap();
        provider.execute(&test_action()).await.unwrap();

        assert_eq!(provider.call_count(), 2);
        assert_eq!(provider.calls().len(), 2);
    }

    #[tokio::test]
    async fn clear_resets_state() {
        let provider = RecordingProvider::new("test");

        provider.execute(&test_action()).await.unwrap();
        provider.clear();

        assert_eq!(provider.call_count(), 0);
        assert!(provider.calls().is_empty());
    }

    #[tokio::test]
    async fn custom_response_fn() {
        let provider = RecordingProvider::new("test").with_response_fn(|_action| {
            Ok(ProviderResponse::success(serde_json::json!({"custom": true})))
        });

        let response = provider.execute(&test_action()).await.unwrap();
        assert_eq!(response.body["custom"], true);
    }

    #[tokio::test]
    async fn failure_mode_always() {
        let provider = RecordingProvider::new("test").with_failure_mode(FailureMode::Always);

        let result = provider.execute(&test_action()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn failure_mode_first_n() {
        let provider = RecordingProvider::new("test").with_failure_mode(FailureMode::FirstN(2));

        // First two calls should fail
        assert!(provider.execute(&test_action()).await.is_err());
        assert!(provider.execute(&test_action()).await.is_err());

        // Third call should succeed
        assert!(provider.execute(&test_action()).await.is_ok());
    }

    #[tokio::test]
    async fn failure_mode_every_n() {
        let provider = RecordingProvider::new("test").with_failure_mode(FailureMode::EveryN(2));

        // Call 1 - fails (1 % 2 != 0, but we're 1-indexed so 2 would fail)
        // Actually: call_number starts at 1, so EveryN(2) fails on calls 2, 4, 6...
        assert!(provider.execute(&test_action()).await.is_ok()); // call 1
        assert!(provider.execute(&test_action()).await.is_err()); // call 2
        assert!(provider.execute(&test_action()).await.is_ok()); // call 3
        assert!(provider.execute(&test_action()).await.is_err()); // call 4
    }

    #[tokio::test]
    async fn assert_methods() {
        let provider = RecordingProvider::new("test");

        provider.assert_not_called();

        provider.execute(&test_action()).await.unwrap();

        provider.assert_called(1);
        provider.assert_called_at_least(1);
    }

    #[tokio::test]
    async fn last_call_and_action() {
        let provider = RecordingProvider::new("test");

        assert!(provider.last_call().is_none());
        assert!(provider.last_action().is_none());

        let action = test_action();
        provider.execute(&action).await.unwrap();

        assert!(provider.last_call().is_some());
        let last_action = provider.last_action().unwrap();
        assert_eq!(last_action.action_type, action.action_type);
    }

    #[tokio::test]
    async fn delay_is_applied() {
        let provider = RecordingProvider::new("test").with_delay(Duration::from_millis(50));

        let start = std::time::Instant::now();
        provider.execute(&test_action()).await.unwrap();
        let elapsed = start.elapsed();

        assert!(elapsed >= Duration::from_millis(50));
    }
}
