use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use tracing::{debug, info, warn};
use wasmtime::{Engine, Module};

use crate::config::{MAX_TRACKED_PLUGINS, WasmPluginConfig, WasmRuntimeConfig};
use crate::error::WasmError;
use crate::runtime::{WasmInvocationResult, WasmPluginRuntime};

/// Maximum size of serialized JSON input passed to a plugin (1 MB).
///
/// Prevents callers from passing unbounded payloads that would be copied
/// into WASM linear memory.
const MAX_INPUT_JSON_BYTES: usize = 1_024 * 1_024;

/// Maximum size of JSON output a plugin can return (1 MB).
///
/// Prevents a malicious plugin from claiming an enormous `result_len`
/// that would cause host-side allocation pressure.
const MAX_OUTPUT_JSON_BYTES: usize = 1_024 * 1_024;

/// Maximum number of table elements a plugin can allocate.
///
/// Tables are used for indirect function calls (`call_indirect`). A
/// malicious module could try to grow tables to exhaust host memory.
const MAX_TABLE_ELEMENTS: usize = 10_000;

/// A compiled WASM module with its configuration.
struct LoadedPlugin {
    config: WasmPluginConfig,
    module: Module,
}

impl std::fmt::Debug for LoadedPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedPlugin")
            .field("config", &self.config)
            .field("module", &"<wasmtime::Module>")
            .finish()
    }
}

/// Registry that manages compiled WASM plugin modules.
///
/// Plugins are compiled once and cached. Invocation creates a new `Wasmtime`
/// store per call for isolation.
///
/// # Security model
///
/// - **No WASI**: plugins have no access to the filesystem, network, or
///   environment variables. The `Wasmtime` engine is configured without
///   any WASI context, so imports like `fd_read` or `sock_connect` will
///   fail at registration time (host import validation).
/// - **No host imports**: modules that import any host functions are
///   rejected at registration time.
/// - **Fuel metering**: each invocation is given a finite fuel budget
///   proportional to the configured `timeout_ms`. When fuel runs out the
///   call traps with a `Timeout` error.
/// - **Memory limits**: a per-store `ResourceLimiter` caps both linear
///   memory growth and table growth.
/// - **Input/output size limits**: serialized JSON is bounded at both
///   ingress and egress to prevent OOM.
/// - **Per-invocation isolation**: every call creates a fresh `Store` and
///   `Instance`, so plugins cannot observe or affect each other's state.
/// - **No threads**: `wasm_threads` is disabled to prevent shared-memory
///   threads that could escape fuel metering.
pub struct WasmPluginRegistry {
    engine: Engine,
    plugins: parking_lot::RwLock<HashMap<String, LoadedPlugin>>,
    runtime_config: WasmRuntimeConfig,
}

impl std::fmt::Debug for WasmPluginRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmPluginRegistry")
            .field("runtime_config", &self.runtime_config)
            .field("plugin_count", &self.plugins.read().len())
            .finish_non_exhaustive()
    }
}

impl WasmPluginRegistry {
    /// Create a new empty registry with a hardened engine configuration.
    ///
    /// The engine is configured with:
    /// - Fuel consumption enabled (for CPU bounding)
    /// - No WASI capabilities (no FS, network, env, clock, random)
    /// - No multi-threading (prevents shared-memory threads)
    pub fn new(runtime_config: WasmRuntimeConfig) -> Result<Self, WasmError> {
        runtime_config.validate()?;

        let mut wasmtime_config = wasmtime::Config::new();

        // Enable fuel-based instruction metering for CPU bounding.
        wasmtime_config.consume_fuel(true);

        // Explicitly disable wasm-threads to prevent plugins from
        // spawning shared-memory threads that could escape metering.
        wasmtime_config.wasm_threads(false);

        let engine = Engine::new(&wasmtime_config)
            .map_err(|e| WasmError::Compilation(format!("failed to create WASM engine: {e}")))?;

        Ok(Self {
            engine,
            plugins: parking_lot::RwLock::new(HashMap::new()),
            runtime_config,
        })
    }

