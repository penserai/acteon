use serde::{Deserialize, Serialize};

use crate::error::WasmError;

/// Default memory limit for WASM plugins: 16 MB.
pub const DEFAULT_MEMORY_LIMIT_BYTES: u64 = 16 * 1024 * 1024;

/// Absolute maximum memory limit: 256 MB.
///
/// Prevents misconfiguration from allocating unbounded host memory.
pub const MAX_MEMORY_LIMIT_BYTES: u64 = 256 * 1024 * 1024;

/// Minimum meaningful timeout: 1 ms.
pub const MIN_TIMEOUT_MS: u64 = 1;

/// Default CPU timeout for WASM plugins: 100 ms.
pub const DEFAULT_TIMEOUT_MS: u64 = 100;

/// Maximum timeout: 30 seconds.
///
/// WASM plugins are meant to be fast condition checks, not long-running tasks.
pub const MAX_TIMEOUT_MS: u64 = 30_000;

/// Maximum number of plugins that can be tracked in the registry.
pub const MAX_TRACKED_PLUGINS: usize = 256;

/// Configuration for a single WASM plugin.
#[derive(Clone, Serialize, Deserialize)]
pub struct WasmPluginConfig {
    /// The plugin name (unique identifier).
    pub name: String,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// Maximum memory in bytes the plugin can use.
    #[serde(default = "default_memory_limit")]
    pub memory_limit_bytes: u64,
    /// Maximum execution time in milliseconds.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// Whether the plugin is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Path to the `.wasm` file (if loaded from disk).
    #[serde(default)]
    pub wasm_path: Option<String>,
}

impl std::fmt::Debug for WasmPluginConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmPluginConfig")
            .field("name", &self.name)
            .field("description", &self.description)
            .field("memory_limit_bytes", &self.memory_limit_bytes)
            .field("timeout_ms", &self.timeout_ms)
            .field("enabled", &self.enabled)
            .field("wasm_path", &self.wasm_path)
            .finish()
    }
}

impl WasmPluginConfig {
    /// Create a new plugin config with defaults.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            memory_limit_bytes: DEFAULT_MEMORY_LIMIT_BYTES,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            enabled: true,
            wasm_path: None,
        }
    }

    /// Set the plugin description.
    #[must_use]
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set the memory limit.
    #[must_use]
    pub fn with_memory_limit(mut self, bytes: u64) -> Self {
        self.memory_limit_bytes = bytes;
        self
    }

    /// Set the timeout.
    #[must_use]
    pub fn with_timeout_ms(mut self, ms: u64) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// Set the WASM file path.
    #[must_use]
    pub fn with_wasm_path(mut self, path: impl Into<String>) -> Self {
        self.wasm_path = Some(path.into());
        self
    }

    /// Set the enabled state.
    #[must_use]
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Validate the plugin configuration, returning an error with an
    /// actionable message if something is wrong.
    ///
    /// Call this before registering a plugin to catch misconfigurations early.
    pub fn validate(&self) -> Result<(), WasmError> {
        // Name must be non-empty and contain only safe characters.
        if self.name.is_empty() {
            return Err(WasmError::InvalidConfig(
                "plugin name must not be empty".into(),
            ));
        }
        if self.name.len() > 128 {
            return Err(WasmError::InvalidConfig(format!(
                "plugin name '{}...' exceeds 128 characters",
                &self.name[..32]
            )));
        }
        if !self
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return Err(WasmError::InvalidConfig(format!(
                "plugin name '{}' contains invalid characters \
                 (only alphanumeric, '-', '_', '.' allowed)",
                self.name
            )));
        }

        // Memory limits.
        if self.memory_limit_bytes == 0 {
            return Err(WasmError::InvalidConfig(format!(
                "plugin '{}': memory_limit_bytes must be > 0",
                self.name
            )));
        }
        if self.memory_limit_bytes > MAX_MEMORY_LIMIT_BYTES {
            return Err(WasmError::InvalidConfig(format!(
                "plugin '{}': memory_limit_bytes ({}) exceeds maximum of {} (256 MB). \
                 WASM plugins should be lightweight condition checks, not full applications.",
                self.name, self.memory_limit_bytes, MAX_MEMORY_LIMIT_BYTES
            )));
        }

        // Timeout limits.
        if self.timeout_ms < MIN_TIMEOUT_MS {
            return Err(WasmError::InvalidConfig(format!(
                "plugin '{}': timeout_ms must be >= {MIN_TIMEOUT_MS}",
                self.name
            )));
        }
        if self.timeout_ms > MAX_TIMEOUT_MS {
            return Err(WasmError::InvalidConfig(format!(
                "plugin '{}': timeout_ms ({}) exceeds maximum of {} (30s). \
                 If your plugin needs more time, consider moving the logic to \
                 a chain step or custom action handler instead.",
                self.name, self.timeout_ms, MAX_TIMEOUT_MS
            )));
        }

        Ok(())
    }
}

