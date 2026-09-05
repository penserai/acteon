/// Lua script for atomic check-and-set (set-if-not-exists).
///
/// KEYS\[1\] = the plain string key
/// KEYS\[2\] = the hash key (`:h` suffix, used by `set`/`compare_and_swap`)
/// ARGV\[1\] = value to set
/// ARGV\[2\] = TTL in milliseconds (0 means no expiry)
///
/// Returns 1 if the key was newly set, 0 if it already existed in either form.
pub const CHECK_AND_SET: &str = r"
-- Check both plain string key and hash key for existence.
local str_exists = redis.call('EXISTS', KEYS[1])
local hash_exists = redis.call('EXISTS', KEYS[2])
if str_exists == 1 or hash_exists == 1 then
    return 0
end
redis.call('SET', KEYS[1], ARGV[1])
local ttl = tonumber(ARGV[2])
if ttl > 0 then
    redis.call('PEXPIRE', KEYS[1], ttl)
end
return 1
";

/// Lua script for atomic compare-and-swap using a hash with `v` (value) and
/// `ver` (version) fields.
///
/// KEYS\[1\] = the hash key
/// ARGV\[1\] = expected version
/// ARGV\[2\] = new value
/// ARGV\[3\] = TTL in milliseconds (0 means no expiry)
///
/// Returns a two-element array:
///   - `[1, new_ver]` on success
///   - `[0, cur_ver, cur_val]` on conflict
///   - `[1, 1]` if key does not exist and expected version is 0
pub const COMPARE_AND_SWAP: &str = r"
local exists = redis.call('EXISTS', KEYS[1])
local expected = tonumber(ARGV[1])
if exists == 0 then
    if expected ~= 0 then
        return {0, 0, false}
    end
    redis.call('HSET', KEYS[1], 'v', ARGV[2], 'ver', 1)
    local ttl = tonumber(ARGV[3])
    if ttl > 0 then
        redis.call('PEXPIRE', KEYS[1], ttl)
    end
    return {1, 1}
end
local cur_ver = tonumber(redis.call('HGET', KEYS[1], 'ver'))
if cur_ver ~= expected then
    local cur_val = redis.call('HGET', KEYS[1], 'v')
    return {0, cur_ver, cur_val}
end
local new_ver = cur_ver + 1
redis.call('HSET', KEYS[1], 'v', ARGV[2], 'ver', new_ver)
local ttl = tonumber(ARGV[3])
if ttl > 0 then
    redis.call('PEXPIRE', KEYS[1], ttl)
else
    redis.call('PERSIST', KEYS[1])
end
return {1, new_ver}
";

/// Lua script for acquiring a distributed lock (SET NX PX).
///
/// KEYS\[1\] = lock key
/// ARGV\[1\] = owner token
/// ARGV\[2\] = TTL in milliseconds
///
/// Returns 1 if acquired, 0 otherwise.
pub const LOCK_ACQUIRE: &str = r"
local ok = redis.call('SET', KEYS[1], ARGV[1], 'NX', 'PX', ARGV[2])
if ok then
    return 1
end
return 0
";

/// Lua script for releasing a distributed lock.
///
/// KEYS\[1\] = lock key
/// ARGV\[1\] = owner token
///
/// Returns 1 if released, 0 if not held by this owner.
pub const LOCK_RELEASE: &str = r"
local owner = redis.call('GET', KEYS[1])
if owner == ARGV[1] then
    redis.call('DEL', KEYS[1])
    return 1
end
return 0
";

/// Lua script for extending a distributed lock's TTL.
///
/// KEYS\[1\] = lock key
/// ARGV\[1\] = owner token
/// ARGV\[2\] = new TTL in milliseconds
///
/// Returns 1 if extended, 0 if not held by this owner.
pub const LOCK_EXTEND: &str = r"
local owner = redis.call('GET', KEYS[1])
if owner == ARGV[1] then
    redis.call('PEXPIRE', KEYS[1], ARGV[2])
    return 1
end
return 0
";
/// Increment either storage representation without losing integer precision in Lua.
/// KEYS are the string key and hash key; ARGV are delta and TTL ms (-1 preserves TTL).
pub const INCREMENT: &str = r"
local key = KEYS[1]
local value
if redis.call('EXISTS', KEYS[2]) == 1 then
    key = KEYS[2]
    redis.call('HINCRBY', key, 'v', ARGV[1])
    redis.call('HINCRBY', key, 'ver', 1)
    value = redis.call('HGET', key, 'v')
else
    redis.call('INCRBY', key, ARGV[1])
    value = redis.call('GET', key)
end
local ttl = tonumber(ARGV[2])
if ttl >= 0 then
    redis.call('PEXPIRE', key, ttl)
end
return value
";
