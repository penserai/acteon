use std::time::Duration;

use async_trait::async_trait;
use deadpool_redis::{Config, Pool, Runtime};
use redis::{AsyncCommands, Script};

use acteon_state::error::StateError;
use acteon_state::key::{KeyKind, StateKey};
use acteon_state::store::{CasResult, StateStore};

use crate::config::RedisConfig;
use crate::key_render::render_key;
use crate::scripts;

/// Redis-backed implementation of [`StateStore`].
///
/// Uses a `deadpool-redis` connection pool and Lua scripts for atomicity.
/// Regular values are stored as plain Redis strings. Versioned values (used by
/// `compare_and_swap` and `set`) are stored as Redis hashes with fields `v`
/// (value) and `ver` (version).
pub struct RedisStateStore {
    pool: Pool,
    prefix: String,
}

impl RedisStateStore {
    /// Create a new `RedisStateStore` from the provided configuration.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::Connection`] if the pool cannot be created.
    pub fn new(config: &RedisConfig) -> Result<Self, StateError> {
        let cfg = Config::from_url(&config.url);
        let pool = cfg
            .builder()
            .map(|b| {
                b.max_size(config.pool_size)
                    .wait_timeout(Some(config.connection_timeout))
                    .runtime(Runtime::Tokio1)
                    .build()
            })
            .map_err(|e| StateError::Connection(e.to_string()))?
            .map_err(|e| StateError::Connection(e.to_string()))?;

        Ok(Self {
            pool,
            prefix: config.prefix.clone(),
        })
    }

    /// Build the full Redis key for a hash-based entry (versioned data).
    fn hash_key(&self, key: &StateKey) -> String {
        format!("{}:h", render_key(&self.prefix, key))
    }

    /// Build the full Redis key for a plain string entry.
    fn string_key(&self, key: &StateKey) -> String {
        render_key(&self.prefix, key)
    }

    /// Obtain a connection from the pool.
    async fn conn(&self) -> Result<deadpool_redis::Connection, StateError> {
        self.pool
            .get()
            .await
            .map_err(|e| StateError::Connection(e.to_string()))
    }
}

#[async_trait]
impl StateStore for RedisStateStore {
    async fn check_and_set(
        &self,
        key: &StateKey,
        value: &str,
        ttl: Option<Duration>,
    ) -> Result<bool, StateError> {
        let string_key = self.string_key(key);
        let hash_key = self.hash_key(key);
        let ttl_ms = ttl.map_or(0i64, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));

