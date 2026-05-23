use serde::{Deserialize, Serialize};

use crate::error::WasmError;

/// The result of invoking a WASM plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmInvocationResult {
    /// Whether the plugin evaluation returned true (pass) or false (fail).
    pub verdict: bool,
    /// Optional message from the plugin (e.g., explanation of the verdict).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Optional structured metadata from the plugin.
    #[serde(default, skip_serializing_if = "is_null")]
    pub metadata: serde_json::Value,
}

fn is_null(v: &serde_json::Value) -> bool {
    v.is_null()
}

impl WasmInvocationResult {
    /// Create a simple verdict result with no message or metadata.
    pub fn from_verdict(verdict: bool) -> Self {
        Self {
            verdict,
            message: None,
            metadata: serde_json::Value::Null,
        }
    }

    /// Create a verdict with a message explaining the decision.
    pub fn with_message(verdict: bool, message: impl Into<String>) -> Self {
        Self {
            verdict,
            message: Some(message.into()),
            metadata: serde_json::Value::Null,
        }
    }
}

/// Trait for WASM plugin runtimes.
///
/// Implementations provide the ability to invoke WASM plugins with a JSON input
/// and receive a verdict. The trait is `Send + Sync` so it can be shared across
/// async tasks.
///
/// Plugins are **pure functions**: they receive action context as JSON and return
/// a verdict. They have no access to host state, filesystem, or network.
/// This keeps the security surface minimal and plugins easy to test.
#[async_trait::async_trait]
pub trait WasmPluginRuntime: Send + Sync + std::fmt::Debug {
    /// Invoke a plugin function with the given JSON input.
    ///
    /// # Arguments
    /// * `plugin` - Name of the registered plugin
    /// * `function` - Name of the exported function to call (typically `"evaluate"`)
    /// * `input` - JSON payload to pass to the plugin
    async fn invoke(
        &self,
        plugin: &str,
        function: &str,
        input: &serde_json::Value,
    ) -> Result<WasmInvocationResult, WasmError>;

    /// Check if a plugin is registered.
    fn has_plugin(&self, name: &str) -> bool;

    /// List all registered plugin names.
    fn list_plugins(&self) -> Vec<String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocation_result_serde_roundtrip() {
        let result = WasmInvocationResult {
            verdict: true,
            message: Some("all good".into()),
            metadata: serde_json::json!({"score": 0.95}),
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: WasmInvocationResult = serde_json::from_str(&json).unwrap();
        assert!(back.verdict);
        assert_eq!(back.message.as_deref(), Some("all good"));
    }

    #[test]
    fn invocation_result_skip_null_metadata() {
        let result = WasmInvocationResult {
            verdict: false,
            message: None,
            metadata: serde_json::Value::Null,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(!json.contains("metadata"));
        assert!(!json.contains("message"));
    }

    #[test]
    fn from_verdict_helper() {
        let result = WasmInvocationResult::from_verdict(true);
        assert!(result.verdict);
        assert!(result.message.is_none());
        assert!(result.metadata.is_null());

        let result = WasmInvocationResult::from_verdict(false);
        assert!(!result.verdict);
    }

    #[test]
    fn with_message_helper() {
        let result = WasmInvocationResult::with_message(false, "blocked by policy");
        assert!(!result.verdict);
        assert_eq!(result.message.as_deref(), Some("blocked by policy"));
        assert!(result.metadata.is_null());
    }

    #[test]
    fn invocation_result_deserialize_minimal() {
        // Only verdict is required.
        let json = r#"{"verdict": true}"#;
        let result: WasmInvocationResult = serde_json::from_str(json).unwrap();
        assert!(result.verdict);
        assert!(result.message.is_none());
        assert!(result.metadata.is_null());
    }

    #[test]
    fn invocation_result_deserialize_with_metadata() {
        let json = r#"{"verdict": false, "message": "rate limited", "metadata": {"count": 42}}"#;
        let result: WasmInvocationResult = serde_json::from_str(json).unwrap();
        assert!(!result.verdict);
        assert_eq!(result.message.as_deref(), Some("rate limited"));
        assert_eq!(result.metadata["count"], 42);
    }

    #[test]
    fn invocation_result_clone() {
        let original = WasmInvocationResult::with_message(true, "original");
        let cloned = original.clone();
        assert_eq!(original.verdict, cloned.verdict);
        assert_eq!(original.message, cloned.message);
    }

    #[test]
    fn invocation_result_debug_format() {
        let result = WasmInvocationResult::from_verdict(true);
        let debug = format!("{result:?}");
        assert!(debug.contains("verdict: true"));
    }
}
