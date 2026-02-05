use std::time::Duration;

use crate::retry::RetryStrategy;

/// Configuration for the [`ActionExecutor`](crate::ActionExecutor).
///
/// Controls retry behaviour, concurrency limits, and per-action timeouts.
///
/// # Examples
///
/// ```
/// use acteon_executor::ExecutorConfig;
///
/// let config = ExecutorConfig::default();
/// assert_eq!(config.max_retries, 3);
/// assert_eq!(config.max_concurrent, 10);
/// ```
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// Maximum number of retry attempts before an action is considered failed.
    pub max_retries: u32,
    /// Strategy used to compute the delay between retries.
    pub retry_strategy: RetryStrategy,
    /// Maximum wall-clock time allowed for a single provider execution call.
    pub execution_timeout: Duration,
    /// Maximum number of actions that may execute concurrently. Enforced via a
    /// [`tokio::sync::Semaphore`].
    pub max_concurrent: usize,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            retry_strategy: RetryStrategy::default(),
            execution_timeout: Duration::from_secs(30),
            max_concurrent: 10,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let cfg = ExecutorConfig::default();
        assert_eq!(cfg.max_retries, 3);
        assert_eq!(cfg.execution_timeout, Duration::from_secs(30));
        assert_eq!(cfg.max_concurrent, 10);
    }

    #[test]
    fn config_custom_values() {
        let cfg = ExecutorConfig {
            max_retries: 5,
            retry_strategy: RetryStrategy::Constant {
                delay: Duration::from_secs(1),
            },
            execution_timeout: Duration::from_secs(60),
            max_concurrent: 50,
        };
        assert_eq!(cfg.max_retries, 5);
        assert_eq!(cfg.execution_timeout, Duration::from_secs(60));
        assert_eq!(cfg.max_concurrent, 50);
    }
}
