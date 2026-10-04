use std::sync::Arc;

use tokio::sync::Semaphore;
use tracing::{debug, instrument, warn};

use acteon_core::{Action, ActionError, ActionOutcome};
use acteon_provider::{DispatchContext, DynProvider, ProviderError};

use crate::config::ExecutorConfig;
use crate::dlq::DeadLetterSink;
use crate::gate::{
    AttemptSettlement, ProviderAttempt, ProviderAttemptGate, ProviderAttemptOutcome,
};

/// Executes actions against a provider with retry logic and bounded concurrency.
///
/// The executor acquires a semaphore permit before each action execution so
/// that at most [`ExecutorConfig::max_concurrent`] actions run in parallel.
/// Failed attempts that yield a retryable error are retried up to
/// [`ExecutorConfig::max_retries`] times with delays computed by the
/// configured [`RetryStrategy`](crate::RetryStrategy).
///
/// When a dead-letter queue sink is configured, actions that exhaust all retries
/// are pushed to the sink before returning [`ActionOutcome::Failed`].
pub struct ActionExecutor {
    clock: Arc<dyn acteon_time::Clock>,
    config: ExecutorConfig,
    semaphore: Arc<Semaphore>,
    dlq: Option<Arc<dyn DeadLetterSink>>,
    require_attempt_gate: bool,
}

