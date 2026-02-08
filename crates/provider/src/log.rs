use acteon_core::{Action, ProviderResponse};
use tracing::info;

use crate::error::ProviderError;
use crate::provider::Provider;

/// A provider that logs the action and returns success without performing any
/// external I/O.
///
/// Useful for local development, simulations, and testing scenarios where you
/// don't have (or need) a real provider endpoint.
pub struct LogProvider {
    name: String,
}

impl LogProvider {
    /// Create a new `LogProvider` with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl Provider for LogProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        info!(
            provider = %self.name,
            action_id = %action.id,
            action_type = %action.action_type,
            namespace = %action.namespace,
            tenant = %action.tenant,
            "log provider executed action"
        );
        Ok(ProviderResponse::success(serde_json::json!({
            "provider": self.name,
            "logged": true,
        })))
    }

    #[allow(clippy::unused_async)]
    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_provider_name() {
        let provider = LogProvider::new("test-log");
        assert_eq!(Provider::name(&provider), "test-log");
    }

    #[tokio::test]
    async fn log_provider_execute_returns_success() {
        let provider = LogProvider::new("my-log");
        let action = Action::new("ns", "t", "my-log", "do_thing", serde_json::Value::Null);
        let resp = Provider::execute(&provider, &action).await.unwrap();
        assert_eq!(resp.status, acteon_core::ResponseStatus::Success);
    }

    #[tokio::test]
    async fn log_provider_health_check() {
        let provider = LogProvider::new("my-log");
        Provider::health_check(&provider).await.unwrap();
    }
}
