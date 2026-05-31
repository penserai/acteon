use std::time::Duration;

use crate::error::StateError;
use crate::key::{KeyKind, StateKey};
use crate::lock::DistributedLock;
use crate::store::{CasResult, StateStore};

fn test_key(kind: KeyKind, id: &str) -> StateKey {
    StateKey::new("test-ns", "test-tenant", kind, id)
}

/// Run the full state store conformance test suite.
///
/// Call this from your backend's test module with a fresh store instance.
///
/// # Errors
///
/// Returns an error if any conformance test fails.
pub async fn run_store_conformance_tests(store: &dyn StateStore) -> Result<(), StateError> {
    test_get_missing(store).await?;
    test_set_and_get(store).await?;
    test_check_and_set_new(store).await?;
    test_check_and_set_existing(store).await?;
    test_delete(store).await?;
    test_increment(store).await?;
    test_compare_and_swap(store).await?;
    test_ttl_set(store).await?;
    test_scan_by_kind_includes_check_and_set(store).await?;
    test_set_clears_ttl_on_overwrite(store).await?;
    test_timeout_index_reindex_replaces(store).await?;
    test_chain_ready_index_reindex_replaces(store).await?;
    Ok(())
}

/// A key written via `check_and_set` must be visible to `scan_keys_by_kind`.
/// On Redis, `check_and_set` stores a plain string while `set` stores a hash;
/// the scan historically issued `HGET` against every matched key and failed
/// the entire scan with `WRONGTYPE` as soon as one `check_and_set` key existed
/// (breaking bus/A2A listing). Memory and Postgres always handled this.
async fn test_scan_by_kind_includes_check_and_set(
    store: &dyn StateStore,
) -> Result<(), StateError> {
    let key = test_key(KeyKind::State, "scan-cas-marker");
    let created = store.check_and_set(&key, "scan-cas-value", None).await?;
    assert!(created, "check_and_set on a fresh key should create it");

    let results = store.scan_keys_by_kind(KeyKind::State).await?;
    assert!(
        results.iter().any(|(_, v)| v == "scan-cas-value"),
        "scan_keys_by_kind must surface a check_and_set-created key, got {results:?}"
    );
    Ok(())
}

/// Overwriting a key that had a TTL with a no-TTL `set` must make it permanent.
/// On Redis, `HSET` preserves the prior expiry, so the value would silently
/// vanish when the original TTL elapsed — diverging from memory/Postgres.
async fn test_set_clears_ttl_on_overwrite(store: &dyn StateStore) -> Result<(), StateError> {
    let key = test_key(KeyKind::State, "ttl-clear-on-overwrite");
    store
        .set(&key, "ephemeral", Some(Duration::from_secs(1)))
        .await?;
    // Overwrite without a TTL — this must clear the expiry set above.
    store.set(&key, "permanent", None).await?;
    // Wait past the original TTL; if it wasn't cleared the key is now gone.
    tokio::time::sleep(Duration::from_millis(1300)).await;
    let val = store.get(&key).await?;
    assert_eq!(
        val.as_deref(),
        Some("permanent"),
        "a no-TTL overwrite must clear the prior TTL (key expired)"
    );
    Ok(())
}

async fn test_get_missing(store: &dyn StateStore) -> Result<(), StateError> {
    let key = test_key(KeyKind::State, "missing");
    let val = store.get(&key).await?;
    assert!(val.is_none(), "get on missing key should return None");
    Ok(())
}

async fn test_set_and_get(store: &dyn StateStore) -> Result<(), StateError> {
    let key = test_key(KeyKind::State, "set-get");
    store.set(&key, "hello", None).await?;
    let val = store.get(&key).await?;
    assert_eq!(val.as_deref(), Some("hello"));
    Ok(())
}

async fn test_check_and_set_new(store: &dyn StateStore) -> Result<(), StateError> {
    let key = test_key(KeyKind::Dedup, "cas-new");
    let created = store.check_and_set(&key, "v1", None).await?;
    assert!(created, "check_and_set on new key should return true");
    let val = store.get(&key).await?;
    assert_eq!(val.as_deref(), Some("v1"));
    Ok(())
}

async fn test_check_and_set_existing(store: &dyn StateStore) -> Result<(), StateError> {
    let key = test_key(KeyKind::Dedup, "cas-existing");
    store.set(&key, "v1", None).await?;
    let created = store.check_and_set(&key, "v2", None).await?;
    assert!(
        !created,
        "check_and_set on existing key should return false"
    );
    let val = store.get(&key).await?;
    assert_eq!(val.as_deref(), Some("v1"), "original value should remain");
    Ok(())
}

async fn test_delete(store: &dyn StateStore) -> Result<(), StateError> {
    let key = test_key(KeyKind::State, "to-delete");
    store.set(&key, "bye", None).await?;
    let existed = store.delete(&key).await?;
    assert!(existed, "delete should return true for existing key");
    let val = store.get(&key).await?;
    assert!(val.is_none(), "get after delete should return None");

    let existed = store.delete(&key).await?;
    assert!(!existed, "delete on missing key should return false");
    Ok(())
}

async fn test_increment(store: &dyn StateStore) -> Result<(), StateError> {
    let key = test_key(KeyKind::Counter, "counter-1");
    let val = store.increment(&key, 1, None).await?;
    assert_eq!(val, 1, "first increment from zero should yield 1");

    let val = store.increment(&key, 5, None).await?;
    assert_eq!(val, 6, "second increment should accumulate");

    let val = store.increment(&key, -2, None).await?;
    assert_eq!(val, 4, "negative delta should decrement");
    Ok(())
}

