/// Errors that can occur during WASM plugin operations.
#[derive(Debug, thiserror::Error)]
pub enum WasmError {
    /// Plugin not found in the registry.
    #[error(
        "WASM plugin '{0}' not found. Registered plugins: use list_plugins() to see available plugins."
    )]
    PluginNotFound(String),

    /// Plugin exists but is disabled.
    #[error("WASM plugin '{0}' is disabled. Enable it via the API or config before invoking.")]
    PluginDisabled(String),

    /// Invalid plugin configuration.
    #[error("invalid WASM plugin config: {0}")]
    InvalidConfig(String),

    /// Error compiling the WASM module.
    ///
    /// This usually means the `.wasm` file is corrupted or was built for
    /// an incompatible target. Ensure the module targets `wasm32-unknown-unknown`
    /// or `wasm32-wasi`.
    #[error("WASM compilation error for plugin: {0}")]
    Compilation(String),

    /// Error during plugin invocation.
    #[error("WASM invocation error: {0}")]
    Invocation(String),

    /// Plugin exceeded its configured timeout.
    ///
    /// The plugin took longer than its `timeout_ms` to return a result.
    /// Consider optimizing the plugin logic or increasing the timeout.
    #[error(
        "WASM plugin timed out after {0}ms. Consider increasing timeout_ms or optimizing plugin logic."
    )]
    Timeout(u64),

    /// Plugin exceeded its configured memory limit.
    #[error(
        "WASM plugin exceeded memory limit of {0} bytes. Consider increasing memory_limit_bytes or reducing allocations."
    )]
    MemoryExceeded(u64),

    /// Error deserializing plugin output.
    ///
    /// The plugin's exported function must return valid JSON matching
    /// the `WasmInvocationResult` schema: `{{ "verdict": bool, "message": string|null }}`.
    #[error(
        "invalid WASM plugin output: {0}. Expected JSON: {{\"verdict\": bool, \"message\": string|null}}"
    )]
    InvalidOutput(String),

    /// I/O error loading a plugin file.
    #[error("I/O error loading WASM plugin: {0}")]
    Io(#[from] std::io::Error),

    /// The plugin registry is full (maximum tracked plugins reached).
    #[error(
        "WASM plugin registry full (max {0} plugins). Remove unused plugins before adding new ones."
    )]
    RegistryFull(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_not_found_message() {
        let err = WasmError::PluginNotFound("my-plugin".into());
        let msg = err.to_string();
        assert!(msg.contains("my-plugin"));
        assert!(msg.contains("not found"));
    }

    #[test]
    fn plugin_disabled_message() {
        let err = WasmError::PluginDisabled("disabled-one".into());
        let msg = err.to_string();
        assert!(msg.contains("disabled-one"));
        assert!(msg.contains("disabled"));
    }

    #[test]
    fn invalid_config_message() {
        let err = WasmError::InvalidConfig("name must not be empty".into());
        let msg = err.to_string();
        assert!(msg.contains("name must not be empty"));
    }

    #[test]
    fn compilation_error_message() {
        let err = WasmError::Compilation("bad bytecode".into());
        let msg = err.to_string();
        assert!(msg.contains("bad bytecode"));
        assert!(msg.contains("compilation"));
    }

    #[test]
    fn invocation_error_message() {
        let err = WasmError::Invocation("trapped".into());
        let msg = err.to_string();
        assert!(msg.contains("trapped"));
    }

    #[test]
    fn timeout_message_contains_ms() {
        let err = WasmError::Timeout(500);
        let msg = err.to_string();
        assert!(msg.contains("500ms"));
    }

    #[test]
    fn memory_exceeded_message_contains_bytes() {
        let err = WasmError::MemoryExceeded(16_777_216);
        let msg = err.to_string();
        assert!(msg.contains("16777216"));
    }

    #[test]
    fn invalid_output_message() {
        let err = WasmError::InvalidOutput("not JSON".into());
        let msg = err.to_string();
        assert!(msg.contains("not JSON"));
        assert!(msg.contains("verdict"));
    }

    #[test]
    fn io_error_from_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let wasm_err: WasmError = io_err.into();
        assert!(matches!(wasm_err, WasmError::Io(_)));
        assert!(wasm_err.to_string().contains("file missing"));
    }

    #[test]
    fn registry_full_message() {
        let err = WasmError::RegistryFull(256);
        let msg = err.to_string();
        assert!(msg.contains("256"));
        assert!(msg.contains("full"));
    }

    #[test]
    fn all_variants_are_debug() {
        // Ensure all variants implement Debug without panicking.
        let errors: Vec<WasmError> = vec![
            WasmError::PluginNotFound("p".into()),
            WasmError::PluginDisabled("p".into()),
            WasmError::InvalidConfig("c".into()),
            WasmError::Compilation("c".into()),
            WasmError::Invocation("i".into()),
            WasmError::Timeout(100),
            WasmError::MemoryExceeded(1024),
            WasmError::InvalidOutput("o".into()),
            WasmError::RegistryFull(10),
        ];
        for err in &errors {
            let _ = format!("{err:?}");
            let _ = format!("{err}");
        }
    }
}
