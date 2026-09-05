use std::sync::Arc;
use std::time::Duration;

use acteon_core::Action;
use acteon_gateway::{CircuitBreakerConfig, CircuitBreakerRegistry, CircuitState, GroupManager};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use acteon_time::{Clock, ManualClock};

fn clock() -> Arc<ManualClock> {
    Arc::new(ManualClock::new(
        chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
    ))
}

#[tokio::test]
async fn circuit_recovery_and_probe_lease_use_the_shared_clock() {
    let clock = clock();
    let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
    let lock = Arc::new(MemoryDistributedLock::with_clock(clock.clone()));
    let mut registry = CircuitBreakerRegistry::new(store, lock).clock(clock.clone());
    registry.register(
        "effect",
        CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 1,
            recovery_timeout: Duration::from_secs(2),
            ..Default::default()
        },
    );
    let breaker = registry.get("effect").unwrap();
    breaker.record_failure().await;
    clock.advance_to(Duration::from_millis(1_999)).unwrap();
    assert_eq!(breaker.try_acquire_permit().await.0, CircuitState::Open);
    clock.advance_to(Duration::from_secs(2)).unwrap();
    assert_eq!(breaker.try_acquire_permit().await.0, CircuitState::HalfOpen);
    // An active probe excludes a second caller.
    assert_eq!(breaker.try_acquire_permit().await.0, CircuitState::Open);
    clock.advance_to(Duration::from_millis(31_999)).unwrap();
    assert_eq!(breaker.try_acquire_permit().await.0, CircuitState::Open);
    clock.advance_to(Duration::from_secs(32)).unwrap();
    assert_eq!(breaker.try_acquire_permit().await.0, CircuitState::HalfOpen);
    breaker.record_success().await;
    assert_eq!(breaker.state().await, CircuitState::Closed);
}

#[tokio::test]
async fn group_timestamps_and_flush_boundary_use_one_epoch() {
    let clock = clock();
    let store = MemoryStateStore::with_clock(clock.clone());
    let groups = GroupManager::with_clock(clock.clone());
    let action = Action::new("ns", "tenant", "effect", "alert", serde_json::json!({}));
    let (_, key, _, notify_at) = groups
        .add_to_group(&action, &[], 2, 5, None, 10, &store, None)
        .await
        .unwrap();
    let group = groups.get_group(&key).unwrap();
    assert_eq!(group.created_at, clock.now());
    assert_eq!(group.updated_at, clock.now());
    assert_eq!(group.events[0].received_at, clock.now());
    assert_eq!(notify_at, clock.now() + chrono::Duration::seconds(2));
    clock.advance_to(Duration::from_millis(1_999)).unwrap();
    assert!(groups.get_ready_groups().is_empty());
    clock.advance_to(Duration::from_secs(2)).unwrap();
    assert_eq!(groups.get_ready_groups().len(), 1);
    let flushed = groups.flush_group(&key).unwrap();
    assert_eq!(flushed.updated_at, clock.now());
}
