//! WASM plugin runtime for Acteon rule evaluation.
//!
//! This crate provides a `Wasmtime`-based runtime for executing WebAssembly
//! plugins as part of rule condition evaluation. Plugins receive an action
//! context as JSON and return a boolean verdict with optional metadata.

pub mod config;
pub mod error;
pub mod registry;
pub mod runtime;

pub use config::WasmPluginConfig;
pub use error::WasmError;
pub use registry::{SharedWasmRegistry, WasmPluginRegistry};
pub use runtime::{WasmInvocationResult, WasmPluginRuntime};

/// Mock WASM runtime for testing without actual WASM modules.
///
/// Always returns the configured verdict, useful for unit and integration tests.
#[derive(Debug)]
pub struct MockWasmRuntime {
    verdict: bool,
    message: Option<String>,
}

impl MockWasmRuntime {
    /// Create a mock runtime that always returns the given verdict.
    pub fn new(verdict: bool) -> Self {
        Self {
            verdict,
            message: None,
        }
    }

    /// Create a mock runtime with a custom message.
    #[must_use]
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }
}

#[async_trait::async_trait]
impl WasmPluginRuntime for MockWasmRuntime {
    async fn invoke(
        &self,
        _plugin: &str,
        _function: &str,
        _input: &serde_json::Value,
    ) -> Result<WasmInvocationResult, WasmError> {
        Ok(WasmInvocationResult {
            verdict: self.verdict,
            message: self.message.clone(),
            metadata: serde_json::Value::Null,
        })
    }

    fn has_plugin(&self, _name: &str) -> bool {
        true
    }

    fn list_plugins(&self) -> Vec<String> {
        vec!["mock-plugin".to_owned()]
    }
}

/// A failing WASM runtime for testing error handling paths.
#[derive(Debug)]
pub struct FailingWasmRuntime;

#[async_trait::async_trait]
impl WasmPluginRuntime for FailingWasmRuntime {
    async fn invoke(
        &self,
        plugin: &str,
        _function: &str,
        _input: &serde_json::Value,
    ) -> Result<WasmInvocationResult, WasmError> {
        Err(WasmError::Invocation(format!(
            "mock failure for plugin '{plugin}'"
        )))
    }

    fn has_plugin(&self, _name: &str) -> bool {
        true
    }

    fn list_plugins(&self) -> Vec<String> {
        vec!["failing-plugin".to_owned()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_runtime_returns_configured_verdict() {
        let rt = MockWasmRuntime::new(true);
        let result = rt
            .invoke("test", "evaluate", &serde_json::json!({}))
            .await
            .unwrap();
        assert!(result.verdict);
    }

    #[tokio::test]
    async fn mock_runtime_false_verdict() {
        let rt = MockWasmRuntime::new(false);
        let result = rt
            .invoke("test", "evaluate", &serde_json::json!({}))
            .await
            .unwrap();
        assert!(!result.verdict);
    }

    #[tokio::test]
    async fn mock_runtime_with_message() {
        let rt = MockWasmRuntime::new(true).with_message("custom msg");
        let result = rt
            .invoke("test", "evaluate", &serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(result.message.as_deref(), Some("custom msg"));
    }

    #[tokio::test]
    async fn failing_runtime_returns_error() {
        let rt = FailingWasmRuntime;
        let result = rt
            .invoke("my-plugin", "evaluate", &serde_json::json!({}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("my-plugin"));
    }

    #[test]
    fn mock_runtime_has_plugin() {
        let rt = MockWasmRuntime::new(true);
        assert!(rt.has_plugin("anything"));
    }

    #[test]
    fn mock_runtime_list_plugins() {
        let rt = MockWasmRuntime::new(true);
        assert_eq!(rt.list_plugins(), vec!["mock-plugin"]);
    }

    #[tokio::test]
    async fn mock_runtime_null_metadata() {
        let rt = MockWasmRuntime::new(true);
        let result = rt
            .invoke("any-plugin", "evaluate", &serde_json::json!({"test": 1}))
            .await
            .unwrap();
        assert!(result.metadata.is_null());
    }

    #[tokio::test]
    async fn failing_runtime_has_plugin_returns_true() {
        // FailingWasmRuntime always claims to have plugins (errors on invoke).
        let rt = FailingWasmRuntime;
        assert!(rt.has_plugin("anything"));
    }

    #[test]
    fn failing_runtime_list_plugins() {
        let rt = FailingWasmRuntime;
        assert_eq!(rt.list_plugins(), vec!["failing-plugin"]);
    }

    #[tokio::test]
    async fn failing_runtime_error_contains_plugin_name() {
        let rt = FailingWasmRuntime;
        let err = rt
            .invoke("custom-name", "evaluate", &serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("custom-name"));
    }

    #[tokio::test]
    async fn mock_runtime_ignores_input() {
        // Mock runtime returns the same verdict regardless of input.
        let rt = MockWasmRuntime::new(true).with_message("always true");
        let empty = rt.invoke("p", "f", &serde_json::json!({})).await.unwrap();
        let complex = rt
            .invoke(
                "p",
                "f",
                &serde_json::json!({"nested": {"deep": [1, 2, 3]}}),
            )
            .await
            .unwrap();
        assert_eq!(empty.verdict, complex.verdict);
        assert_eq!(empty.message, complex.message);
    }
}
