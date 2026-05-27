use serde::Deserialize;

/// Configuration for loading rules from disk.
#[derive(Debug, Default, Deserialize)]
pub struct RulesConfig {
    /// Optional path to a directory containing YAML rule files.
    pub directory: Option<String>,
    /// Default IANA timezone for time-based rule conditions (e.g. `"US/Eastern"`).
    ///
    /// When set, `time.*` fields use this timezone unless a rule provides its
    /// own `timezone` override. If not set, UTC is used.
    pub default_timezone: Option<String>,
    /// Whether to watch `directory` for changes and hot-reload rules.
    /// Defaults to `true` when a `directory` is configured.
    /// Equivalent to repeatedly calling `POST /v1/rules/reload`.
    #[serde(default = "default_rules_watch")]
    pub watch: bool,
}

fn default_rules_watch() -> bool {
    true
}

/// Configuration for the action executor.
#[derive(Debug, Default, Deserialize)]
pub struct ExecutorConfig {
    /// Maximum retry attempts per action.
    pub max_retries: Option<u32>,
    /// Per-action execution timeout in seconds.
    pub timeout_seconds: Option<u64>,
    /// Maximum number of actions executing concurrently.
    pub max_concurrent: Option<usize>,
    /// Whether to enable the dead-letter queue for failed actions.
    #[serde(default)]
    pub dlq_enabled: bool,
}