impl ActionExecutor {
    /// Create a new executor from the given configuration.
    ///
    /// # Examples
    ///
    /// ```
    /// use acteon_executor::{ActionExecutor, ExecutorConfig};
    ///
    /// let executor = ActionExecutor::new(ExecutorConfig::default());
    /// ```
    pub fn new(config: ExecutorConfig) -> Self {
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent));
        Self {
            clock: Arc::new(acteon_time::SystemClock::default()),
            config,
            semaphore,
            dlq: None,
            require_attempt_gate: false,
        }
    }

    /// Create a new executor with a dead-letter queue sink.
    ///
    /// Actions that exhaust all retry attempts will be pushed to the sink.
    pub fn with_dlq(config: ExecutorConfig, dlq: Arc<dyn DeadLetterSink>) -> Self {
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent));
        Self {
            clock: Arc::new(acteon_time::SystemClock::default()),
            config,
            semaphore,
            dlq: Some(dlq),
            require_attempt_gate: false,
        }
    }

    /// Use a shared clock for execution deadlines and retry delays.
    #[must_use]
    pub fn clock(mut self, clock: Arc<dyn acteon_time::Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Refuse all ungated calls, including attachment and batch entrypoints.
    /// This library setting does not enable gateway-wide permit enforcement.
    #[must_use]
    pub fn require_attempt_gate(mut self) -> Self {
        self.require_attempt_gate = true;
        self
    }

    /// Execute with a trusted gate after waits and before every provider attempt.
    /// A fresh durable guard is required for each retry. Ambiguous settlement or
    /// settlement failure stops automatic retries without DLQ redispatch.
    /// Legacy DLQ entries cannot retain authority; gated failures are returned
    /// without writing that sink. The host must retain governed result/handoff
    /// evidence through its registered-attempt adapter.
    #[instrument(skip(self, action, provider, ctx, gate), fields(action.id = %action.id, attempt, otel.status_code))]
    pub async fn execute_with_gate(
        &self,
        action: &Action,
        provider: &dyn DynProvider,
        ctx: Option<&DispatchContext>,
        gate: &dyn ProviderAttemptGate,
    ) -> ActionOutcome {
        self.execute_inner(action, provider, ctx, Some(gate)).await
    }

    /// Return a reference to the executor configuration.
    pub fn config(&self) -> &ExecutorConfig {
        &self.config
    }

    /// Return a reference to the dead-letter queue sink, if configured.
    pub fn dlq(&self) -> Option<&Arc<dyn DeadLetterSink>> {
        self.dlq.as_ref()
    }

    /// Push an action to the dead-letter queue if configured.
    async fn push_to_dlq(&self, action: &Action, error: &str, attempts: u32) {
        if let Some(ref dlq) = self.dlq {
            debug!(
                action_id = %action.id,
                attempts,
                "pushing failed action to dead-letter queue"
            );
            if dlq
                .push(action.clone(), error.to_owned(), attempts)
                .await
                .is_err()
            {
                tracing::error!(action_id = %action.id, "failed action was not retained in dead-letter storage");
            }
        }
    }

    /// Execute an action against the given provider.
    ///
    /// The method acquires a concurrency permit, then enters a retry loop.
    /// On a retryable error the executor sleeps for the duration prescribed
    /// by the retry strategy before attempting again. Non-retryable errors
    /// cause an immediate failure.
    ///
    /// Returns [`ActionOutcome::Executed`] on success or
    /// [`ActionOutcome::Failed`] after all retries are exhausted or on a
    /// non-retryable error.
    #[instrument(skip(self, action, provider), fields(action.id = %action.id, attempt, otel.status_code))]
    pub async fn execute(&self, action: &Action, provider: &dyn DynProvider) -> ActionOutcome {
        self.execute_inner(action, provider, None, None).await
    }

    /// Execute an action with a [`DispatchContext`] (e.g. resolved attachments).
    ///
    /// Identical to [`execute`](Self::execute) but calls
    /// [`DynProvider::execute_with_context`] instead of
    /// [`DynProvider::execute`], passing the context on each attempt.
    #[instrument(skip(self, action, provider, ctx), fields(action.id = %action.id, attempt, otel.status_code))]
    pub async fn execute_with_context(
        &self,
        action: &Action,
        provider: &dyn DynProvider,
        ctx: &DispatchContext,
    ) -> ActionOutcome {
        self.execute_inner(action, provider, Some(ctx), None).await
    }

    /// Shared retry/timeout/concurrency loop used by both [`execute`] and
    /// [`execute_with_context`].
    #[allow(clippy::too_many_lines)]
    async fn execute_inner(
        &self,
        action: &Action,
        provider: &dyn DynProvider,
        ctx: Option<&DispatchContext>,
        gate: Option<&dyn ProviderAttemptGate>,
    ) -> ActionOutcome {
        if self.require_attempt_gate && gate.is_none() {
            return gate_failure("ATTEMPT_GATE_REQUIRED", 0);
        }
        // Acquire a concurrency permit. This is cancel-safe: if the caller
        // drops the future while waiting, the permit is never acquired.
        let _permit = self
            .semaphore
            .acquire()
            .await
            .expect("semaphore should never be closed");

        let mut last_error: Option<ProviderError> = None;

        for attempt in 0..=self.config.max_retries {
            tracing::Span::current().record("attempt", attempt);
            debug!(
                action_id = %action.id,
                attempt,
                max_retries = self.config.max_retries,
                "executing action"
            );

            let registered = if let Some(gate) = gate {
                match gate
                    .start(ProviderAttempt {
                        action,
                        provider_name: provider.name(),
                        ordinal: attempt,
                        now_ms: self.clock.now().timestamp_millis(),
                    })
                    .await
                {
                    Ok(registered) => Some(registered),
                    Err(error) => return gate_failure(error.code(), attempt),
                }
            } else {
                None
            };

            let result = acteon_time::timeout(
                self.clock.as_ref(),
                self.config.execution_timeout,
                match ctx {
                    Some(c) => provider.execute_with_context(action, c),
                    None => provider.execute(action),
                },
            )
            .await;

            if let Some(registered) = registered {
                let outcome = match &result {
                    Ok(Ok(response)) => ProviderAttemptOutcome::Succeeded(response),
                    Ok(Err(error)) => ProviderAttemptOutcome::Failed(error),
                    Err(_) => ProviderAttemptOutcome::TimedOut,
                };
                match registered.finish(outcome).await {
                    Ok(AttemptSettlement::Settled) => {}
                    Ok(AttemptSettlement::Uncertain) => {
                        return gate_failure("ATTEMPT_UNCERTAIN", attempt + 1);
                    }
                    Err(_) => return gate_failure("ATTEMPT_SETTLEMENT_REQUIRED", attempt + 1),
                }
            }

            match result {
                Ok(Ok(response)) => {
                    debug!(action_id = %action.id, "action executed successfully");
                    return ActionOutcome::Executed(response);
                }
                Ok(Err(err)) => {
                    if err.is_retryable() && attempt < self.config.max_retries {
                        let delay = self.config.retry_strategy.delay_for(attempt);
                        warn!(
                            action_id = %action.id,
                            attempt,
                            error = %err,
                            delay_ms = %delay.as_millis(),
                            "retryable error, will retry"
                        );
                        self.clock.sleep(delay).await;
                        last_error = Some(err);
                    } else {
                        // Non-retryable or final attempt.
                        let attempts = attempt + 1;
                        let is_retryable = err.is_retryable();
                        warn!(
                            action_id = %action.id,
                            attempt,
                            error = %err,
                            retryable = is_retryable,
                            "action failed"
                        );
                        // Push to DLQ only if retries were exhausted (not for non-retryable errors).
                        if is_retryable && gate.is_none() {
                            self.push_to_dlq(action, &err.to_string(), attempts).await;
                        }
                        tracing::Span::current().record("otel.status_code", "ERROR");
                        return ActionOutcome::Failed(ActionError {
                            code: error_code(&err),
                            message: err.to_string(),
                            retryable: is_retryable && gate.is_none(),
                            attempts,
                        });
                    }
                }
                Err(_elapsed) => {
                    let err = ProviderError::Timeout(self.config.execution_timeout);
                    if attempt < self.config.max_retries {
                        let delay = self.config.retry_strategy.delay_for(attempt);
                        warn!(
                            action_id = %action.id,
                            attempt,
                            timeout = ?self.config.execution_timeout,
                            delay_ms = %delay.as_millis(),
                            "execution timed out, will retry"
                        );
                        self.clock.sleep(delay).await;
                        last_error = Some(err);
                    } else {
                        let attempts = attempt + 1;
                        warn!(
                            action_id = %action.id,
                            attempt,
                            "execution timed out, no retries left"
                        );
                        if gate.is_none() {
                            self.push_to_dlq(action, &err.to_string(), attempts).await;
                        }
                        tracing::Span::current().record("otel.status_code", "ERROR");
                        return ActionOutcome::Failed(ActionError {
                            code: error_code(&err),
                            message: err.to_string(),
                            retryable: gate.is_none(),
                            attempts,
                        });
                    }
                }
            }
        }

        // Should only be reached if max_retries > 0 and every attempt was
        // retryable.  Turn the last seen error into a final failure.
        let err = last_error.expect("at least one error must have occurred");
        let attempts = self.config.max_retries + 1;
        if gate.is_none() {
            self.push_to_dlq(action, &err.to_string(), attempts).await;
        }
        tracing::Span::current().record("otel.status_code", "ERROR");
        ActionOutcome::Failed(ActionError {
            code: error_code(&err),
            message: err.to_string(),
            retryable: err.is_retryable() && gate.is_none(),
            attempts,
        })
    }
}

