use acteon_core::{Action, ProviderResponse};
use async_trait::async_trait;

use crate::context::DispatchContext;
use crate::error::ProviderError;

/// Strongly-typed provider trait with native `async fn`.
///
/// This trait is **not** object-safe because it uses native `async fn` methods
/// (which desugar to opaque `impl Future` return types). If you need dynamic
/// dispatch, use [`DynProvider`] instead -- every `Provider` automatically
/// implements `DynProvider` via a blanket implementation.
pub trait Provider: Send + Sync {
    /// Returns the unique name of this provider.
    fn name(&self) -> &str;

    /// Execute the given action and return a provider response.
    fn execute(
        &self,
        action: &Action,
    ) -> impl std::future::Future<Output = Result<ProviderResponse, ProviderError>> + Send;

    /// Perform a health check to verify the provider is operational.
    fn health_check(&self) -> impl std::future::Future<Output = Result<(), ProviderError>> + Send;

    /// Whether this provider supports file attachments.
    ///
    /// Defaults to `false`. Providers that handle attachments (email, Slack,
    /// Discord, webhook) should override this to return `true`.
    fn supports_attachments(&self) -> bool {
        false
    }

    /// Execute the given action with additional dispatch context (e.g. resolved attachments).
    ///
    /// The default implementation ignores the context and delegates to [`execute`](Self::execute).
    /// Providers that support attachments should override this to handle the
    /// resolved blobs from [`DispatchContext::attachments`].
    fn execute_with_context(
        &self,
        action: &Action,
        _ctx: &DispatchContext,
    ) -> impl std::future::Future<Output = Result<ProviderResponse, ProviderError>> + Send {
        self.execute(action)
    }
}

/// Object-safe provider trait for use behind `Arc<dyn DynProvider>`.
///
/// Uses [`macro@async_trait`] to enable dynamic dispatch of async methods.
/// You generally should not implement this trait directly -- instead implement
/// [`Provider`] and rely on the blanket implementation.
#[async_trait]
pub trait DynProvider: Send + Sync {
    /// Returns the unique name of this provider.
    fn name(&self) -> &str;

    /// Execute the given action and return a provider response.
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError>;

    /// Perform a health check to verify the provider is operational.
    async fn health_check(&self) -> Result<(), ProviderError>;

    /// Whether this provider supports file attachments.
    fn supports_attachments(&self) -> bool {
        false
    }

    /// Execute the given action with additional dispatch context.
    async fn execute_with_context(
        &self,
        action: &Action,
        _ctx: &DispatchContext,
    ) -> Result<ProviderResponse, ProviderError> {
        self.execute(action).await
    }
}

/// Blanket implementation: any type that implements [`Provider`] also
/// implements [`DynProvider`], bridging the static and dynamic dispatch worlds.
#[async_trait]
impl<T: Provider + Sync> DynProvider for T {
    fn name(&self) -> &str {
        Provider::name(self)
    }

    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        Provider::execute(self, action).await
    }

    async fn health_check(&self) -> Result<(), ProviderError> {
        Provider::health_check(self).await
    }

    fn supports_attachments(&self) -> bool {
        Provider::supports_attachments(self)
    }

    async fn execute_with_context(
        &self,
        action: &Action,
        ctx: &DispatchContext,
    ) -> Result<ProviderResponse, ProviderError> {
        Provider::execute_with_context(self, action, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use acteon_core::ProviderResponse;

    use super::*;

    /// A mock provider for testing the trait and blanket impl.
    struct MockProvider {
        provider_name: String,
        should_fail: bool,
    }

    impl MockProvider {
        fn new(name: &str, should_fail: bool) -> Self {
            Self {
                provider_name: name.to_owned(),
                should_fail,
            }
        }
    }

    impl Provider for MockProvider {
        fn name(&self) -> &str {
            &self.provider_name
        }

        async fn execute(&self, _action: &Action) -> Result<ProviderResponse, ProviderError> {
            if self.should_fail {
                return Err(ProviderError::ExecutionFailed("mock failure".into()));
            }
            Ok(ProviderResponse::success(serde_json::json!({"mock": true})))
        }

        async fn health_check(&self) -> Result<(), ProviderError> {
            if self.should_fail {
                return Err(ProviderError::Connection("mock unhealthy".into()));
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn provider_execute_success() {
        let provider = MockProvider::new("test", false);
        let action = Action::new("ns", "t", "test", "do_thing", serde_json::Value::Null);
        let resp = Provider::execute(&provider, &action).await.unwrap();
        assert_eq!(resp.status, acteon_core::ResponseStatus::Success);
    }

    #[tokio::test]
    async fn provider_execute_failure() {
        let provider = MockProvider::new("test", true);
        let action = Action::new("ns", "t", "test", "do_thing", serde_json::Value::Null);
        let err = Provider::execute(&provider, &action).await.unwrap_err();
        assert!(matches!(err, ProviderError::ExecutionFailed(_)));
    }

    #[tokio::test]
    async fn blanket_dyn_provider_impl() {
        let provider: Arc<dyn DynProvider> = Arc::new(MockProvider::new("dyn-test", false));
        assert_eq!(provider.name(), "dyn-test");

        let action = Action::new("ns", "t", "dyn-test", "act", serde_json::Value::Null);
        let resp = provider.execute(&action).await.unwrap();
        assert_eq!(resp.status, acteon_core::ResponseStatus::Success);

        provider.health_check().await.unwrap();
    }

    #[tokio::test]
    async fn dyn_provider_health_check_failure() {
        let provider: Arc<dyn DynProvider> = Arc::new(MockProvider::new("sick", true));
        let err = provider.health_check().await.unwrap_err();
        assert!(matches!(err, ProviderError::Connection(_)));
    }
}
