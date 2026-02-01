use crate::registry::ProviderRegistry;

/// The health status of a single provider.
#[derive(Debug, Clone)]
pub struct HealthStatus {
    /// The provider name.
    pub provider: String,
    /// Whether the provider is healthy.
    pub healthy: bool,
    /// An error message if the provider is unhealthy.
    pub error: Option<String>,
}

/// Run health checks against every provider in the registry concurrently and
/// return a status entry for each one.
///
/// Providers that return `Ok(())` from their health check are marked healthy.
/// Providers that return an error are marked unhealthy with the error message
/// captured.
pub async fn check_all(registry: &ProviderRegistry) -> Vec<HealthStatus> {
    let providers: Vec<_> = registry
        .list()
        .into_iter()
        .filter_map(|name| registry.get(name).map(|p| (name.to_owned(), p)))
        .collect();

    let mut results = Vec::with_capacity(providers.len());

    for (name, provider) in providers {
        let result = provider.health_check().await;
        results.push(match result {
            Ok(()) => HealthStatus {
                provider: name,
                healthy: true,
                error: None,
            },
            Err(e) => HealthStatus {
                provider: name,
                healthy: false,
                error: Some(e.to_string()),
            },
        });
    }

    results
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use acteon_core::{Action, ProviderResponse};

    use super::*;
    use crate::error::ProviderError;
    use crate::provider::Provider;

    struct HealthyProvider;

    impl Provider for HealthyProvider {
        fn name(&self) -> &str {
            "healthy"
        }

        async fn execute(&self, _action: &Action) -> Result<ProviderResponse, ProviderError> {
            Ok(ProviderResponse::success(serde_json::Value::Null))
        }

        async fn health_check(&self) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    struct UnhealthyProvider;

    impl Provider for UnhealthyProvider {
        fn name(&self) -> &str {
            "unhealthy"
        }

        async fn execute(&self, _action: &Action) -> Result<ProviderResponse, ProviderError> {
            Err(ProviderError::ExecutionFailed("always fails".into()))
        }

        async fn health_check(&self) -> Result<(), ProviderError> {
            Err(ProviderError::Connection("connection refused".into()))
        }
    }

    #[tokio::test]
    async fn check_all_empty_registry() {
        let reg = ProviderRegistry::new();
        let statuses = check_all(&reg).await;
        assert!(statuses.is_empty());
    }

    #[tokio::test]
    async fn check_all_mixed() {
        let mut reg = ProviderRegistry::new();
        reg.register(Arc::new(HealthyProvider));
        reg.register(Arc::new(UnhealthyProvider));

        let statuses = check_all(&reg).await;
        assert_eq!(statuses.len(), 2);

        let healthy = statuses.iter().find(|s| s.provider == "healthy").unwrap();
        assert!(healthy.healthy);
        assert!(healthy.error.is_none());

        let unhealthy = statuses.iter().find(|s| s.provider == "unhealthy").unwrap();
        assert!(!unhealthy.healthy);
        assert!(unhealthy
            .error
            .as_deref()
            .unwrap()
            .contains("connection refused"));
    }

    #[tokio::test]
    async fn check_all_all_healthy() {
        let mut reg = ProviderRegistry::new();
        reg.register(Arc::new(HealthyProvider));

        let statuses = check_all(&reg).await;
        assert_eq!(statuses.len(), 1);
        assert!(statuses[0].healthy);
    }
}
