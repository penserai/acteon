use serde::Deserialize;

/// Configuration for payload encryption at rest.
///
/// When enabled, action payloads stored in the state and audit backends are
/// encrypted using AES-256-GCM. Requires the `ACTEON_PAYLOAD_KEY` environment
/// variable to be set to a 32-byte key (hex or base64 encoded).
///
/// # Example
///
/// ```toml
/// [encryption]
/// enabled = true
/// ```
#[derive(Debug, Default, Deserialize)]
pub struct EncryptionConfig {
    /// Whether payload encryption is enabled.
    #[serde(default)]
    pub enabled: bool,
}

/// Configuration for tenant quota policies.
#[derive(Debug, Deserialize)]
pub struct QuotaConfig {
    /// Whether quota enforcement is enabled.
    #[serde(default = "default_quotas_enabled")]
    pub enabled: bool,
    /// Default window for new quota policies (e.g., `"daily"`).
    #[serde(default)]
    pub default_window: Option<String>,
    /// Default overage behavior for new quota policies (e.g., `"block"`).
    #[serde(default)]
    pub default_overage_behavior: Option<String>,
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self {
            enabled: default_quotas_enabled(),
            default_window: None,
            default_overage_behavior: None,
        }
    }
}

fn default_quotas_enabled() -> bool {
    true
}

/// Configuration for payload templates.
#[derive(Debug, Default, Deserialize)]
pub struct TemplateServerConfig {
    /// Directory to scan for `.jinja` template files on startup.
    #[serde(default)]
    pub directory: Option<String>,
    /// Directory to scan for profile YAML files on startup.
    #[serde(default)]
    pub profiles_directory: Option<String>,
}

/// Configuration for action payload attachments.
///
/// Controls the maximum decoded size for a single attachment and the maximum
/// number of attachments per action.
///
/// # Example
///
/// ```toml
/// [attachments]
/// max_inline_bytes = 5242880
/// max_attachments_per_action = 10
/// ```
#[derive(Debug, Deserialize)]
pub struct AttachmentConfig {
    /// Maximum decoded size in bytes for a single attachment (default: 5 MB).
    #[serde(default = "default_max_inline_bytes")]
    pub max_inline_bytes: u64,
    /// Maximum number of attachments allowed per action (default: 10).
    #[serde(default = "default_max_attachments_per_action")]
    pub max_attachments_per_action: usize,
}

impl Default for AttachmentConfig {
    fn default() -> Self {
        Self {
            max_inline_bytes: default_max_inline_bytes(),
            max_attachments_per_action: default_max_attachments_per_action(),
        }
    }
}

fn default_max_inline_bytes() -> u64 {
    5 * 1024 * 1024 // 5 MB
}

fn default_max_attachments_per_action() -> usize {
    10
}

/// Configuration for the WASM plugin runtime.
///
/// When enabled, Acteon loads `.wasm` plugin files from the configured
/// directory and makes them available for use in rule conditions via the
/// `wasm()` predicate.
///
/// # Example
///
/// ```toml
/// [wasm]
/// enabled = true
/// plugin_dir = "/etc/acteon/plugins"
/// default_memory_limit_bytes = 16777216
/// default_timeout_ms = 100
/// ```
#[derive(Debug, Deserialize)]
pub struct WasmServerConfig {
    /// Whether the WASM plugin runtime is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Directory to scan for `.wasm` plugin files on startup.
    #[serde(default)]
    pub plugin_dir: Option<String>,
    /// Default memory limit for plugins in bytes (default: 16 MB).
    #[serde(default = "default_wasm_memory_limit")]
    pub default_memory_limit_bytes: u64,
    /// Default execution timeout for plugins in milliseconds (default: 100 ms).
    #[serde(default = "default_wasm_timeout_ms")]
    pub default_timeout_ms: u64,
}

impl Default for WasmServerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            plugin_dir: None,
            default_memory_limit_bytes: default_wasm_memory_limit(),
            default_timeout_ms: default_wasm_timeout_ms(),
        }
    }
}

fn default_wasm_memory_limit() -> u64 {
    16 * 1024 * 1024 // 16 MB
}

fn default_wasm_timeout_ms() -> u64 {
    100
}

/// Compliance mode configuration for `SOC2` / `HIPAA` audit mode.
///
/// # Example
///
/// ```toml
/// [compliance]
/// mode = "soc2"       # "none", "soc2", or "hipaa"
/// sync_audit_writes = true
/// immutable_audit = false
/// hash_chain = true
/// ```
#[derive(Debug, Default, Deserialize)]
pub struct ComplianceServerConfig {
    /// Compliance mode preset: `"none"` (default), `"soc2"`, or `"hipaa"`.
    ///
    /// Each mode pre-configures sensible defaults that can be individually
    /// overridden by setting the other fields explicitly.
    #[serde(default)]
    pub mode: String,
    /// Override: whether audit writes must be synchronous.
    #[serde(default)]
    pub sync_audit_writes: Option<bool>,
    /// Override: whether audit records are immutable.
    #[serde(default)]
    pub immutable_audit: Option<bool>,
    /// Override: whether `SHA-256` hash chaining is enabled.
    #[serde(default)]
    pub hash_chain: Option<bool>,
}

impl ComplianceServerConfig {
    /// Convert this config into a [`acteon_core::ComplianceConfig`], applying
    /// mode presets first, then any explicit overrides.
    pub fn to_compliance_config(&self) -> acteon_core::ComplianceConfig {
        let mode = match self.mode.to_lowercase().as_str() {
            "soc2" => acteon_core::ComplianceMode::Soc2,
            "hipaa" => acteon_core::ComplianceMode::Hipaa,
            _ => acteon_core::ComplianceMode::None,
        };

        let mut config = acteon_core::ComplianceConfig::new(mode);

        if let Some(v) = self.sync_audit_writes {
            config = config.with_sync_audit_writes(v);
        }
        if let Some(v) = self.immutable_audit {
            config = config.with_immutable_audit(v);
        }
        if let Some(v) = self.hash_chain {
            config = config.with_hash_chain(v);
        }

        config
    }

    /// Returns `true` if any compliance feature is enabled.
    pub fn is_active(&self) -> bool {
        let config = self.to_compliance_config();
        config.mode != acteon_core::ComplianceMode::None
            || config.sync_audit_writes
            || config.immutable_audit
            || config.hash_chain
    }
}