async fn test_compare_and_swap(store: &dyn StateStore) -> Result<(), StateError> {
    let key = test_key(KeyKind::State, "cas-version");

    // Set initial value at version 0 (treated as "create")
    store.set(&key, "initial", None).await?;

    // CAS with wrong version should fail
    let result = store.compare_and_swap(&key, 999, "updated", None).await?;
    assert!(
        matches!(result, CasResult::Conflict { .. }),
        "CAS with wrong version should conflict"
    );

    // CAS with correct version should succeed
    let result = store.compare_and_swap(&key, 1, "updated", None).await?;
    assert_eq!(
        result,
        CasResult::Ok,
        "CAS with correct version should succeed"
    );

    let val = store.get(&key).await?;
    assert_eq!(val.as_deref(), Some("updated"));
    Ok(())
}

async fn test_ttl_set(store: &dyn StateStore) -> Result<(), StateError> {
    let key = test_key(KeyKind::State, "ttl-test");
    store
        .set(&key, "ephemeral", Some(Duration::from_secs(3600)))
        .await?;
    let val = store.get(&key).await?;
    assert_eq!(val.as_deref(), Some("ephemeral"));
    Ok(())
}

/// Re-indexing a timeout for a key must REPLACE its deadline, not leave a
/// stale duplicate in the old bucket. Redis (`ZADD`) and Postgres (`UPSERT`)
/// do this natively; memory and `DynamoDB` must too, or a re-scheduled
/// recurring/event-timeout fires off-schedule (and the stale entry never
/// drains).
async fn test_timeout_index_reindex_replaces(store: &dyn StateStore) -> Result<(), StateError> {
    let key = test_key(KeyKind::EventTimeout, "reindex");
    let canonical = key.canonical();

    store.index_timeout(&key, 1_000).await?;
    // Re-index forward WITHOUT removing first (the recurring/event re-arm path).
    store.index_timeout(&key, 5_000).await?;

    // The key moved forward: it must NOT still be due at the old time.
    let due_early = store.get_expired_timeouts(2_000).await?;
    assert!(
        !due_early.contains(&canonical),
        "re-indexed timeout key must not linger in its old, earlier bucket",
    );

    // At/after the new deadline it must appear EXACTLY ONCE.
    let due_late = store.get_expired_timeouts(6_000).await?;
    let count = due_late.iter().filter(|k| **k == canonical).count();
    assert_eq!(
        count, 1,
        "re-indexed timeout key must appear exactly once, not duplicated",
    );

    store.remove_timeout_index(&key).await?;
    Ok(())
}

/// Same contract as [`test_timeout_index_reindex_replaces`] for the
/// chain-ready index.
async fn test_chain_ready_index_reindex_replaces(store: &dyn StateStore) -> Result<(), StateError> {
    let key = test_key(KeyKind::PendingChains, "reindex");
    let canonical = key.canonical();

    store.index_chain_ready(&key, 1_000).await?;
    store.index_chain_ready(&key, 5_000).await?;

    let ready_early = store.get_ready_chains(2_000).await?;
    assert!(
        !ready_early.contains(&canonical),
        "re-indexed chain-ready key must not linger in its old, earlier bucket",
    );

    let ready_late = store.get_ready_chains(6_000).await?;
    let count = ready_late.iter().filter(|k| **k == canonical).count();
    assert_eq!(
        count, 1,
        "re-indexed chain-ready key must appear exactly once, not duplicated",
    );

    store.remove_chain_ready_index(&key).await?;
    Ok(())
}

/// Run the full distributed lock conformance test suite.
///
/// # Errors
///
/// Returns an error if any conformance test fails.
pub async fn run_lock_conformance_tests(lock: &dyn DistributedLock) -> Result<(), StateError> {
    test_try_acquire_and_release(lock).await?;
    test_try_acquire_contention(lock).await?;
    test_lock_extend(lock).await?;
    test_lock_is_held(lock).await?;
    Ok(())
}

async fn test_try_acquire_and_release(lock: &dyn DistributedLock) -> Result<(), StateError> {
    let guard = lock
        .try_acquire("test-lock-1", Duration::from_secs(10))
        .await?;
    assert!(guard.is_some(), "should acquire uncontested lock");
    let guard = guard.unwrap();
    guard.release().await?;
    Ok(())
}

async fn test_try_acquire_contention(lock: &dyn DistributedLock) -> Result<(), StateError> {
    let guard = lock
        .try_acquire("test-lock-2", Duration::from_secs(10))
        .await?;
    assert!(guard.is_some());
    let held = guard.unwrap();

    let second = lock
        .try_acquire("test-lock-2", Duration::from_secs(10))
        .await?;
    assert!(
        second.is_none(),
        "second acquire should fail while lock is held"
    );

    held.release().await?;
    Ok(())
}

async fn test_lock_extend(lock: &dyn DistributedLock) -> Result<(), StateError> {
    let guard = lock
        .try_acquire("test-lock-3", Duration::from_secs(5))
        .await?
        .expect("should acquire lock");

    guard.extend(Duration::from_secs(10)).await?;

    let held = guard.is_held().await?;
    assert!(held, "lock should still be held after extend");

    guard.release().await?;
    Ok(())
}

async fn test_lock_is_held(lock: &dyn DistributedLock) -> Result<(), StateError> {
    let guard = lock
        .try_acquire("test-lock-4", Duration::from_secs(10))
        .await?
        .expect("should acquire lock");

    assert!(guard.is_held().await?, "lock should be held");
    guard.release().await?;
    Ok(())
}
