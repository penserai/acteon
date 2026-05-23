use std::time::Duration;

/// Configuration for the Redis state store and distributed lock backends.
#[derive(Debug, Clone)]
pub struct RedisConfig {
    /// Redis connection URL (e.g. `redis://127.0.0.1:6379`).
    ///
    /// Use `rediss://` scheme for TLS connections. When `tls_enabled` is set,
    /// the URL scheme is automatically upgraded to `rediss://`.
    pub url: String,

    /// Key prefix applied to every Redis key to avoid collisions.
    pub prefix: String,

    /// Number of connections in the `deadpool-redis` pool.
    pub pool_size: usize,

    /// Timeout for acquiring a pooled connection.
    pub connection_timeout: Duration,

    /// Whether TLS is enabled. When `true`, a `redis://` URL is automatically
    /// upgraded to `rediss://`.
    pub tls_enabled: bool,

    /// Accept invalid certificates (dev/test only). Only applies when using
    /// `rediss://` connections.
    pub tls_insecure: bool,
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            url: String::from("redis://127.0.0.1:6379"),
            prefix: String::from("acteon"),
            pool_size: 10,
            connection_timeout: Duration::from_secs(5),
            tls_enabled: false,
            tls_insecure: false,
        }
    }
}

impl RedisConfig {
    /// Return the effective connection URL, upgrading to `rediss://` when TLS
    /// is enabled.
    pub fn effective_url(&self) -> String {
        if self.tls_enabled && self.url.starts_with("redis://") {
            self.url.replacen("redis://", "rediss://", 1)
        } else {
            self.url.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let cfg = RedisConfig::default();
        assert_eq!(cfg.url, "redis://127.0.0.1:6379");
        assert_eq!(cfg.prefix, "acteon");
        assert_eq!(cfg.pool_size, 10);
        assert_eq!(cfg.connection_timeout, Duration::from_secs(5));
    }
}