fn gate_failure(code: &str, attempts: u32) -> ActionOutcome {
    tracing::Span::current().record("otel.status_code", "ERROR");
    ActionOutcome::Failed(ActionError {
        code: code.into(),
        message: "Provider execution requires authority or settlement review".into(),
        retryable: false,
        attempts,
    })
}

/// Map a [`ProviderError`] variant to a short error code string.
fn error_code(err: &ProviderError) -> String {
    match err {
        ProviderError::NotFound(_) => "NOT_FOUND".into(),
        ProviderError::ExecutionFailed(_) => "EXECUTION_FAILED".into(),
        ProviderError::Timeout(_) => "TIMEOUT".into(),
        ProviderError::Connection(_) => "CONNECTION".into(),
        ProviderError::Configuration(_) => "CONFIGURATION".into(),
        ProviderError::RateLimited => "RATE_LIMITED".into(),
        ProviderError::Serialization(_) => "SERIALIZATION".into(),
    }
}

#[cfg(test)]
#[allow(clippy::unnecessary_literal_bound)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use async_trait::async_trait;
    use tokio::sync::Barrier;

    use acteon_core::ProviderResponse;

    use crate::retry::RetryStrategy;

    // -- Mock providers (defined before usage) --------------------------------

    struct MockProvider {
        should_fail: bool,
        retryable: bool,
    }

    impl MockProvider {
        fn success() -> Self {
            Self {
                should_fail: false,
                retryable: false,
            }
        }

        fn failing(retryable: bool) -> Self {
            Self {
                should_fail: true,
                retryable,
            }
        }
    }

    #[async_trait]
    impl DynProvider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }

        async fn execute(&self, _action: &Action) -> Result<ProviderResponse, ProviderError> {
            if self.should_fail {
                if self.retryable {
                    Err(ProviderError::Connection("transient".into()))
                } else {
                    Err(ProviderError::ExecutionFailed("permanent".into()))
                }
            } else {
                Ok(ProviderResponse::success(serde_json::json!({"ok": true})))
            }
        }

        async fn health_check(&self) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    /// Provider that fails the first N calls then succeeds.
    struct FlakyProvider {
        failures_left: AtomicU32,
    }

    impl FlakyProvider {
        fn new(failures: u32) -> Self {
            Self {
                failures_left: AtomicU32::new(failures),
            }
        }
    }

    #[async_trait]
    impl DynProvider for FlakyProvider {
        fn name(&self) -> &str {
            "flaky"
        }

        async fn execute(&self, _action: &Action) -> Result<ProviderResponse, ProviderError> {
            let remaining = self.failures_left.fetch_sub(1, Ordering::SeqCst);
            if remaining > 0 {
                Err(ProviderError::Connection("flaky".into()))
            } else {
                Ok(ProviderResponse::success(
                    serde_json::json!({"recovered": true}),
                ))
            }
        }

        async fn health_check(&self) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    /// Provider that sleeps longer than any reasonable timeout.
    struct SlowProvider;

    #[async_trait]
    impl DynProvider for SlowProvider {
        fn name(&self) -> &str {
            "slow"
        }

        async fn execute(&self, _action: &Action) -> Result<ProviderResponse, ProviderError> {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            Ok(ProviderResponse::success(serde_json::Value::Null))
        }

        async fn health_check(&self) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    /// A provider that increments a counter and waits at a barrier so we can
    /// observe concurrency.
    struct BarrierProvider {
        count: Arc<AtomicU32>,
        barrier: Arc<Barrier>,
    }

    #[async_trait]
    impl DynProvider for BarrierProvider {
        fn name(&self) -> &str {
            "barrier"
        }

        async fn execute(&self, _action: &Action) -> Result<ProviderResponse, ProviderError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            self.barrier.wait().await;
            Ok(ProviderResponse::success(serde_json::Value::Null))
        }

        async fn health_check(&self) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    // -- Helpers --------------------------------------------------------------

    fn test_action() -> Action {
        Action::new("ns", "t", "mock", "test", serde_json::Value::Null)
    }

    fn fast_config() -> ExecutorConfig {
        ExecutorConfig {
            max_retries: 3,
            retry_strategy: RetryStrategy::Constant {
                delay: Duration::from_millis(1),
            },
            execution_timeout: Duration::from_secs(5),
            max_concurrent: 10,
        }
    }

    // -- Tests ---------------------------------------------------------------

    #[tokio::test]
    async fn execute_success() {
        let executor = ActionExecutor::new(fast_config());
        let provider = MockProvider::success();
        let outcome = executor.execute(&test_action(), &provider).await;
        assert!(matches!(outcome, ActionOutcome::Executed(_)));
    }

    #[tokio::test]
    async fn execute_non_retryable_fails_immediately() {
        let executor = ActionExecutor::new(fast_config());
        let provider = MockProvider::failing(false);
        let outcome = executor.execute(&test_action(), &provider).await;
        match outcome {
            ActionOutcome::Failed(err) => {
                assert!(!err.retryable);
                assert_eq!(err.attempts, 1, "should fail on first attempt");
                assert_eq!(err.code, "EXECUTION_FAILED");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn execute_retries_on_retryable_error() {
        let executor = ActionExecutor::new(fast_config());
        let provider = MockProvider::failing(true);
        let outcome = executor.execute(&test_action(), &provider).await;
        match outcome {
            ActionOutcome::Failed(err) => {
                assert!(err.retryable);
                assert_eq!(err.attempts, 4, "1 initial + 3 retries");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn execute_recovers_after_retries() {
        let executor = ActionExecutor::new(fast_config());
        // Fail twice then succeed -- should recover on attempt index 2.
        let provider = FlakyProvider::new(2);
        let outcome = executor.execute(&test_action(), &provider).await;
        assert!(
            matches!(outcome, ActionOutcome::Executed(_)),
            "should recover after transient failures"
        );
    }

    #[tokio::test]
    async fn execute_timeout() {
        tokio::time::pause();
        let config = ExecutorConfig {
            max_retries: 0,
            retry_strategy: RetryStrategy::Constant {
                delay: Duration::from_millis(1),
            },
            execution_timeout: Duration::from_millis(100),
            max_concurrent: 10,
        };
        let executor = ActionExecutor::new(config);
        let provider = SlowProvider;
        let outcome = executor.execute(&test_action(), &provider).await;
        match outcome {
            ActionOutcome::Failed(err) => {
                assert_eq!(err.code, "TIMEOUT");
                assert!(err.retryable);
            }
            other => panic!("expected Failed(Timeout), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn concurrent_execution_respects_semaphore() {
        let config = ExecutorConfig {
            max_retries: 0,
            retry_strategy: RetryStrategy::default(),
            execution_timeout: Duration::from_secs(5),
            max_concurrent: 2,
        };
        let executor = Arc::new(ActionExecutor::new(config));

        let call_count = Arc::new(AtomicU32::new(0));
        let barrier = Arc::new(Barrier::new(2));

        let provider = Arc::new(BarrierProvider {
            count: Arc::clone(&call_count),
            barrier: Arc::clone(&barrier),
        });

        let mut handles = Vec::new();
        for _ in 0..2 {
            let exec = Arc::clone(&executor);
            let prov = Arc::clone(&provider);
            handles.push(tokio::spawn(async move {
                let action = test_action();
                exec.execute(&action, prov.as_ref()).await
            }));
        }

        for handle in handles {
            let outcome = handle.await.expect("task should not panic");
            assert!(matches!(outcome, ActionOutcome::Executed(_)));
        }

        // Both tasks ran.
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }
}
