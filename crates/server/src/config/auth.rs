use serde::{Deserialize, Serialize};

/// Reference to the auth config file from `acteon.toml`.
#[derive(Debug, Default, Deserialize)]
pub struct AuthRefConfig {
    /// Whether authentication/authorization is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Path to the auth config file (`auth.toml`), relative to `acteon.toml` or absolute.
    pub config_path: Option<String>,
    /// Whether to watch the auth config file for changes (hot-reload). Defaults to `true`.
    pub watch: Option<bool>,
}

/// Reference to the rate limit config file from `acteon.toml`.
#[derive(Debug, Default, Deserialize)]
pub struct RateLimitRefConfig {
    /// Whether rate limiting is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Path to the rate limit config file (`ratelimit.toml`), relative to `acteon.toml` or absolute.
    pub config_path: Option<String>,
    /// Behavior when the state store is unavailable.
    #[serde(default)]
    pub on_error: RateLimitErrorBehavior,
}

/// Behavior when the rate limiter's state store is unavailable.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitErrorBehavior {
    /// Allow requests through (fail-open).
    #[default]
    Allow,
    /// Deny requests (fail-closed).
    Deny,
}
