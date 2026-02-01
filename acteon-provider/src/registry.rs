use std::collections::HashMap;
use std::sync::Arc;

use crate::provider::DynProvider;

/// A registry that maps provider names to their implementations.
///
/// Providers are stored behind `Arc<dyn DynProvider>` so they can be shared
/// across tasks safely. The registry itself is not thread-safe for mutation;
/// it is intended to be built once at startup and then shared as an immutable
/// reference or wrapped in an `Arc`.
pub struct ProviderRegistry {
    providers: HashMap<String, Arc<dyn DynProvider>>,
}

impl ProviderRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
        }
    }

    /// Register a provider. The provider's name (from [`DynProvider::name`])
    /// is used as the lookup key.
    ///
    /// If a provider with the same name already exists, it is replaced.
    pub fn register(&mut self, provider: Arc<dyn DynProvider>) {
        let name = provider.name().to_owned();
        self.providers.insert(name, provider);
    }

    /// Look up a provider by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn DynProvider>> {
        self.providers.get(name).cloned()
    }

    /// Return a sorted list of all registered provider names.
    pub fn list(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.providers.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }

    /// Return the number of registered providers.
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// Return `true` if no providers are registered.
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use acteon_core::{Action, ProviderResponse};

    use super::*;
    use crate::error::ProviderError;
    use crate::provider::Provider;

    struct StubProvider {
        stub_name: String,
    }

    impl StubProvider {
        fn new(name: &str) -> Self {
            Self {
                stub_name: name.to_owned(),
            }
        }
    }

    impl Provider for StubProvider {
        fn name(&self) -> &str {
            &self.stub_name
        }

        async fn execute(&self, _action: &Action) -> Result<ProviderResponse, ProviderError> {
            Ok(ProviderResponse::success(serde_json::json!({"stub": true})))
        }

        async fn health_check(&self) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    #[test]
    fn empty_registry() {
        let reg = ProviderRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert!(reg.list().is_empty());
    }

    #[test]
    fn register_and_get() {
        let mut reg = ProviderRegistry::new();
        reg.register(Arc::new(StubProvider::new("email")));
        reg.register(Arc::new(StubProvider::new("sms")));

        assert_eq!(reg.len(), 2);
        assert!(!reg.is_empty());

        let provider = reg.get("email").expect("email provider should exist");
        assert_eq!(provider.name(), "email");

        assert!(reg.get("push").is_none());
    }

    #[test]
    fn list_sorted() {
        let mut reg = ProviderRegistry::new();
        reg.register(Arc::new(StubProvider::new("sms")));
        reg.register(Arc::new(StubProvider::new("email")));
        reg.register(Arc::new(StubProvider::new("push")));

        assert_eq!(reg.list(), vec!["email", "push", "sms"]);
    }

    #[test]
    fn register_replaces_existing() {
        let mut reg = ProviderRegistry::new();
        reg.register(Arc::new(StubProvider::new("email")));
        reg.register(Arc::new(StubProvider::new("email")));
        assert_eq!(reg.len(), 1);
    }

    #[tokio::test]
    async fn execute_through_registry() {
        let mut reg = ProviderRegistry::new();
        reg.register(Arc::new(StubProvider::new("email")));

        let provider = reg.get("email").unwrap();
        let action = Action::new("ns", "t", "email", "send", serde_json::Value::Null);
        let resp = provider.execute(&action).await.unwrap();
        assert_eq!(resp.status, acteon_core::ResponseStatus::Success);
    }

    #[test]
    fn default_is_empty() {
        let reg = ProviderRegistry::default();
        assert!(reg.is_empty());
    }
}