        let mut conn = self.conn().await?;
        let script = Script::new(scripts::CHECK_AND_SET);
        let result: i64 = script
            .key(&string_key)
            .key(&hash_key)
            .arg(value)
            .arg(ttl_ms)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(result == 1)
    }

    async fn get(&self, key: &StateKey) -> Result<Option<String>, StateError> {
        let redis_key = self.hash_key(key);
        let mut conn = self.conn().await?;

        // First try hash-based storage (set by `set` / `compare_and_swap`).
        let val: Option<String> = conn
            .hget(&redis_key, "v")
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        if val.is_some() {
            return Ok(val);
        }

        // Fall back to plain string key (set by `check_and_set`).
        let string_key = self.string_key(key);
        let val: Option<String> = conn
            .get(&string_key)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(val)
    }

    async fn set(
        &self,
        key: &StateKey,
        value: &str,
        ttl: Option<Duration>,
    ) -> Result<(), StateError> {
        let redis_key = self.hash_key(key);
        let mut conn = self.conn().await?;

        // Read current version; if missing start at 0, then increment to 1.
        let cur_ver: Option<u64> = conn
            .hget(&redis_key, "ver")
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;
        let new_ver = cur_ver.map_or(1, |v| v + 1);

        redis::pipe()
            .hset(&redis_key, "v", value)
            .ignore()
            .hset(&redis_key, "ver", new_ver)
            .ignore()
            .exec_async(&mut conn)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        if let Some(d) = ttl {
            let ms = i64::try_from(d.as_millis()).unwrap_or(i64::MAX);
            let () = conn
                .pexpire(&redis_key, ms)
                .await
                .map_err(|e| StateError::Backend(e.to_string()))?;
        }

        // Also remove any plain-string key so `get` reads consistently.
        let string_key = self.string_key(key);
        let _: () = conn
            .del(&string_key)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(())
    }

    async fn delete(&self, key: &StateKey) -> Result<bool, StateError> {
        let hash_key = self.hash_key(key);
        let string_key = self.string_key(key);
        let mut conn = self.conn().await?;

        let deleted: i64 = redis::pipe()
            .del(&hash_key)
            .del(&string_key)
            .query_async(&mut conn)
            .await
            .map(|(a, b): (i64, i64)| a + b)
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(deleted > 0)
    }

    async fn increment(
        &self,
        key: &StateKey,
        delta: i64,
        ttl: Option<Duration>,
    ) -> Result<i64, StateError> {
        // Counters use plain string keys with INCRBY.
        let redis_key = self.string_key(key);
        let mut conn = self.conn().await?;

        let new_val: i64 = conn
            .incr(&redis_key, delta)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        if let Some(d) = ttl {
            let ms = i64::try_from(d.as_millis()).unwrap_or(i64::MAX);
            let () = conn
                .pexpire(&redis_key, ms)
                .await
                .map_err(|e| StateError::Backend(e.to_string()))?;
        }

        Ok(new_val)
    }

    async fn compare_and_swap(
        &self,
        key: &StateKey,
        expected_version: u64,
        new_value: &str,
        ttl: Option<Duration>,
    ) -> Result<CasResult, StateError> {
        let redis_key = self.hash_key(key);
        let ttl_ms = ttl.map_or(0i64, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));

        let mut conn = self.conn().await?;
        let script = Script::new(scripts::COMPARE_AND_SWAP);
        let result: Vec<redis::Value> = script
            .key(&redis_key)
            .arg(expected_version)
            .arg(new_value)
            .arg(ttl_ms)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        // Parse the Lua return value.
        // Success: [1, new_version]
        // Conflict: [0, current_version, current_value | false]
        let status = match result.first() {
            Some(redis::Value::Int(n)) => *n,
            _ => return Err(StateError::Backend("unexpected CAS script response".into())),
        };

        if status == 1 {
            Ok(CasResult::Ok)
        } else {
            let current_version = match result.get(1) {
                Some(redis::Value::Int(n)) => u64::try_from(*n).unwrap_or(0),
                _ => 0,
            };
            let current_value = match result.get(2) {
                Some(redis::Value::BulkString(bytes)) => String::from_utf8(bytes.clone()).ok(),
                _ => None,
            };

            Ok(CasResult::Conflict {
                current_value,
                current_version,
            })
        }
    }

    async fn scan_keys(
        &self,
        namespace: &str,
        tenant: &str,
        kind: KeyKind,
        prefix: Option<&str>,
    ) -> Result<Vec<(String, String)>, StateError> {
        let pattern = match prefix {
            Some(p) => format!("{}:{}:{}:{}:{}*", self.prefix, namespace, tenant, kind, p),
            None => format!("{}:{}:{}:{}:*", self.prefix, namespace, tenant, kind),
        };

        let mut conn = self.conn().await?;
        let mut results = Vec::new();
        let mut cursor = 0u64;

        loop {
            let (new_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(100)
                .query_async(&mut conn)
                .await
                .map_err(|e| StateError::Backend(e.to_string()))?;

            for key in keys {
                // Try to get value from hash first, then plain string
                let val: Option<String> = conn
                    .hget(&key, "v")
                    .await
                    .map_err(|e| StateError::Backend(e.to_string()))?;

                let value = if let Some(v) = val {
                    v
                } else {
                    // Try plain string key (without :h suffix)
                    let plain_key = key.strip_suffix(":h").unwrap_or(&key);
                    let v: Option<String> = conn
                        .get(plain_key)
                        .await
                        .map_err(|e| StateError::Backend(e.to_string()))?;
                    match v {
                        Some(s) => s,
                        None => continue,
                    }
                };

                // Strip the prefix to return a clean key
                let clean_key = key
                    .strip_prefix(&format!("{}:", self.prefix))
                    .unwrap_or(&key)
                    .strip_suffix(":h")
                    .unwrap_or(&key)
                    .to_string();

                results.push((clean_key, value));
            }

            cursor = new_cursor;
            if cursor == 0 {
                break;
            }
        }

        Ok(results)
    }

    async fn scan_keys_by_kind(&self, kind: KeyKind) -> Result<Vec<(String, String)>, StateError> {
        // Scan all keys matching the pattern: {prefix}:*:*:{kind}:*
        // This matches keys across all namespaces and tenants for the given kind.
        let pattern = format!("{}:*:*:{}:*", self.prefix, kind);

        let mut conn = self.conn().await?;
        let mut results = Vec::new();
        let mut cursor = 0u64;

        loop {
            let (new_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(100)
                .query_async(&mut conn)
                .await
                .map_err(|e| StateError::Backend(e.to_string()))?;

            for key in keys {
                // Try to get value from hash first, then plain string
                let val: Option<String> = conn
                    .hget(&key, "v")
                    .await
                    .map_err(|e| StateError::Backend(e.to_string()))?;

                let value = if let Some(v) = val {
                    v
                } else {
                    // Try plain string key (without :h suffix)
                    let plain_key = key.strip_suffix(":h").unwrap_or(&key);
                    let v: Option<String> = conn
                        .get(plain_key)
                        .await
                        .map_err(|e| StateError::Backend(e.to_string()))?;
                    match v {
                        Some(s) => s,
                        None => continue,
                    }
                };

                // Strip the prefix to return a clean key
                let clean_key = key
                    .strip_prefix(&format!("{}:", self.prefix))
                    .unwrap_or(&key)
                    .strip_suffix(":h")
                    .unwrap_or(&key)
                    .to_string();

                results.push((clean_key, value));
            }

            cursor = new_cursor;
            if cursor == 0 {
                break;
            }
        }

        Ok(results)
    }

    async fn index_timeout(&self, key: &StateKey, expires_at_ms: i64) -> Result<(), StateError> {
        let canonical = key.canonical();
        let index_key = format!("{}:timeout_index", self.prefix);

        let mut conn = self.conn().await?;

        // ZADD timeout_index <score=expires_at_ms> <member=canonical_key>
        redis::cmd("ZADD")
            .arg(&index_key)
            .arg(expires_at_ms)
            .arg(&canonical)
            .query_async::<i64>(&mut conn)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(())
    }

    async fn remove_timeout_index(&self, key: &StateKey) -> Result<(), StateError> {
        let canonical = key.canonical();
        let index_key = format!("{}:timeout_index", self.prefix);

        let mut conn = self.conn().await?;

        // ZREM timeout_index <member=canonical_key>
        redis::cmd("ZREM")
            .arg(&index_key)
            .arg(&canonical)
            .query_async::<i64>(&mut conn)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(())
    }

    async fn get_expired_timeouts(&self, now_ms: i64) -> Result<Vec<String>, StateError> {
        let index_key = format!("{}:timeout_index", self.prefix);

        let mut conn = self.conn().await?;

        // ZRANGEBYSCORE timeout_index -inf <now_ms>
        // Returns all members with score <= now_ms (i.e., expired)
        let keys: Vec<String> = redis::cmd("ZRANGEBYSCORE")
            .arg(&index_key)
            .arg("-inf")
            .arg(now_ms)
            .query_async(&mut conn)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        // Strip the prefix from the returned keys
        let clean_keys: Vec<String> = keys
            .into_iter()
            .map(|k| {
                k.strip_prefix(&format!("{}:", self.prefix))
                    .unwrap_or(&k)
                    .to_string()
            })
            .collect();

        Ok(clean_keys)
    }
}

#[cfg(all(test, feature = "integration"))]
mod integration_tests {
    use super::*;
    use crate::config::RedisConfig;

    fn test_config() -> RedisConfig {
        RedisConfig {
            url: std::env::var("REDIS_URL")
                .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string()),
            prefix: format!("acteon-test-{}", uuid::Uuid::new_v4()),
            ..RedisConfig::default()
        }
    }

    #[tokio::test]
    async fn store_conformance() {
        let config = test_config();
        let store = RedisStateStore::new(&config).expect("pool creation should succeed");
        acteon_state::testing::run_store_conformance_tests(&store)
            .await
            .expect("conformance tests should pass");
    }
}