    /// Register a plugin from raw WASM bytes.
    ///
    /// The config is validated before registration. The WASM module is
    /// also validated to ensure it does not import any host functions.
    pub fn register_bytes(
        &self,
        config: WasmPluginConfig,
        wasm_bytes: &[u8],
    ) -> Result<(), WasmError> {
        // Validate config before doing expensive compilation.
        config.validate()?;

        let plugins = self.plugins.read();
        if plugins.len() >= MAX_TRACKED_PLUGINS && !plugins.contains_key(&config.name) {
            return Err(WasmError::RegistryFull(MAX_TRACKED_PLUGINS));
        }
        drop(plugins);

        if wasm_bytes.is_empty() {
            return Err(WasmError::Compilation(format!(
                "plugin '{}': WASM bytes are empty. Provide a valid .wasm file.",
                config.name
            )));
        }

        // Wasmtime's Module::new validates the WASM binary structure.
        // Malformed or malicious modules will be rejected here.
        let module = Module::new(&self.engine, wasm_bytes).map_err(|e| {
            WasmError::Compilation(format!(
                "failed to compile plugin '{}': {e}. \
                 Ensure the module targets wasm32-unknown-unknown or wasm32-wasi.",
                config.name
            ))
        })?;

        // Reject modules that import host functions. WASM plugins must be
        // self-contained pure computation. Imports indicate the module
        // expects WASI or custom host functions, which we do not provide.
        for import in module.imports() {
            if import.ty().func().is_some() {
                return Err(WasmError::Compilation(format!(
                    "plugin '{}' imports host function '{}::{}'. \
                     Acteon WASM plugins must be self-contained with no host imports.",
                    config.name,
                    import.module(),
                    import.name(),
                )));
            }
        }

        info!(plugin = %config.name, "registered WASM plugin");
        self.plugins
            .write()
            .insert(config.name.clone(), LoadedPlugin { config, module });
        Ok(())
    }

    /// Register a plugin from a `.wasm` file on disk.
    pub fn register_file(&self, config: WasmPluginConfig, path: &Path) -> Result<(), WasmError> {
        let wasm_bytes = std::fs::read(path)?;
        self.register_bytes(config, &wasm_bytes)
    }

    /// Remove a plugin from the registry.
    pub fn unregister(&self, name: &str) -> bool {
        let removed = self.plugins.write().remove(name).is_some();
        if removed {
            info!(plugin = %name, "unregistered WASM plugin");
        }
        removed
    }

    /// Get the configuration of a registered plugin.
    pub fn get_config(&self, name: &str) -> Option<WasmPluginConfig> {
        self.plugins.read().get(name).map(|p| p.config.clone())
    }

    /// Get the runtime configuration.
    pub fn runtime_config(&self) -> &WasmRuntimeConfig {
        &self.runtime_config
    }

    /// Return the number of registered plugins.
    pub fn plugin_count(&self) -> usize {
        self.plugins.read().len()
    }

    /// Load all `.wasm` files from the configured plugin directory.
    ///
    /// Plugin names are derived from file stems. Names that fail validation
    /// (e.g. contain path traversal characters) are skipped with a warning.
    pub fn load_plugin_dir(&self) -> Result<usize, WasmError> {
        let Some(ref dir) = self.runtime_config.plugin_dir else {
            return Ok(0);
        };

        let path = Path::new(dir);
        if !path.is_dir() {
            warn!(path = %dir, "WASM plugin directory does not exist");
            return Ok(0);
        }

        let mut loaded = 0;
        let entries = std::fs::read_dir(path)?;
        for entry in entries {
            let entry = entry?;
            let file_path = entry.path();
            if file_path.extension().and_then(|e| e.to_str()) == Some("wasm") {
                let name = file_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_owned();

                let config = WasmPluginConfig::new(&name)
                    .with_memory_limit(self.runtime_config.default_memory_limit_bytes)
                    .with_timeout_ms(self.runtime_config.default_timeout_ms)
                    .with_wasm_path(file_path.display().to_string());

                // register_bytes calls validate() internally, so invalid
                // names (e.g. path traversal) are caught there.
                match self.register_file(config, &file_path) {
                    Ok(()) => {
                        loaded += 1;
                        debug!(plugin = %name, path = %file_path.display(), "loaded WASM plugin from directory");
                    }
                    Err(e) => {
                        warn!(plugin = %name, error = %e, "failed to load WASM plugin");
                    }
                }
            }
        }

        info!(count = loaded, dir = %dir, "loaded WASM plugins from directory");
        Ok(loaded)
    }

