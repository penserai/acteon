use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use acteon_state::key::{KeyKind, StateKey};
use acteon_state::store::StateStore;

use crate::config::RateLimitErrorBehavior;

use super::config::{RateLimitFileConfig, RateLimitTier};

/// The system namespace and tenant for rate limit keys.
const SYSTEM_NAMESPACE: &str = "_system";
const SYSTEM_TENANT: &str = "_ratelimit";

/// Bucket identifier for anonymous callers.
pub const ANONYMOUS_BUCKET: &str = "_anonymous";

/// Result of a rate limit check.
#[derive(Debug, Clone)]
pub struct RateLimitResult {
    /// Whether the request is allowed.
    pub allowed: bool,
    /// The configured limit for this tier.
    pub limit: u64,
    /// Approximate remaining requests in the current window.
    pub remaining: u64,
    /// Seconds until the current window resets.
    pub reset_after: u64,
}

/// Error returned when rate limit is exceeded.
#[derive(Debug)]
pub struct RateLimitExceeded {
    /// Seconds until the caller can retry.
    pub retry_after: u64,
    /// The configured limit.
    pub limit: u64,
}

/// Distributed rate limiter using the sliding window approximation algorithm.
///
/// Uses `StateStore::increment()` for atomic counters, making it work across
/// multiple server instances with any backend (Redis, `DynamoDB`, `PostgreSQL`, etc.).
pub struct RateLimiter {
    store: Arc<dyn StateStore>,
    config: RateLimitFileConfig,
    on_error: RateLimitErrorBehavior,
}

impl RateLimiter {
    /// Create a new rate limiter.
    pub fn new(
        store: Arc<dyn StateStore>,
        config: RateLimitFileConfig,
        on_error: RateLimitErrorBehavior,
    ) -> Self {
        Self {
            store,
            config,
            on_error,
        }
    }

    /// Get the rate limit configuration.
    pub fn config(&self) -> &RateLimitFileConfig {
        &self.config
    }

    /// Check and record a request for the given caller.
    ///
    /// Returns `Ok(RateLimitResult)` if allowed, `Err(RateLimitExceeded)` if blocked.
    /// On state store errors, behavior depends on `on_error` config.
    pub async fn check_caller_limit(
        &self,
        caller_id: &str,
    ) -> Result<RateLimitResult, RateLimitExceeded> {
        let tier = self.resolve_caller_tier(caller_id);
        let bucket = format!("caller:{caller_id}");
        self.check_limit(&bucket, tier).await
    }

    /// Check and record a request for the given tenant.
    ///
    /// Returns `Ok(RateLimitResult)` if allowed, `Err(RateLimitExceeded)` if blocked.
    pub async fn check_tenant_limit(
        &self,
        tenant_id: &str,
    ) -> Result<RateLimitResult, RateLimitExceeded> {
        let tier = self.resolve_tenant_tier(tenant_id);
        let bucket = format!("tenant:{tenant_id}");
        self.check_limit(&bucket, tier).await
    }

    /// Resolve the rate limit tier for a caller.
    fn resolve_caller_tier(&self, caller_id: &str) -> &RateLimitTier {
        if caller_id.is_empty() || caller_id == ANONYMOUS_BUCKET {
            return &self.config.callers.anonymous;
        }

        self.config
            .callers
            .overrides
            .get(caller_id)
            .unwrap_or(&self.config.callers.default)
    }

    /// Resolve the rate limit tier for a tenant.
    fn resolve_tenant_tier(&self, tenant_id: &str) -> &RateLimitTier {
        self.config
            .tenants
            .overrides
            .get(tenant_id)
            .unwrap_or(&self.config.tenants.default)
    }

    /// Core rate limiting logic using the sliding window approximation algorithm.
    ///
    /// Algorithm (used by Cloudflare, has ~2% error margin):
    /// 1. Compute current and previous window timestamps
    /// 2. Get counts for both windows
    /// 3. Calculate weighted effective count:
    ///    `effective = prev_count * weight + curr_count`
    ///    where `weight = (window_seconds - elapsed) / window_seconds`
    /// 4. If effective < limit, increment current window and allow
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    async fn check_limit(
        &self,
        bucket: &str,
        tier: &RateLimitTier,
    ) -> Result<RateLimitResult, RateLimitExceeded> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let window = tier.window_seconds;
        let limit = tier.requests_per_window;

        // Compute window boundaries
        let current_window_start = (now / window) * window;
        let previous_window_start = current_window_start.saturating_sub(window);
        let elapsed = now - current_window_start;

        // Build state keys
        let current_key = Self::build_key(bucket, current_window_start);
        let previous_key = Self::build_key(bucket, previous_window_start);

        // Get previous window count (for sliding window approximation)
        let prev_count = match self.store.get(&previous_key).await {
            Ok(Some(v)) => v.parse::<u64>().unwrap_or(0),
            Ok(None) => 0,
            Err(e) => {
                tracing::warn!(error = %e, "rate limiter: failed to get previous window count");
                return self.handle_store_error(tier);
            }
        };

        // Get current window count
        let curr_count = match self.store.get(&current_key).await {
            Ok(Some(v)) => v.parse::<u64>().unwrap_or(0),
            Ok(None) => 0,
            Err(e) => {
                tracing::warn!(error = %e, "rate limiter: failed to get current window count");
                return self.handle_store_error(tier);
            }
        };

        // Calculate sliding window approximation
        let weight = (window.saturating_sub(elapsed)) as f64 / window as f64;
        let effective_count = (prev_count as f64 * weight) as u64 + curr_count;

        // Check if over limit
        if effective_count >= limit {
            let reset_after = window.saturating_sub(elapsed);
            return Err(RateLimitExceeded {
                retry_after: reset_after.max(1),
                limit,
            });
        }

        // Increment current window counter
        let ttl = Duration::from_secs(window * 2); // Keep for 2 windows
        match self.store.increment(&current_key, 1, Some(ttl)).await {
            Ok(_new_count) => {
                let remaining = limit.saturating_sub(effective_count + 1);
                let reset_after = window.saturating_sub(elapsed);

                Ok(RateLimitResult {
                    allowed: true,
                    limit,
                    remaining,
                    reset_after,
                })
            }
            Err(e) => {
                tracing::warn!(error = %e, "rate limiter: failed to increment counter");
                self.handle_store_error(tier)
            }
        }
    }

    /// Build a state key for a rate limit bucket and window.
    fn build_key(bucket: &str, window_start: u64) -> StateKey {
        StateKey::new(
            SYSTEM_NAMESPACE,
            SYSTEM_TENANT,
            KeyKind::RateLimit,
            format!("{bucket}:{window_start}"),
        )
    }

    /// Handle state store errors according to the configured behavior.
    fn handle_store_error(
        &self,
        tier: &RateLimitTier,
    ) -> Result<RateLimitResult, RateLimitExceeded> {
        match self.on_error {
            RateLimitErrorBehavior::Allow => {
                // Fail-open: allow the request
                Ok(RateLimitResult {
                    allowed: true,
                    limit: tier.requests_per_window,
                    remaining: tier.requests_per_window,
                    reset_after: tier.window_seconds,
                })
            }
            RateLimitErrorBehavior::Deny => {
                // Fail-closed: deny the request
                Err(RateLimitExceeded {
                    retry_after: 60, // Default retry time on error
                    limit: tier.requests_per_window,
                })
            }
        }
    }
}
