use std::time::Duration;

use async_trait::async_trait;
use deadpool_redis::{Config, Pool, Runtime};
use redis::{AsyncCommands, Script};

use acteon_state::error::StateError;
use acteon_state::key::{KeyKind, StateKey};
use acteon_state::store::{CasResult, StateScanPage, StateStore};

use crate::config::RedisConfig;
use crate::key_render::render_key;
use crate::scripts;

/// Redis-backed implementation of [`StateStore`].
///
/// Uses a `deadpool-redis` connection pool and Lua scripts for atomicity.
/// Values are stored as Redis hashes with fields `v` (value) and `ver`
/// (version). Reads atomically promote legacy strings, including counters,
/// preserving their remaining expiry and removing shadow values.
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
        let effective_url = config.effective_url();
        let cfg = Config::from_url(&effective_url);
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

    fn ordered_index_key(&self, key: &StateKey) -> String {
        format!(
            "{}:__state_ordered__:{}:{}:{}",
            self.prefix, key.namespace, key.tenant, key.kind
        )
    }

    fn ordered_index_marker(&self, namespace: &str, tenant: &str, kind: &KeyKind) -> String {
        format!(
            "{}:__state_ordered_complete__:{namespace}:{tenant}:{kind}",
            self.prefix
        )
    }

    fn ordered_index_for_scope(&self, namespace: &str, tenant: &str, kind: &KeyKind) -> String {
        format!(
            "{}:__state_ordered__:{namespace}:{tenant}:{kind}",
            self.prefix
        )
    }

    fn ordered_kind_index_key(&self, key: &StateKey) -> String {
        self.ordered_kind_index(&key.kind)
    }

    fn ordered_kind_index(&self, kind: &KeyKind) -> String {
        format!("{}:__state_ordered_kind__:{kind}", self.prefix)
    }

    fn ordered_kind_index_marker(&self, kind: &KeyKind) -> String {
        format!("{}:__state_ordered_kind_complete__:{kind}", self.prefix)
    }

    /// Obtain a connection from the pool.
    async fn conn(&self) -> Result<deadpool_redis::Connection, StateError> {
        self.pool
            .get()
            .await
            .map_err(|e| StateError::Connection(e.to_string()))
    }

    async fn ensure_ordered_index(
        &self,
        conn: &mut deadpool_redis::Connection,
        namespace: &str,
        tenant: &str,
        kind: &KeyKind,
    ) -> Result<(), StateError> {
        let marker = self.ordered_index_marker(namespace, tenant, kind);
        let index = self.ordered_index_for_scope(namespace, tenant, kind);
        let (complete, index_exists): (bool, bool) = redis::pipe()
            .exists(&marker)
            .exists(&index)
            .query_async(&mut **conn)
            .await
            .map_err(|error| StateError::Backend(error.to_string()))?;
        if complete && index_exists {
            return Ok(());
        }
        let pattern = format!("{}:{namespace}:{tenant}:{kind}:*", self.prefix);
        let physical_prefix = format!("{}:", self.prefix);
        let mut cursor = 0_u64;
        loop {
            let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(500)
                .query_async(&mut **conn)
                .await
                .map_err(|error| StateError::Backend(error.to_string()))?;
            if !keys.is_empty() {
                let mut pipe = redis::pipe();
                for key in keys {
                    let stripped = key.strip_prefix(&physical_prefix).unwrap_or(&key);
                    let canonical = stripped.strip_suffix(":h").unwrap_or(stripped);
                    pipe.zadd(&index, canonical, 0).ignore();
                }
                pipe.query_async::<()>(&mut **conn)
                    .await
                    .map_err(|error| StateError::Backend(error.to_string()))?;
            }
            cursor = next;
            if cursor == 0 {
                break;
            }
        }
        redis::pipe()
            .atomic()
            .zadd(&index, "__acteon_state_index_sentinel__", 0)
            .ignore()
            .set(marker, "1")
            .ignore()
            .query_async::<()>(&mut **conn)
            .await
            .map_err(|error| StateError::Backend(error.to_string()))?;
        Ok(())
    }

    async fn ensure_ordered_kind_index(
        &self,
        conn: &mut deadpool_redis::Connection,
        kind: &KeyKind,
    ) -> Result<(), StateError> {
        let marker = self.ordered_kind_index_marker(kind);
        let index = self.ordered_kind_index(kind);
        let (complete, index_exists): (bool, bool) = redis::pipe()
            .exists(&marker)
            .exists(&index)
            .query_async(&mut **conn)
            .await
            .map_err(|error| StateError::Backend(error.to_string()))?;
        if complete && index_exists {
            return Ok(());
        }
        let pattern = format!("{}:*:*:{kind}:*", self.prefix);
        let physical_prefix = format!("{}:", self.prefix);
        let mut cursor = 0_u64;
        loop {
            let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(500)
                .query_async(&mut **conn)
                .await
                .map_err(|error| StateError::Backend(error.to_string()))?;
            if !keys.is_empty() {
                let mut pipe = redis::pipe();
                for key in keys {
                    let stripped = key.strip_prefix(&physical_prefix).unwrap_or(&key);
                    let canonical = stripped.strip_suffix(":h").unwrap_or(stripped);
                    pipe.zadd(&index, canonical, 0).ignore();
                }
                pipe.query_async::<()>(&mut **conn)
                    .await
                    .map_err(|error| StateError::Backend(error.to_string()))?;
            }
            cursor = next;
            if cursor == 0 {
                break;
            }
        }
        redis::pipe()
            .atomic()
            .zadd(&index, "__acteon_state_index_sentinel__", 0)
            .ignore()
            .set(marker, "1")
            .ignore()
            .query_async::<()>(&mut **conn)
            .await
            .map_err(|error| StateError::Backend(error.to_string()))?;
        Ok(())
    }

    async fn indexed_value(
        &self,
        conn: &mut deadpool_redis::Connection,
        canonical: &str,
    ) -> Result<Option<String>, StateError> {
        let physical = format!("{}:{canonical}", self.prefix);
        let hash_value: Option<String> = conn
            .hget(format!("{physical}:h"), "v")
            .await
            .map_err(|error| StateError::Backend(error.to_string()))?;
        if hash_value.is_some() {
            return Ok(hash_value);
        }
        conn.get(physical)
            .await
            .map_err(|error| StateError::Backend(error.to_string()))
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
            .key(self.ordered_index_key(key))
            .key(self.ordered_kind_index_key(key))
            .arg(value)
            .arg(ttl_ms)
            .arg(key.canonical())
            .invoke_async(&mut conn)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(result == 1)
    }

    async fn get(&self, key: &StateKey) -> Result<Option<String>, StateError> {
        self.get_versioned(key)
            .await
            .map(|entry| entry.map(|(value, _)| value))
    }

    async fn get_versioned(&self, key: &StateKey) -> Result<Option<(String, u64)>, StateError> {
        let mut conn = self.conn().await?;
        let (value, version): (Option<String>, u64) = Script::new(scripts::GET_VERSIONED)
            .key(self.hash_key(key))
            .key(self.string_key(key))
            .key(self.ordered_index_key(key))
            .key(self.ordered_kind_index_key(key))
            .arg(key.canonical())
            .invoke_async(&mut conn)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;
        Ok(value.map(|value| (value, version)))
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
            .atomic()
            .hset(&redis_key, "v", value)
            .ignore()
            .hset(&redis_key, "ver", new_ver)
            .ignore()
            .zadd(self.ordered_index_key(key), key.canonical(), 0)
            .ignore()
            .zadd(self.ordered_kind_index_key(key), key.canonical(), 0)
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
        } else {
            // Clear any pre-existing expiry: `HSET` preserves the old TTL, so
            // without this a no-TTL overwrite of a key first written with a TTL
            // would silently expire — diverging from the memory and Postgres
            // backends, which make the overwrite permanent.
            let () = conn
                .persist(&redis_key)
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
            .atomic()
            .del(&hash_key)
            .del(&string_key)
            .zrem(self.ordered_index_key(key), key.canonical())
            .zrem(self.ordered_kind_index_key(key), key.canonical())
            .query_async(&mut conn)
            .await
            .map(|(hash, string, _, _): (i64, i64, i64, i64)| hash + string)
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(deleted > 0)
    }

    async fn compare_and_delete(
        &self,
        key: &StateKey,
        expected_version: u64,
    ) -> Result<bool, StateError> {
        let mut conn = self.conn().await?;
        let deleted: i64 = Script::new(scripts::COMPARE_AND_DELETE)
            .key(self.hash_key(key))
            .key(self.string_key(key))
            .key(self.ordered_index_key(key))
            .key(self.ordered_kind_index_key(key))
            .arg(expected_version)
            .arg(key.canonical())
            .invoke_async(&mut conn)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;
        Ok(deleted == 1)
    }

    async fn increment(
        &self,
        key: &StateKey,
        delta: i64,
        ttl: Option<Duration>,
    ) -> Result<i64, StateError> {
        let mut conn = self.conn().await?;
        // Respect values created by set/CAS as well as legacy string counters.
        // The increment and its TTL update are one atomic operation.
        let ttl_ms = ttl.map_or(-1, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
        let value: String = Script::new(scripts::INCREMENT)
            .key(self.string_key(key))
            .key(self.hash_key(key))
            .key(self.ordered_index_key(key))
            .key(self.ordered_kind_index_key(key))
            .arg(delta)
            .arg(ttl_ms)
            .arg(key.canonical())
            .invoke_async(&mut conn)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;
        value
            .parse()
            .map_err(|e| StateError::Serialization(format!("invalid counter result: {e}")))
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
            .key(self.string_key(key))
            .key(self.ordered_index_key(key))
            .key(self.ordered_kind_index_key(key))
            .arg(expected_version)
            .arg(new_value)
            .arg(ttl_ms)
            .arg(key.canonical())
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
                // Dispatch on the key shape: versioned hashes use suffix
                // `:h`; legacy entries/counters may be strings. Issuing HGET
                // against a string key fails the WHOLE scan with WRONGTYPE, so
                // pick the matching read per key.
                let value = if key.ends_with(":h") {
                    let v: Option<String> = conn
                        .hget(&key, "v")
                        .await
                        .map_err(|e| StateError::Backend(e.to_string()))?;
                    match v {
                        Some(s) => s,
                        None => continue,
                    }
                } else {
                    let v: Option<String> = conn
                        .get(&key)
                        .await
                        .map_err(|e| StateError::Backend(e.to_string()))?;
                    match v {
                        Some(s) => s,
                        None => continue,
                    }
                };

                // Strip the prefix to return a clean key
                let stripped = key
                    .strip_prefix(&format!("{}:", self.prefix))
                    .unwrap_or(&key);
                let clean_key = stripped.strip_suffix(":h").unwrap_or(stripped).to_string();

                results.push((clean_key, value));
            }

            cursor = new_cursor;
            if cursor == 0 {
                break;
            }
        }

        Ok(results)
    }

    async fn scan_keys_page(
        &self,
        namespace: &str,
        tenant: &str,
        kind: KeyKind,
        prefix: Option<&str>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<StateScanPage, StateError> {
        if !(1..=1_000).contains(&limit) {
            return Err(StateError::Backend("invalid state scan page limit".into()));
        }
        let canonical_prefix = match prefix {
            Some(prefix) => format!("{namespace}:{tenant}:{kind}:{prefix}"),
            None => format!("{namespace}:{tenant}:{kind}:"),
        };
        if after.is_some_and(|cursor| !cursor.starts_with(&canonical_prefix)) {
            return Err(StateError::Backend("invalid state scan page cursor".into()));
        }
        let index = self.ordered_index_for_scope(namespace, tenant, &kind);
        let mut conn = self.conn().await?;
        self.ensure_ordered_index(&mut conn, namespace, tenant, &kind)
            .await?;

        let mut entries = Vec::new();
        let mut cursor = after.map(str::to_owned);
        while entries.len() <= limit {
            let minimum = cursor.as_ref().map_or_else(
                || format!("[{canonical_prefix}"),
                |value| format!("({value}"),
            );
            let batch = (limit + 1 - entries.len()).max(64);
            let members: Vec<String> = redis::cmd("ZRANGEBYLEX")
                .arg(&index)
                .arg(minimum)
                .arg("+")
                .arg("LIMIT")
                .arg(0)
                .arg(batch)
                .query_async(&mut conn)
                .await
                .map_err(|error| StateError::Backend(error.to_string()))?;
            if members.is_empty() {
                break;
            }
            let member_count = members.len();
            let mut left_prefix = false;
            for member in members {
                cursor = Some(member.clone());
                if !member.starts_with(&canonical_prefix) {
                    left_prefix = true;
                    break;
                }
                if let Some(value) = self.indexed_value(&mut conn, &member).await? {
                    entries.push((member, value));
                    if entries.len() > limit {
                        break;
                    }
                } else {
                    let _: usize = conn
                        .zrem(&index, &member)
                        .await
                        .map_err(|error| StateError::Backend(error.to_string()))?;
                }
            }
            if left_prefix || entries.len() > limit || member_count < batch {
                break;
            }
        }
        let has_more = entries.len() > limit;
        entries.truncate(limit);
        let next_cursor = has_more.then(|| entries[entries.len() - 1].0.clone());
        Ok(StateScanPage {
            entries,
            next_cursor,
        })
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
                // Dispatch on the key shape: versioned hashes use suffix
                // `:h`; legacy entries/counters may be strings. Issuing HGET
                // against a string key fails the WHOLE scan with WRONGTYPE, so
                // pick the matching read per key.
                let value = if key.ends_with(":h") {
                    let v: Option<String> = conn
                        .hget(&key, "v")
                        .await
                        .map_err(|e| StateError::Backend(e.to_string()))?;
                    match v {
                        Some(s) => s,
                        None => continue,
                    }
                } else {
                    let v: Option<String> = conn
                        .get(&key)
                        .await
                        .map_err(|e| StateError::Backend(e.to_string()))?;
                    match v {
                        Some(s) => s,
                        None => continue,
                    }
                };

                // Strip the prefix to return a clean key
                let stripped = key
                    .strip_prefix(&format!("{}:", self.prefix))
                    .unwrap_or(&key);
                let clean_key = stripped.strip_suffix(":h").unwrap_or(stripped).to_string();

                results.push((clean_key, value));
            }

            cursor = new_cursor;
            if cursor == 0 {
                break;
            }
        }

        Ok(results)
    }

    async fn scan_keys_by_kind_page(
        &self,
        kind: KeyKind,
        after: Option<&str>,
        limit: usize,
    ) -> Result<StateScanPage, StateError> {
        if !(1..=1_000).contains(&limit) {
            return Err(StateError::Backend("invalid state scan page limit".into()));
        }
        let marker = format!(":{kind}:");
        if after.is_some_and(|cursor| !cursor.contains(&marker)) {
            return Err(StateError::Backend("invalid state scan page cursor".into()));
        }
        let index = self.ordered_kind_index(&kind);
        let mut conn = self.conn().await?;
        self.ensure_ordered_kind_index(&mut conn, &kind).await?;
        let mut entries = Vec::new();
        let mut cursor = after.map(str::to_owned);
        while entries.len() <= limit {
            let minimum = cursor
                .as_ref()
                .map_or_else(|| "-".to_owned(), |value| format!("({value}"));
            let batch = (limit + 1 - entries.len()).max(64);
            let members: Vec<String> = redis::cmd("ZRANGEBYLEX")
                .arg(&index)
                .arg(minimum)
                .arg("+")
                .arg("LIMIT")
                .arg(0)
                .arg(batch)
                .query_async(&mut conn)
                .await
                .map_err(|error| StateError::Backend(error.to_string()))?;
            if members.is_empty() {
                break;
            }
            let member_count = members.len();
            for member in members {
                cursor = Some(member.clone());
                if member == "__acteon_state_index_sentinel__" {
                    continue;
                }
                if let Some(value) = self.indexed_value(&mut conn, &member).await? {
                    entries.push((member, value));
                    if entries.len() > limit {
                        break;
                    }
                } else {
                    let _: usize = conn
                        .zrem(&index, &member)
                        .await
                        .map_err(|error| StateError::Backend(error.to_string()))?;
                }
            }
            if entries.len() > limit || member_count < batch {
                break;
            }
        }
        let has_more = entries.len() > limit;
        entries.truncate(limit);
        let next_cursor = has_more.then(|| entries[entries.len() - 1].0.clone());
        Ok(StateScanPage {
            entries,
            next_cursor,
        })
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

    async fn index_chain_ready(&self, key: &StateKey, ready_at_ms: i64) -> Result<(), StateError> {
        let canonical = key.canonical();
        let index_key = format!("{}:chain_ready_index", self.prefix);

        let mut conn = self.conn().await?;

        redis::cmd("ZADD")
            .arg(&index_key)
            .arg(ready_at_ms)
            .arg(&canonical)
            .query_async::<i64>(&mut conn)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(())
    }

    async fn remove_chain_ready_index(&self, key: &StateKey) -> Result<(), StateError> {
        let canonical = key.canonical();
        let index_key = format!("{}:chain_ready_index", self.prefix);

        let mut conn = self.conn().await?;

        redis::cmd("ZREM")
            .arg(&index_key)
            .arg(&canonical)
            .query_async::<i64>(&mut conn)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(())
    }

    async fn get_ready_chains(&self, now_ms: i64) -> Result<Vec<String>, StateError> {
        let index_key = format!("{}:chain_ready_index", self.prefix);

        let mut conn = self.conn().await?;

        let keys: Vec<String> = redis::cmd("ZRANGEBYSCORE")
            .arg(&index_key)
            .arg("-inf")
            .arg(now_ms)
            .query_async(&mut conn)
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(keys)
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

    #[tokio::test]
    async fn ordered_pages_backfill_legacy_rows_and_track_mutations() {
        let store = RedisStateStore::new(&test_config()).unwrap();
        let keys = ["c", "a", "b"].map(|id| {
            StateKey::new(
                "page-ns",
                "page-tenant",
                KeyKind::Custom("journal".into()),
                id,
            )
        });
        let mut conn = store.conn().await.unwrap();
        for key in &keys {
            let _: () = conn
                .set(store.string_key(key), key.id.as_str())
                .await
                .unwrap();
        }
        let first = store
            .scan_keys_page(
                "page-ns",
                "page-tenant",
                KeyKind::Custom("journal".into()),
                None,
                None,
                2,
            )
            .await
            .unwrap();
        assert_eq!(
            first
                .entries
                .iter()
                .map(|(_, value)| value.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert!(first.next_cursor.is_some());
        let second = store
            .scan_keys_page(
                "page-ns",
                "page-tenant",
                KeyKind::Custom("journal".into()),
                None,
                first.next_cursor.as_deref(),
                2,
            )
            .await
            .unwrap();
        assert_eq!(second.entries[0].1, "c");
        assert!(second.next_cursor.is_none());

        let added = StateKey::new(
            "page-ns",
            "page-tenant",
            KeyKind::Custom("journal".into()),
            "d",
        );
        store.set(&added, "d", None).await.unwrap();
        store.delete(&keys[0]).await.unwrap();
        let page = store
            .scan_keys_page(
                "page-ns",
                "page-tenant",
                KeyKind::Custom("journal".into()),
                None,
                None,
                10,
            )
            .await
            .unwrap();
        assert_eq!(
            page.entries
                .iter()
                .map(|(_, value)| value.as_str())
                .collect::<Vec<_>>(),
            ["a", "b", "d"]
        );
    }

    #[tokio::test]
    async fn conditional_delete_preserves_conflicting_legacy_values_and_removes_shadows() {
        let store = RedisStateStore::new(&test_config()).unwrap();
        let key = StateKey::new("ns", "tenant", KeyKind::Chain, "delete-legacy");
        let mut conn = store.conn().await.unwrap();
        let _: () = conn
            .set_ex(store.string_key(&key), "legacy", 30)
            .await
            .unwrap();
        let before: i64 = conn.pttl(store.string_key(&key)).await.unwrap();
        assert!(!store.compare_and_delete(&key, 2).await.unwrap());
        let after: i64 = conn.pttl(store.string_key(&key)).await.unwrap();
        assert!(after > 0 && after <= before);
        assert_eq!(
            conn.get::<_, String>(store.string_key(&key)).await.unwrap(),
            "legacy"
        );
        assert!(store.compare_and_delete(&key, 1).await.unwrap());
        assert!(store.get(&key).await.unwrap().is_none());

        store.set(&key, "current", None).await.unwrap();
        let _: () = conn.set(store.string_key(&key), "shadow").await.unwrap();
        assert!(store.compare_and_delete(&key, 1).await.unwrap());
        assert!(!conn.exists::<_, bool>(store.hash_key(&key)).await.unwrap());
        assert!(
            !conn
                .exists::<_, bool>(store.string_key(&key))
                .await
                .unwrap()
        );
        assert!(!store.compare_and_delete(&key, 1).await.unwrap());
    }

    #[tokio::test]
    async fn legacy_read_promotes_value_and_preserves_remaining_expiry() {
        let store = RedisStateStore::new(&test_config()).unwrap();
        let key = StateKey::new("ns", "tenant", KeyKind::Dedup, "legacy");
        let mut conn = store.conn().await.unwrap();
        let _: () = conn
            .set_ex(store.string_key(&key), "original", 30)
            .await
            .unwrap();
        let before: i64 = conn.pttl(store.string_key(&key)).await.unwrap();

        assert_eq!(
            store.get_versioned(&key).await.unwrap(),
            Some(("original".into(), 1))
        );
        let after: i64 = conn.pttl(store.hash_key(&key)).await.unwrap();
        assert!(after > 0 && after <= before);
        assert!(
            !conn
                .exists::<_, bool>(store.string_key(&key))
                .await
                .unwrap()
        );
        assert_eq!(
            store
                .compare_and_swap(&key, 1, "replacement", None)
                .await
                .unwrap(),
            CasResult::Ok
        );
        assert_eq!(conn.pttl::<_, i64>(store.hash_key(&key)).await.unwrap(), -1);
        store.delete(&key).await.unwrap();
    }

    #[tokio::test]
    async fn legacy_cas_conflict_preserves_value_and_success_removes_shadow() {
        let store = RedisStateStore::new(&test_config()).unwrap();
        let key = StateKey::new("ns", "tenant", KeyKind::Dedup, "legacy-cas");
        let mut conn = store.conn().await.unwrap();
        let _: () = conn
            .set_ex(store.string_key(&key), "original", 30)
            .await
            .unwrap();
        assert!(matches!(
            store
                .compare_and_swap(&key, 0, "stale", None)
                .await
                .unwrap(),
            CasResult::Conflict { .. }
        ));
        assert_eq!(
            conn.get::<_, String>(store.string_key(&key)).await.unwrap(),
            "original"
        );
        assert!(conn.pttl::<_, i64>(store.string_key(&key)).await.unwrap() > 0);
        assert_eq!(
            store
                .compare_and_swap(&key, 1, "replacement", None)
                .await
                .unwrap(),
            CasResult::Ok
        );
        assert!(
            !conn
                .exists::<_, bool>(store.string_key(&key))
                .await
                .unwrap()
        );
        assert_eq!(
            store.get_versioned(&key).await.unwrap(),
            Some(("replacement".into(), 2))
        );
        store.delete(&key).await.unwrap();
        assert!(matches!(
            store
                .compare_and_swap(&key, 0, "resurrected", None)
                .await
                .unwrap(),
            CasResult::Conflict { .. }
        ));
    }

    #[tokio::test]
    async fn legacy_shadow_cannot_reappear_after_hash_expiry() {
        let store = RedisStateStore::new(&test_config()).unwrap();
        let key = StateKey::new("ns", "tenant", KeyKind::Dedup, "shadow");
        let mut conn = store.conn().await.unwrap();
        store
            .set(&key, "terminal", Some(Duration::from_secs(1)))
            .await
            .unwrap();
        // Reproduce the dual representation left by older CAS implementations.
        let _: () = conn.set(store.string_key(&key), "original").await.unwrap();
        assert_eq!(store.get(&key).await.unwrap().as_deref(), Some("terminal"));
        assert!(
            !conn
                .exists::<_, bool>(store.string_key(&key))
                .await
                .unwrap()
        );
        tokio::time::sleep(Duration::from_millis(1300)).await;
        assert_eq!(store.get(&key).await.unwrap(), None);
    }
}