    /// Invoke a plugin function.
    ///
    /// Creates a new `Wasmtime` store per invocation for isolation. The plugin's
    /// exported function is called with the JSON-serialized input as a string
    /// parameter.
    ///
    /// # Security bounds
    ///
    /// - Input JSON capped at [`MAX_INPUT_JSON_BYTES`] (1 MB)
    /// - Output JSON capped at [`MAX_OUTPUT_JSON_BYTES`] (1 MB)
    /// - Fuel proportional to `timeout_ms`
    /// - Memory growth bounded by `memory_limit_bytes`
    /// - Table growth bounded by [`MAX_TABLE_ELEMENTS`]
    #[allow(clippy::too_many_lines)]
    fn invoke_internal(
        &self,
        plugin_name: &str,
        function_name: &str,
        input: &serde_json::Value,
    ) -> Result<WasmInvocationResult, WasmError> {
        // Clone the module and config under the read lock, then drop it
        // immediately. This avoids holding the RwLock for the entire WASM
        // execution, which would block concurrent register/unregister ops.
        let (module, config) = {
            let plugins = self.plugins.read();
            let loaded = plugins
                .get(plugin_name)
                .ok_or_else(|| WasmError::PluginNotFound(plugin_name.to_owned()))?;

            if !loaded.config.enabled {
                return Err(WasmError::PluginDisabled(plugin_name.to_owned()));
            }

            // Module::clone is cheap (Arc internally).
            (loaded.module.clone(), loaded.config.clone())
        };

        // Calculate fuel from timeout: ~1M instructions per ms as a rough heuristic.
        let fuel = config.timeout_ms.saturating_mul(1_000_000);

        // Store the MemoryLimiter as the store's data so that the
        // `limiter()` callback can return a &mut reference to it.
        let memory_limit = config.memory_limit_bytes;
        let max_memory_bytes = usize::try_from(memory_limit).unwrap_or(usize::MAX);
        let store_data = MemoryLimiter {
            max_memory_bytes,
            max_table_elements: MAX_TABLE_ELEMENTS,
        };
        let mut store = wasmtime::Store::new(&self.engine, store_data);
        store
            .set_fuel(fuel)
            .map_err(|e| WasmError::Invocation(format!("failed to set fuel: {e}")))?;

        // Point the store's limiter at the data we placed inside.
        store.limiter(|data| data as &mut dyn wasmtime::ResourceLimiter);

        let instance = wasmtime::Instance::new(&mut store, &module, &[]).map_err(|e| {
            WasmError::Invocation(format!("failed to instantiate plugin '{plugin_name}': {e}"))
        })?;

        // Get the plugin's memory export for writing input.
        let memory = instance.get_memory(&mut store, "memory").ok_or_else(|| {
            WasmError::Invocation(format!("plugin '{plugin_name}' does not export 'memory'"))
        })?;

        // Serialize input to JSON string with size check.
        let input_json = serde_json::to_string(input)
            .map_err(|e| WasmError::Invocation(format!("failed to serialize input: {e}")))?;
        if input_json.len() > MAX_INPUT_JSON_BYTES {
            return Err(WasmError::Invocation(format!(
                "serialized input ({} bytes) exceeds maximum of {MAX_INPUT_JSON_BYTES} bytes",
                input_json.len()
            )));
        }
        let input_bytes = input_json.as_bytes();

        // Try to get the `alloc` function for writing input.
        // If the plugin exports `alloc`, we use it to allocate memory for the input.
        // Otherwise, we write the input at a fixed offset.
        let (input_ptr, input_len) = if let Ok(alloc_fn) =
            instance.get_typed_func::<i32, i32>(&mut store, "alloc")
        {
            let len = i32::try_from(input_bytes.len())
                .map_err(|_| WasmError::Invocation("input too large for i32 addressing".into()))?;
            let ptr = alloc_fn.call(&mut store, len).map_err(|e| {
                if store.get_fuel().ok() == Some(0) {
                    WasmError::Timeout(config.timeout_ms)
                } else {
                    WasmError::Invocation(format!("alloc failed: {e}"))
                }
            })?;
            // Reject negative pointers (indicates alloc failure in guest).
            if ptr < 0 {
                return Err(WasmError::Invocation(
                    "alloc returned negative pointer".into(),
                ));
            }
            // Safe: we checked ptr >= 0 above.
            #[allow(clippy::cast_sign_loss)]
            let ptr_usize = ptr as usize;
            // Use checked arithmetic to prevent overflow on ptr + len.
            let end = ptr_usize
                .checked_add(input_bytes.len())
                .ok_or(WasmError::MemoryExceeded(memory_limit))?;
            memory
                .data_mut(&mut store)
                .get_mut(ptr_usize..end)
                .ok_or(WasmError::MemoryExceeded(memory_limit))?
                .copy_from_slice(input_bytes);
            (ptr, len)
        } else {
            // Fallback: write at offset 0 (simple plugins).
            let data = memory.data_mut(&mut store);
            if input_bytes.len() > data.len() {
                return Err(WasmError::MemoryExceeded(memory_limit));
            }
            data[..input_bytes.len()].copy_from_slice(input_bytes);
            (
                0i32,
                i32::try_from(input_bytes.len()).map_err(|_| {
                    WasmError::Invocation("input too large for i32 addressing".into())
                })?,
            )
        };

        // Call the plugin function.
        // Expected signature: fn(input_ptr: i32, input_len: i32) -> i32
        // Return value: 0 = false, non-zero = true (simple) or pointer to result JSON.
        let func = instance
            .get_typed_func::<(i32, i32), i32>(&mut store, function_name)
            .map_err(|e| {
                WasmError::Invocation(format!(
                    "function '{function_name}' not found in plugin '{plugin_name}': {e}"
                ))
            })?;

        let result_code = func.call(&mut store, (input_ptr, input_len)).map_err(|e| {
            if store.get_fuel().ok() == Some(0) {
                WasmError::Timeout(config.timeout_ms)
            } else {
                WasmError::Invocation(format!(
                    "plugin '{plugin_name}' function '{function_name}' trapped: {e}"
                ))
            }
        })?;

        // Try to read a result JSON from the plugin's memory.
        // Convention: if the plugin exports `result_ptr` and `result_len` globals,
        // we read the result JSON from those offsets. Otherwise, treat the return
        // code as a simple boolean (0 = false, non-zero = true).
        if let (Some(result_ptr_global), Some(result_len_global)) = (
            instance.get_global(&mut store, "result_ptr"),
            instance.get_global(&mut store, "result_len"),
        ) {
            let rptr = result_ptr_global.get(&mut store).i32().unwrap_or(0);
            let rlen = result_len_global.get(&mut store).i32().unwrap_or(0);

            // Validate pointer and length are non-negative.
            if rptr >= 0 && rlen > 0 {
                // Safe: we checked rptr >= 0 and rlen > 0 above.
                #[allow(clippy::cast_sign_loss)]
                let rptr = rptr as usize;
                #[allow(clippy::cast_sign_loss)]
                let rlen = rlen as usize;

                // Bound output size to prevent host-side OOM from a
                // malicious plugin claiming a huge result_len.
                if rlen > MAX_OUTPUT_JSON_BYTES {
                    return Err(WasmError::InvalidOutput(format!(
                        "plugin output ({rlen} bytes) exceeds maximum of {MAX_OUTPUT_JSON_BYTES} bytes"
                    )));
                }

                // Use checked arithmetic to prevent overflow on
                // rptr + rlen wrapping to a small value.
                let data = memory.data(&store);
                if let Some(end) = rptr.checked_add(rlen)
                    && end <= data.len()
                {
                    let result_bytes = &data[rptr..end];
                    if let Ok(result_str) = std::str::from_utf8(result_bytes)
                        && let Ok(mut result) =
                            serde_json::from_str::<WasmInvocationResult>(result_str)
                    {
                        // Truncate unreasonably long messages.
                        if let Some(ref msg) = result.message
                            && msg.len() > MAX_OUTPUT_JSON_BYTES
                        {
                            result.message = Some(format!("{}... (truncated)", &msg[..1024]));
                        }
                        return Ok(result);
                    }
                }
            }
        }

        // Fallback: simple boolean from return code.
        Ok(WasmInvocationResult {
            verdict: result_code != 0,
            message: None,
            metadata: serde_json::Value::Null,
        })
    }