const fn default_memory_limit() -> u64 {
    DEFAULT_MEMORY_LIMIT_BYTES
}

const fn default_timeout_ms() -> u64 {
    DEFAULT_TIMEOUT_MS
}

const fn default_true() -> bool {
    true
}

/// Global WASM runtime configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmRuntimeConfig {
    /// Whether the WASM runtime is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Directory to scan for `.wasm` plugin files.
    #[serde(default)]
    pub plugin_dir: Option<String>,
    /// Default memory limit for plugins (overridable per-plugin).
    #[serde(default = "default_memory_limit")]
    pub default_memory_limit_bytes: u64,
    /// Default timeout for plugins (overridable per-plugin).
    #[serde(default = "default_timeout_ms")]
    pub default_timeout_ms: u64,
}

impl Default for WasmRuntimeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            plugin_dir: None,
            default_memory_limit_bytes: DEFAULT_MEMORY_LIMIT_BYTES,
            default_timeout_ms: DEFAULT_TIMEOUT_MS,
        }
    }
}

impl WasmRuntimeConfig {
    /// Validate the runtime configuration.
    ///
    /// The default memory/timeout values follow the same bounds as
    /// per-plugin config, because every plugin created via
    /// `load_plugin_dir` inherits these defaults.
    pub fn validate(&self) -> Result<(), WasmError> {
        if self.default_memory_limit_bytes == 0 {
            return Err(WasmError::InvalidConfig(
                "default_memory_limit_bytes must be > 0".into(),
            ));
        }
        if self.default_memory_limit_bytes > MAX_MEMORY_LIMIT_BYTES {
            return Err(WasmError::InvalidConfig(format!(
                "default_memory_limit_bytes ({}) exceeds maximum of {} (256 MB)",
                self.default_memory_limit_bytes, MAX_MEMORY_LIMIT_BYTES
            )));
        }
        if self.default_timeout_ms < MIN_TIMEOUT_MS {
            return Err(WasmError::InvalidConfig(format!(
                "default_timeout_ms must be >= {MIN_TIMEOUT_MS}"
            )));
        }
        if self.default_timeout_ms > MAX_TIMEOUT_MS {
            return Err(WasmError::InvalidConfig(format!(
                "default_timeout_ms ({}) exceeds maximum of {} (30s)",
                self.default_timeout_ms, MAX_TIMEOUT_MS
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults() {
        let config = WasmPluginConfig::new("test-plugin");
        assert_eq!(config.name, "test-plugin");
        assert_eq!(config.memory_limit_bytes, DEFAULT_MEMORY_LIMIT_BYTES);
        assert_eq!(config.timeout_ms, DEFAULT_TIMEOUT_MS);
        assert!(config.enabled);
        assert!(config.wasm_path.is_none());
    }

    #[test]
    fn config_builder() {
        let config = WasmPluginConfig::new("my-plugin")
            .with_description("A test plugin")
            .with_memory_limit(1024 * 1024)
            .with_timeout_ms(50)
            .with_wasm_path("/plugins/my-plugin.wasm")
            .with_enabled(false);

        assert_eq!(config.name, "my-plugin");
        assert_eq!(config.description.as_deref(), Some("A test plugin"));
        assert_eq!(config.memory_limit_bytes, 1024 * 1024);
        assert_eq!(config.timeout_ms, 50);
        assert_eq!(config.wasm_path.as_deref(), Some("/plugins/my-plugin.wasm"));
        assert!(!config.enabled);
    }

    #[test]
    fn config_serde_roundtrip() {
        let config = WasmPluginConfig::new("serde-test").with_timeout_ms(200);
        let json = serde_json::to_string(&config).unwrap();
        let back: WasmPluginConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "serde-test");
        assert_eq!(back.timeout_ms, 200);
    }

    #[test]
    fn runtime_config_defaults() {
        let config = WasmRuntimeConfig::default();
        assert!(!config.enabled);
        assert!(config.plugin_dir.is_none());
    }

    #[test]
    fn debug_does_not_expose_internals() {
        let config = WasmPluginConfig::new("dbg-test");
        let debug_str = format!("{config:?}");
        assert!(debug_str.contains("dbg-test"));
    }

    // --- Validation tests ---

    #[test]
    fn validate_valid_config() {
        let config = WasmPluginConfig::new("my-plugin");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_empty_name() {
        let config = WasmPluginConfig::new("");
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn validate_name_too_long() {
        let long_name = "a".repeat(200);
        let config = WasmPluginConfig::new(long_name);
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("exceeds 128 characters"));
    }

    #[test]
    fn validate_name_invalid_chars() {
        let config = WasmPluginConfig::new("bad name!");
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("invalid characters"));
    }

    #[test]
    fn validate_name_with_allowed_special_chars() {
        let config = WasmPluginConfig::new("my-plugin_v2.0");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_zero_memory() {
        let config = WasmPluginConfig::new("test").with_memory_limit(0);
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("memory_limit_bytes must be > 0"));
    }

    #[test]
    fn validate_excessive_memory() {
        let config = WasmPluginConfig::new("test").with_memory_limit(MAX_MEMORY_LIMIT_BYTES + 1);
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("exceeds maximum"));
        assert!(err.to_string().contains("256 MB"));
    }

    #[test]
    fn validate_zero_timeout() {
        let config = WasmPluginConfig::new("test").with_timeout_ms(0);
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("timeout_ms must be >="));
    }

    #[test]
    fn validate_excessive_timeout() {
        let config = WasmPluginConfig::new("test").with_timeout_ms(MAX_TIMEOUT_MS + 1);
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("exceeds maximum"));
        assert!(err.to_string().contains("chain step"));
    }

    // --- WasmRuntimeConfig validation tests ---

    #[test]
    fn runtime_config_validate_defaults_ok() {
        let config = WasmRuntimeConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn runtime_config_validate_zero_memory() {
        let config = WasmRuntimeConfig {
            default_memory_limit_bytes: 0,
            ..WasmRuntimeConfig::default()
        };
        let err = config.validate().unwrap_err();
        assert!(
            err.to_string()
                .contains("default_memory_limit_bytes must be > 0")
        );
    }

    #[test]
    fn runtime_config_validate_excessive_memory() {
        let config = WasmRuntimeConfig {
            default_memory_limit_bytes: MAX_MEMORY_LIMIT_BYTES + 1,
            ..WasmRuntimeConfig::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("exceeds maximum"));
    }

    #[test]
    fn runtime_config_validate_zero_timeout() {
        let config = WasmRuntimeConfig {
            default_timeout_ms: 0,
            ..WasmRuntimeConfig::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("default_timeout_ms must be >="));
    }

    #[test]
    fn runtime_config_validate_excessive_timeout() {
        let config = WasmRuntimeConfig {
            default_timeout_ms: MAX_TIMEOUT_MS + 1,
            ..WasmRuntimeConfig::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("exceeds maximum"));
    }
}
