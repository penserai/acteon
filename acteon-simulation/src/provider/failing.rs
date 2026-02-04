//! Provider that simulates various failure scenarios.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use acteon_core::{Action, ProviderResponse};
use acteon_provider::{DynProvider, ProviderError};
use async_trait::async_trait;

/// A provider that always fails or fails in specific patterns.
///
/// Useful for testing error handling and retry logic.
#[derive(Debug)]
pub struct FailingProvider {
    name: String,
    error_type: FailureType,
    call_count: AtomicUsize,
    fail_until: Option<usize>,
}

/// Type of failure to simulate.
#[derive(Debug, Clone)]
pub enum FailureType {
    /// Execution failure (non-retryable by default).
    ExecutionFailed(String),
    /// Timeout failure (retryable).
    Timeout(Duration),
    /// Connection failure (retryable).
    Connection(String),
    /// Rate limiting (retryable).
    RateLimited,
    /// Configuration error (non-retryable).
    Configuration(String),
}

impl FailingProvider {
    /// Create a new failing provider with the given name and error type.
    pub fn new(name: impl Into<String>, error_type: FailureType) -> Self {
        Self {
            name: name.into(),
            error_type,
            call_count: AtomicUsize::new(0),
            fail_until: None,
        }
    }

    /// Create a provider that fails with an execution error.
    pub fn execution_failed(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(name, FailureType::ExecutionFailed(message.into()))
    }

    /// Create a provider that fails with a timeout.
    pub fn timeout(name: impl Into<String>, duration: Duration) -> Self {
        Self::new(name, FailureType::Timeout(duration))
    }

    /// Create a provider that fails with a connection error.
    pub fn connection_error(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(name, FailureType::Connection(message.into()))
    }

    /// Create a provider that fails with rate limiting.
    pub fn rate_limited(name: impl Into<String>) -> Self {
        Self::new(name, FailureType::RateLimited)
    }

    /// Set the provider to fail only until N calls have been made,
    /// then succeed afterwards.
    #[must_use]
    pub fn fail_until(mut self, n: usize) -> Self {
        self.fail_until = Some(n);
        self
    }

    /// Get the number of calls made to this provider.
    pub fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }

    /// Reset the call counter.
    pub fn reset(&self) {
        self.call_count.store(0, Ordering::SeqCst);
    }

    fn make_error(&self) -> ProviderError {
        match &self.error_type {
            FailureType::ExecutionFailed(msg) => ProviderError::ExecutionFailed(msg.clone()),
            FailureType::Timeout(d) => ProviderError::Timeout(*d),
            FailureType::Connection(msg) => ProviderError::Connection(msg.clone()),
            FailureType::RateLimited => ProviderError::RateLimited,
            FailureType::Configuration(msg) => ProviderError::Configuration(msg.clone()),
        }
    }
}

#[async_trait]
impl DynProvider for FailingProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        let call_number = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;

        // Check if we should succeed after N failures
        if let Some(fail_until) = self.fail_until {
            if call_number > fail_until {
                return Ok(ProviderResponse::success(serde_json::json!({
                    "provider": self.name,
                    "action_id": action.id.to_string(),
                    "recovered_after": fail_until
                })));
            }
        }

        // Simulate delay for timeout errors
        if let FailureType::Timeout(duration) = &self.error_type {
            tokio::time::sleep(*duration).await;
        }

        Err(self.make_error())
    }

    async fn health_check(&self) -> Result<(), ProviderError> {
        Err(self.make_error())
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
            serde_json::json!({}),
        )
    }

    #[tokio::test]
    async fn execution_failed() {
        let provider = FailingProvider::execution_failed("test", "simulated failure");

        let result = provider.execute(&test_action()).await;

        assert!(matches!(result, Err(ProviderError::ExecutionFailed(_))));
        assert!(!result.unwrap_err().is_retryable());
    }

    #[tokio::test]
    async fn timeout_is_retryable() {
        let provider = FailingProvider::timeout("test", Duration::from_millis(10));

        let result = provider.execute(&test_action()).await;

        assert!(matches!(result, Err(ProviderError::Timeout(_))));
        assert!(result.unwrap_err().is_retryable());
    }

    #[tokio::test]
    async fn connection_error_is_retryable() {
        let provider = FailingProvider::connection_error("test", "network error");

        let result = provider.execute(&test_action()).await;

        assert!(matches!(result, Err(ProviderError::Connection(_))));
        assert!(result.unwrap_err().is_retryable());
    }

    #[tokio::test]
    async fn rate_limited_is_retryable() {
        let provider = FailingProvider::rate_limited("test");

        let result = provider.execute(&test_action()).await;

        assert!(matches!(result, Err(ProviderError::RateLimited)));
        assert!(result.unwrap_err().is_retryable());
    }

    #[tokio::test]
    async fn fail_until_then_succeed() {
        let provider = FailingProvider::execution_failed("test", "fail").fail_until(2);

        // First two calls fail
        assert!(provider.execute(&test_action()).await.is_err());
        assert!(provider.execute(&test_action()).await.is_err());

        // Third call succeeds
        assert!(provider.execute(&test_action()).await.is_ok());

        // Fourth call also succeeds
        assert!(provider.execute(&test_action()).await.is_ok());

        assert_eq!(provider.call_count(), 4);
    }

    #[tokio::test]
    async fn reset_call_count() {
        let provider = FailingProvider::execution_failed("test", "fail");

        provider.execute(&test_action()).await.unwrap_err();
        provider.execute(&test_action()).await.unwrap_err();

        assert_eq!(provider.call_count(), 2);

        provider.reset();

        assert_eq!(provider.call_count(), 0);
    }

    #[tokio::test]
    async fn health_check_fails() {
        let provider = FailingProvider::connection_error("test", "unhealthy");

        let result = provider.health_check().await;

        assert!(result.is_err());
    }
}