    /// Check if a plugin is registered.
    pub fn has_plugin(&self, name: &str) -> bool {
        self.plugins.read().contains_key(name)
    }

    /// List all registered plugin names.
    pub fn list_plugins(&self) -> Vec<String> {
        self.plugins.read().keys().cloned().collect()
    }
}

/// Resource limiter for `Wasmtime` stores that enforces both linear memory
/// and table growth bounds.
struct MemoryLimiter {
    max_memory_bytes: usize,
    max_table_elements: usize,
}

impl wasmtime::ResourceLimiter for MemoryLimiter {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(desired <= self.max_memory_bytes)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(desired <= self.max_table_elements)
    }
}

/// An `Arc`-wrapped registry that implements [`WasmPluginRuntime`].
///
/// This wrapper exists so that `invoke()` can safely pass a clone into
/// `tokio::task::spawn_blocking` without raw pointer tricks. The `Arc`
/// ensures the registry stays alive for the duration of the blocking task.
#[derive(Debug, Clone)]
pub struct SharedWasmRegistry {
    inner: Arc<WasmPluginRegistry>,
}

impl SharedWasmRegistry {
    /// Wrap a registry in `Arc` for thread-safe async invocation.
    pub fn new(registry: WasmPluginRegistry) -> Self {
        Self {
            inner: Arc::new(registry),
        }
    }

