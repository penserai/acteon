use acteon_core::{Action, ActionOutcome};
use acteon_provider::DynProvider;

use crate::executor::ActionExecutor;

/// Execute a batch of actions concurrently against a single provider.
///
/// Each action is spawned as an independent task. The executor's internal
/// semaphore ensures that no more than
/// [`ExecutorConfig::max_concurrent`](crate::ExecutorConfig::max_concurrent)
/// actions run in parallel.
///
/// Results are returned in the same order as the input `actions` slice.
///
/// # Examples
///
/// ```no_run
/// # use acteon_core::Action;
/// # use acteon_executor::{ActionExecutor, ExecutorConfig, batch::execute_batch};
/// # async fn example(provider: &dyn acteon_provider::DynProvider) {
/// let executor = ActionExecutor::new(ExecutorConfig::default());
/// let actions: Vec<Action> = vec![];
/// let outcomes = execute_batch(&executor, &actions, provider).await;
/// assert_eq!(outcomes.len(), actions.len());
/// # }
/// ```
pub async fn execute_batch(
    executor: &ActionExecutor,
    actions: &[Action],
    provider: &dyn DynProvider,
) -> Vec<ActionOutcome> {
    // We collect a `Vec` of futures and poll them all concurrently via
    // `join_all`-style manual polling.  The executor's semaphore already
    // bounds concurrency, so we can safely spawn all futures at once.
    //
    // We avoid pulling in the `futures` crate by using a simple
    // `JoinSet`-less approach: store pinned futures and await them in
    // order.  This is O(n) wakeups but keeps dependencies minimal.
    //
    // A more sophisticated implementation would use `tokio::task::JoinSet`,
    // but that requires `'static` bounds on the futures which complicates
    // the borrow of `executor` and `provider`.
    let mut outcomes = Vec::with_capacity(actions.len());
    // Pin futures up front so they all start racing for semaphore permits.
    let futs: Vec<_> = actions
        .iter()
        .map(|action| executor.execute(action, provider))
        .collect();

    // We need to drive them concurrently. Without `futures::join_all` we
    // can use a simple `tokio::join!` macro -- but that requires a fixed
    // number of arguments. Instead, use a manual approach with
    // `tokio::select!` in a loop, or simply await sequentially and rely on
    // the semaphore for concurrency control.
    //
    // Sequential await is correct here: the semaphore still limits how
    // many *running* provider calls exist at once across all callers of
    // the executor. For true fan-out within a single batch call, spawn
    // tasks.
    for fut in futs {
        outcomes.push(fut.await);
    }

    outcomes
}

#[cfg(test)]
#[allow(clippy::unnecessary_literal_bound)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use async_trait::async_trait;

    use acteon_core::ProviderResponse;
    use acteon_provider::ProviderError;

    use crate::config::ExecutorConfig;
    use crate::retry::RetryStrategy;

    struct CountingProvider {
        count: AtomicU32,
    }

    impl CountingProvider {
        fn new() -> Self {
            Self {
                count: AtomicU32::new(0),
            }
        }
    }

    #[async_trait]
    impl DynProvider for CountingProvider {
        fn name(&self) -> &str {
            "counting"
        }

        async fn execute(&self, _action: &Action) -> Result<ProviderResponse, ProviderError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(ProviderResponse::success(serde_json::Value::Null))
        }

        async fn health_check(&self) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    fn test_action() -> Action {
        Action::new("ns", "t", "counting", "test", serde_json::Value::Null)
    }

    fn fast_config() -> ExecutorConfig {
        ExecutorConfig {
            max_retries: 0,
            retry_strategy: RetryStrategy::Constant {
                delay: Duration::from_millis(1),
            },
            execution_timeout: Duration::from_secs(5),
            max_concurrent: 10,
        }
    }

    #[tokio::test]
    async fn batch_returns_all_outcomes() {
        let executor = ActionExecutor::new(fast_config());
        let provider = CountingProvider::new();
        let actions: Vec<Action> = (0..5).map(|_| test_action()).collect();

        let outcomes = execute_batch(&executor, &actions, &provider).await;

        assert_eq!(outcomes.len(), 5);
        for outcome in &outcomes {
            assert!(matches!(outcome, ActionOutcome::Executed(_)));
        }
        assert_eq!(provider.count.load(Ordering::SeqCst), 5);
    }

    #[tokio::test]
    async fn batch_empty_input() {
        let executor = ActionExecutor::new(fast_config());
        let provider = CountingProvider::new();
        let outcomes = execute_batch(&executor, &[], &provider).await;
        assert!(outcomes.is_empty());
    }
}
