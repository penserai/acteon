use async_trait::async_trait;

use crate::ProviderError;

/// Trait for providers that can look up external resource state.
///
/// Used by pre-dispatch enrichment to fetch live data (e.g., current `AutoScaling` group
/// state) before rule evaluation.
#[async_trait]
pub trait ResourceLookup: Send + Sync {
    /// Look up a resource by type and parameters.
    ///
    /// Returns a JSON value containing the resource state, which will be merged
    /// into the action payload under the configured merge key.
    async fn lookup(
        &self,
        resource_type: &str,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError>;

    /// Returns the list of resource types this provider supports.
    fn supported_resource_types(&self) -> Vec<String>;
}