    /// Access the underlying registry (e.g. for registration).
    pub fn registry(&self) -> &WasmPluginRegistry {
        &self.inner
    }
}

#[async_trait::async_trait]
impl WasmPluginRuntime for SharedWasmRegistry {
    async fn invoke(
        &self,
        plugin: &str,
        function: &str,
        input: &serde_json::Value,
    ) -> Result<WasmInvocationResult, WasmError> {
        // Clone the Arc and owned strings so the closure is 'static.
        let registry = Arc::clone(&self.inner);
        let plugin = plugin.to_owned();
        let function = function.to_owned();
        let input = input.clone();

        // Run the synchronous WASM invocation on a blocking thread to
        // avoid blocking the async runtime. The Arc ensures the registry
        // stays alive for the duration of the blocking task.
        tokio::task::spawn_blocking(move || registry.invoke_internal(&plugin, &function, &input))
            .await
            .map_err(|e| WasmError::Invocation(format!("task join error: {e}")))?
    }

    fn has_plugin(&self, name: &str) -> bool {
        self.inner.has_plugin(name)
    }

    fn list_plugins(&self) -> Vec<String> {
        self.inner.list_plugins()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> WasmRuntimeConfig {
        WasmRuntimeConfig::default()
    }

    fn minimal_wasm_true() -> Vec<u8> {
        let wat = r#"(module
            (memory (export "memory") 1)
            (func (export "evaluate") (param i32 i32) (result i32)
                i32.const 1
            )
        )"#;
        wat::parse_str(wat).unwrap()
    }

