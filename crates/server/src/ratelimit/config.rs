use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Schema for `ratelimit.toml`, the separate rate limit configuration file.
#[derive(Debug, Default, Deserialize)]
pub struct RateLimitFileConfig {
    /// Per-caller rate limiting configuration.
    #[serde(default)]
    pub callers: CallerRateLimitConfig,
    /// Per-tenant rate limiting configuration.
    #[serde(default)]
    pub tenants: TenantRateLimitConfig,
}

/// Configuration for per-caller rate limiting.
#[derive(Debug, Default, Deserialize)]
pub struct CallerRateLimitConfig {
    /// Default rate limit tier for authenticated callers.
    #[serde(default)]
    pub default: RateLimitTier,
    /// Rate limit tier for anonymous users (when auth is disabled).
    #[serde(default = "default_anonymous_tier")]
    pub anonymous: RateLimitTier,
    /// Per-caller overrides keyed by `CallerIdentity.id`.
    #[serde(default)]
    pub overrides: HashMap<String, RateLimitTier>,
}

/// Configuration for per-tenant rate limiting.
#[derive(Debug, Default, Deserialize)]
pub struct TenantRateLimitConfig {
    /// Whether per-tenant rate limiting is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Default rate limit tier for tenants without overrides.
    #[serde(default = "default_tenant_tier")]
    pub default: RateLimitTier,
    /// Per-tenant overrides keyed by tenant ID.
    #[serde(default)]
    pub overrides: HashMap<String, RateLimitTier>,
}

/// A rate limit tier defining the limit and window.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RateLimitTier {
    /// Maximum number of requests allowed per window.
    #[serde(default = "default_requests")]
    pub requests_per_window: u64,
    /// Window duration in seconds.
    #[serde(default = "default_window")]
    pub window_seconds: u64,
}

impl Default for RateLimitTier {
    fn default() -> Self {
        Self {
            requests_per_window: default_requests(),
            window_seconds: default_window(),
        }
    }
}

fn default_requests() -> u64 {
    1000
}

fn default_window() -> u64 {
    60
}

fn default_anonymous_tier() -> RateLimitTier {
    RateLimitTier {
        requests_per_window: 100,
        window_seconds: 60,
    }
}

fn default_tenant_tier() -> RateLimitTier {
    RateLimitTier {
        requests_per_window: 10000,
        window_seconds: 60,
    }
}
