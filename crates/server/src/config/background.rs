use serde::Deserialize;

/// Configuration for background processing (group flushing, timeouts).
#[derive(Debug, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct BackgroundProcessingConfig {
    /// Whether background processing is enabled.
    #[serde(default = "default_background_enabled")]
    pub enabled: bool,
    /// How often to check for ready groups (seconds).
    #[serde(default = "default_group_flush_interval")]
    pub group_flush_interval_seconds: u64,
    /// How often to check for state machine timeouts (seconds).
    #[serde(default = "default_timeout_check_interval")]
    pub timeout_check_interval_seconds: u64,
    /// How often to run cleanup tasks (seconds).
    #[serde(default = "default_cleanup_interval_bg")]
    pub cleanup_interval_seconds: u64,
    /// Whether to flush groups automatically.
    #[serde(default = "default_enable_group_flush")]
    pub enable_group_flush: bool,
    /// Whether to process state machine timeouts.
    #[serde(default = "default_enable_timeout_processing")]
    pub enable_timeout_processing: bool,
    /// Whether to retry failed approval notifications.
    #[serde(default = "default_enable_approval_retry")]
    pub enable_approval_retry: bool,
    /// Whether to process scheduled actions.
    #[serde(default)]
    pub enable_scheduled_actions: bool,
    /// How often to check for due scheduled actions (seconds).
    #[serde(default = "default_scheduled_check_interval")]
    pub scheduled_check_interval_seconds: u64,
    /// Whether to process recurring actions.
    #[serde(default)]
    pub enable_recurring_actions: bool,
    /// How often to check for due recurring actions (seconds).
    #[serde(default = "default_recurring_check_interval")]
    pub recurring_check_interval_seconds: u64,
    /// Maximum number of recurring actions per tenant.
    #[serde(default = "default_max_recurring_actions_per_tenant")]
    pub max_recurring_actions_per_tenant: usize,
    /// Whether to run the data retention reaper.
    #[serde(default)]
    pub enable_retention_reaper: bool,
    /// How often to run the data retention reaper (seconds).
    #[serde(default = "default_retention_check_interval")]
    pub retention_check_interval_seconds: u64,
    /// Whether to periodically sync templates from the state store.
    #[serde(default = "default_enable_template_sync")]
    pub enable_template_sync: bool,
    /// How often to sync templates from the state store (seconds).
    #[serde(default = "default_template_sync_interval")]
    pub template_sync_interval_seconds: u64,
    /// Namespace to scan for timeouts (required for timeout processing).
    #[serde(default)]
    pub namespace: String,
    /// Tenant to scan for timeouts (required for timeout processing).
    #[serde(default)]
    pub tenant: String,
}

impl Default for BackgroundProcessingConfig {
    fn default() -> Self {
        Self {
            enabled: default_background_enabled(),
            group_flush_interval_seconds: default_group_flush_interval(),
            timeout_check_interval_seconds: default_timeout_check_interval(),
            cleanup_interval_seconds: default_cleanup_interval_bg(),
            enable_group_flush: default_enable_group_flush(),
            enable_timeout_processing: default_enable_timeout_processing(),
            enable_approval_retry: default_enable_approval_retry(),
            enable_scheduled_actions: false,
            scheduled_check_interval_seconds: default_scheduled_check_interval(),
            enable_recurring_actions: false,
            recurring_check_interval_seconds: default_recurring_check_interval(),
            max_recurring_actions_per_tenant: default_max_recurring_actions_per_tenant(),
            enable_retention_reaper: false,
            retention_check_interval_seconds: default_retention_check_interval(),
            enable_template_sync: default_enable_template_sync(),
            template_sync_interval_seconds: default_template_sync_interval(),
            namespace: String::new(),
            tenant: String::new(),
        }
    }
}

fn default_retention_check_interval() -> u64 {
    3600
}

fn default_enable_template_sync() -> bool {
    true
}

fn default_template_sync_interval() -> u64 {
    30
}

fn default_background_enabled() -> bool {
    false
}

fn default_group_flush_interval() -> u64 {
    5
}

fn default_timeout_check_interval() -> u64 {
    10
}

fn default_cleanup_interval_bg() -> u64 {
    60
}

fn default_enable_group_flush() -> bool {
    true
}

fn default_enable_timeout_processing() -> bool {
    true
}

fn default_enable_approval_retry() -> bool {
    true
}

fn default_scheduled_check_interval() -> u64 {
    5
}

fn default_recurring_check_interval() -> u64 {
    60
}

fn default_max_recurring_actions_per_tenant() -> usize {
    100
}