    fn minimal_wasm_false() -> Vec<u8> {
        let wat = r#"(module
            (memory (export "memory") 1)
            (func (export "evaluate") (param i32 i32) (result i32)
                i32.const 0
            )
        )"#;
        wat::parse_str(wat).unwrap()
    }

    #[test]
    fn create_registry() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        assert!(registry.list_plugins().is_empty());
        assert_eq!(registry.plugin_count(), 0);
    }

    #[test]
    fn register_invalid_wasm_fails() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        let config = WasmPluginConfig::new("bad-plugin");
        let result = registry.register_bytes(config, b"not a wasm module");
        assert!(result.is_err());
    }

    #[test]
    fn register_empty_bytes_fails() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        let config = WasmPluginConfig::new("empty-plugin");
        let result = registry.register_bytes(config, b"");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn has_plugin_false_for_unknown() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        assert!(!registry.has_plugin("nonexistent"));
    }

    #[test]
    fn unregister_nonexistent_returns_false() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        assert!(!registry.unregister("nonexistent"));
    }

    #[test]
    fn load_nonexistent_dir_returns_zero() {
        let config = WasmRuntimeConfig {
            plugin_dir: Some("/nonexistent/wasm/dir".into()),
            ..WasmRuntimeConfig::default()
        };
        let registry = WasmPluginRegistry::new(config).unwrap();
        assert_eq!(registry.load_plugin_dir().unwrap(), 0);
    }

    #[test]
    fn get_config_returns_none_for_unknown() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        assert!(registry.get_config("unknown").is_none());
    }

    #[test]
    fn register_and_invoke_minimal_wasm() {
        let wasm_bytes = minimal_wasm_true();
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        let config = WasmPluginConfig::new("minimal");
        registry.register_bytes(config, &wasm_bytes).unwrap();

        assert!(registry.has_plugin("minimal"));

        let result = registry
            .invoke_internal("minimal", "evaluate", &serde_json::json!({}))
            .unwrap();
        assert!(result.verdict);
    }

    #[test]
    fn register_wasm_returning_false() {
        let wasm_bytes = minimal_wasm_false();
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        let config = WasmPluginConfig::new("false-plugin");
        registry.register_bytes(config, &wasm_bytes).unwrap();

        let result = registry
            .invoke_internal("false-plugin", "evaluate", &serde_json::json!({}))
            .unwrap();
        assert!(!result.verdict);
    }

    #[test]
    fn invoke_nonexistent_plugin_errors() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        let result = registry.invoke_internal("missing", "evaluate", &serde_json::json!({}));
        assert!(matches!(result, Err(WasmError::PluginNotFound(_))));
    }

    #[test]
    fn invoke_disabled_plugin_errors() {
        let wasm_bytes = minimal_wasm_true();
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        let config = WasmPluginConfig::new("disabled-plugin").with_enabled(false);
        registry.register_bytes(config, &wasm_bytes).unwrap();

        let result =
            registry.invoke_internal("disabled-plugin", "evaluate", &serde_json::json!({}));
        assert!(matches!(result, Err(WasmError::PluginDisabled(_))));
    }

    #[test]
    fn unregister_plugin() {
        let wasm_bytes = minimal_wasm_true();
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        let config = WasmPluginConfig::new("removable");
        registry.register_bytes(config, &wasm_bytes).unwrap();
        assert!(registry.has_plugin("removable"));

        assert!(registry.unregister("removable"));
        assert!(!registry.has_plugin("removable"));
    }

    // --- Security-focused tests ---

    #[test]
    fn register_validates_config_empty_name() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        let config = WasmPluginConfig::new("");
        let result = registry.register_bytes(config, &minimal_wasm_true());
        assert!(matches!(result, Err(WasmError::InvalidConfig(_))));
    }

    #[test]
    fn register_rejects_path_traversal_name() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        // Names with path separators should be rejected by validate().
        let config = WasmPluginConfig::new("../../../etc/passwd");
        let result = registry.register_bytes(config, &minimal_wasm_true());
        assert!(matches!(result, Err(WasmError::InvalidConfig(_))));
    }

    #[test]
    fn register_rejects_module_with_wasi_imports() {
        // A module importing a WASI function must be rejected.
        let wat = r#"(module
            (import "wasi_snapshot_preview1" "fd_write"
                (func $fd_write (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "evaluate") (param i32 i32) (result i32)
                i32.const 1
            )
        )"#;
        let wasm_bytes = wat::parse_str(wat).unwrap();

        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        let config = WasmPluginConfig::new("wasi-sneaker");
        let result = registry.register_bytes(config, &wasm_bytes);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("imports host function"),
            "expected import rejection, got: {err}"
        );
    }

    #[test]
    fn register_rejects_module_with_custom_host_imports() {
        // A module importing arbitrary host functions must be rejected.
        let wat = r#"(module
            (import "env" "steal_data"
                (func $steal (param i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "evaluate") (param i32 i32) (result i32)
                i32.const 1
            )
        )"#;
        let wasm_bytes = wat::parse_str(wat).unwrap();

        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        let config = WasmPluginConfig::new("custom-import");
        let result = registry.register_bytes(config, &wasm_bytes);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("imports host function"),
            "expected import rejection, got: {err}"
        );
    }

    #[test]
    fn input_size_limit_enforced() {
        let wasm_bytes = minimal_wasm_true();
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        let config = WasmPluginConfig::new("input-limit-test");
        registry.register_bytes(config, &wasm_bytes).unwrap();

        // Create input larger than MAX_INPUT_JSON_BYTES.
        let large_string = "x".repeat(MAX_INPUT_JSON_BYTES + 1);
        let large_input = serde_json::json!({"data": large_string});

        let result = registry.invoke_internal("input-limit-test", "evaluate", &large_input);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("exceeds maximum"),
            "expected input size error, got: {err}"
        );
    }

    #[test]
    fn memory_limiter_rejects_excessive_growth() {
        let mut limiter = MemoryLimiter {
            max_memory_bytes: 1024,
            max_table_elements: 100,
        };

        // Within limit: ok
        assert!(wasmtime::ResourceLimiter::memory_growing(&mut limiter, 0, 512, None).unwrap());
        // Exceeds limit: rejected
        assert!(!wasmtime::ResourceLimiter::memory_growing(&mut limiter, 0, 2048, None).unwrap());
    }

    #[test]
    fn table_limiter_rejects_excessive_growth() {
        let mut limiter = MemoryLimiter {
            max_memory_bytes: 1024,
            max_table_elements: 100,
        };

        // Within limit: ok
        assert!(wasmtime::ResourceLimiter::table_growing(&mut limiter, 0, 50, None).unwrap());
        // Exceeds limit: rejected
        assert!(!wasmtime::ResourceLimiter::table_growing(&mut limiter, 0, 200, None).unwrap());
    }

    #[test]
    fn register_rejects_zero_memory_limit() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        let config = WasmPluginConfig::new("zero-mem").with_memory_limit(0);
        let result = registry.register_bytes(config, &minimal_wasm_true());
        assert!(matches!(result, Err(WasmError::InvalidConfig(_))));
    }

    #[test]
    fn register_rejects_zero_timeout() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        let config = WasmPluginConfig::new("zero-timeout").with_timeout_ms(0);
        let result = registry.register_bytes(config, &minimal_wasm_true());
        assert!(matches!(result, Err(WasmError::InvalidConfig(_))));
    }

    #[test]
    fn register_rejects_excessive_memory() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        let config = WasmPluginConfig::new("big-mem").with_memory_limit(512 * 1024 * 1024); // 512 MB > 256 MB max
        let result = registry.register_bytes(config, &minimal_wasm_true());
        assert!(matches!(result, Err(WasmError::InvalidConfig(_))));
    }

    #[test]
    fn register_rejects_excessive_timeout() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        let config = WasmPluginConfig::new("slow-plugin").with_timeout_ms(60_000); // 60s > 30s max
        let result = registry.register_bytes(config, &minimal_wasm_true());
        assert!(matches!(result, Err(WasmError::InvalidConfig(_))));
    }

    #[tokio::test]
    async fn shared_registry_invoke_works() {
        let wasm_bytes = minimal_wasm_true();
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        let config = WasmPluginConfig::new("shared-test");
        registry.register_bytes(config, &wasm_bytes).unwrap();

        let shared = SharedWasmRegistry::new(registry);
        let result = shared
            .invoke("shared-test", "evaluate", &serde_json::json!({}))
            .await
            .unwrap();
        assert!(result.verdict);
    }

    #[tokio::test]
    async fn shared_registry_has_plugin_and_list() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        let config = WasmPluginConfig::new("listed");
        registry
            .register_bytes(config, &minimal_wasm_true())
            .unwrap();

        let shared = SharedWasmRegistry::new(registry);
        assert!(shared.has_plugin("listed"));
        assert!(!shared.has_plugin("not-listed"));

        let plugins = shared.list_plugins();
        assert!(plugins.contains(&"listed".to_owned()));
    }

    #[test]
    fn plugin_count_tracks_registrations() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        assert_eq!(registry.plugin_count(), 0);

        registry
            .register_bytes(WasmPluginConfig::new("p1"), &minimal_wasm_true())
            .unwrap();
        assert_eq!(registry.plugin_count(), 1);

        registry
            .register_bytes(WasmPluginConfig::new("p2"), &minimal_wasm_true())
            .unwrap();
        assert_eq!(registry.plugin_count(), 2);

        registry.unregister("p1");
        assert_eq!(registry.plugin_count(), 1);
    }

    #[test]
    fn registry_rejects_invalid_runtime_config() {
        let config = WasmRuntimeConfig {
            default_memory_limit_bytes: 0,
            ..WasmRuntimeConfig::default()
        };
        let result = WasmPluginRegistry::new(config);
        assert!(matches!(result, Err(WasmError::InvalidConfig(_))));
    }

    #[test]
    fn get_config_after_register() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        let config = WasmPluginConfig::new("cfg-test")
            .with_timeout_ms(50)
            .with_memory_limit(1024 * 1024);
        registry
            .register_bytes(config, &minimal_wasm_true())
            .unwrap();

        let got = registry.get_config("cfg-test").unwrap();
        assert_eq!(got.name, "cfg-test");
        assert_eq!(got.timeout_ms, 50);
        assert_eq!(got.memory_limit_bytes, 1024 * 1024);
    }

    #[test]
    fn re_register_overwrites_plugin() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();

        // Register with true verdict.
        let config = WasmPluginConfig::new("overwrite-me");
        registry
            .register_bytes(config, &minimal_wasm_true())
            .unwrap();
        let result = registry
            .invoke_internal("overwrite-me", "evaluate", &serde_json::json!({}))
            .unwrap();
        assert!(result.verdict);

        // Re-register with false verdict.
        let config = WasmPluginConfig::new("overwrite-me");
        registry
            .register_bytes(config, &minimal_wasm_false())
            .unwrap();
        let result = registry
            .invoke_internal("overwrite-me", "evaluate", &serde_json::json!({}))
            .unwrap();
        assert!(!result.verdict);

        // Plugin count should still be 1.
        assert_eq!(registry.plugin_count(), 1);
    }

    #[test]
    fn invoke_missing_function_errors() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        let config = WasmPluginConfig::new("fn-test");
        registry
            .register_bytes(config, &minimal_wasm_true())
            .unwrap();

        // Call a function that does not exist in the module.
        let result =
            registry.invoke_internal("fn-test", "nonexistent_function", &serde_json::json!({}));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("nonexistent_function"),
            "expected error to mention function name, got: {err}"
        );
    }

    #[test]
    fn registry_debug_format() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        registry
            .register_bytes(WasmPluginConfig::new("dbg"), &minimal_wasm_true())
            .unwrap();
        let debug = format!("{registry:?}");
        assert!(debug.contains("plugin_count: 1"));
    }

    #[test]
    fn list_plugins_after_multiple_registers() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        registry
            .register_bytes(WasmPluginConfig::new("alpha"), &minimal_wasm_true())
            .unwrap();
        registry
            .register_bytes(WasmPluginConfig::new("beta"), &minimal_wasm_true())
            .unwrap();
        registry
            .register_bytes(WasmPluginConfig::new("gamma"), &minimal_wasm_false())
            .unwrap();

        let mut plugins = registry.list_plugins();
        plugins.sort();
        assert_eq!(plugins, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn unregister_then_invoke_errors() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        registry
            .register_bytes(WasmPluginConfig::new("remove-me"), &minimal_wasm_true())
            .unwrap();

        assert!(registry.unregister("remove-me"));

        let result = registry.invoke_internal("remove-me", "evaluate", &serde_json::json!({}));
        assert!(matches!(result, Err(WasmError::PluginNotFound(_))));
    }

    #[test]
    fn runtime_config_accessor() {
        let config = WasmRuntimeConfig {
            enabled: true,
            plugin_dir: Some("/test/dir".into()),
            ..WasmRuntimeConfig::default()
        };
        let registry = WasmPluginRegistry::new(config).unwrap();
        assert!(registry.runtime_config().enabled);
        assert_eq!(
            registry.runtime_config().plugin_dir.as_deref(),
            Some("/test/dir")
        );
    }

    #[tokio::test]
    async fn shared_registry_invoke_nonexistent_errors() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        let shared = SharedWasmRegistry::new(registry);

        let result = shared
            .invoke("does-not-exist", "evaluate", &serde_json::json!({}))
            .await;
        assert!(matches!(result, Err(WasmError::PluginNotFound(_))));
    }

    #[test]
    fn shared_registry_debug() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        let shared = SharedWasmRegistry::new(registry);
        let debug = format!("{shared:?}");
        assert!(debug.contains("SharedWasmRegistry"));
    }

    #[test]
    fn shared_registry_clone() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        registry
            .register_bytes(WasmPluginConfig::new("clone-test"), &minimal_wasm_true())
            .unwrap();
        let shared = SharedWasmRegistry::new(registry);
        let cloned = shared.clone();

        // Both clones see the same plugins.
        assert!(cloned.has_plugin("clone-test"));
        assert_eq!(shared.list_plugins(), cloned.list_plugins());
    }

    #[test]
    fn shared_registry_access_underlying() {
        let registry = WasmPluginRegistry::new(test_config()).unwrap();
        let shared = SharedWasmRegistry::new(registry);

        // Register via the underlying registry accessor.
        shared
            .registry()
            .register_bytes(WasmPluginConfig::new("via-accessor"), &minimal_wasm_true())
            .unwrap();
        assert!(shared.has_plugin("via-accessor"));
    }

    #[test]
    fn load_empty_plugin_dir() {
        let dir = std::env::temp_dir().join("acteon-wasm-test-empty-dir");
        std::fs::create_dir_all(&dir).ok();

        let config = WasmRuntimeConfig {
            plugin_dir: Some(dir.display().to_string()),
            ..WasmRuntimeConfig::default()
        };
        let registry = WasmPluginRegistry::new(config).unwrap();
        let loaded = registry.load_plugin_dir().unwrap();
        assert_eq!(loaded, 0);

        // Cleanup.
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_plugin_dir_returns_zero() {
        let config = WasmRuntimeConfig::default();
        let registry = WasmPluginRegistry::new(config).unwrap();
        let loaded = registry.load_plugin_dir().unwrap();
        assert_eq!(loaded, 0);
    }
}
