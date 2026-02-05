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
