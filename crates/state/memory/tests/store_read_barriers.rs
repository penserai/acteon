//! Deterministic read cuts against the same backend-agnostic fault adapter as writes.
use acteon_state::{
    KeyKind, StateKey, StateStore,
    testing::faults::{FaultStore, FaultTiming, ReadOperation, WriteOperation},
};
use acteon_state_memory::MemoryStateStore;
use std::{sync::Arc, time::Duration};

#[tokio::test]
async fn read_barriers_distinguish_before_and_after_backend_observation() {
    for operation in [ReadOperation::Get, ReadOperation::GetVersioned] {
        for timing in [FaultTiming::Before, FaultTiming::After] {
            let store = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
            let kind = KeyKind::Custom("read_barrier".into());
            let key = StateKey::new("test", "isolated", kind.clone(), "record");
            store.set(&key, "original", None).await.unwrap();
            let (reached, resume) = store.pause_next_read(kind, operation, timing).unwrap();
            let reading_store = store.clone();
            let reading_key = key.clone();
            let reading = tokio::spawn(async move {
                match operation {
                    ReadOperation::Get => reading_store.get(&reading_key).await,
                    ReadOperation::GetVersioned => reading_store
                        .get_versioned(&reading_key)
                        .await
                        .map(|value| value.map(|(raw, _)| raw)),
                }
            });
            tokio::time::timeout(Duration::from_secs(5), reached)
                .await
                .unwrap()
                .unwrap();
            store.set(&key, "newer", None).await.unwrap();
            resume.send(()).unwrap();
            let observed = tokio::time::timeout(Duration::from_secs(5), reading)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let expected = if timing == FaultTiming::Before {
                "newer"
            } else {
                "original"
            };
            assert_eq!(observed.as_deref(), Some(expected));
            assert_eq!(store.get(&key).await.unwrap().as_deref(), Some("newer"));
            assert_eq!(store.consumed(), 1);
        }
    }
}

#[tokio::test]
async fn a_read_pause_is_one_shot_and_its_dropped_controller_fails_closed() {
    let store = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
    let kind = KeyKind::Custom("read_barrier".into());
    let key = StateKey::new("test", "isolated", kind.clone(), "record");
    let (reached, resume) = store
        .pause_next_read(kind.clone(), ReadOperation::Get, FaultTiming::Before)
        .unwrap();
    assert!(
        store
            .fail_next(kind, WriteOperation::Set, FaultTiming::Before)
            .is_err()
    );
    let unrelated = StateKey::new(
        "test",
        "isolated",
        KeyKind::Custom("unrelated".into()),
        "record",
    );
    assert!(store.get(&unrelated).await.unwrap().is_none());
    assert_eq!(store.consumed(), 0);
    let reading_store = store.clone();
    let reading_key = key.clone();
    let reading = tokio::spawn(async move { reading_store.get(&reading_key).await });
    tokio::time::timeout(Duration::from_secs(5), reached)
        .await
        .unwrap()
        .unwrap();
    drop(resume);
    assert!(
        tokio::time::timeout(Duration::from_secs(5), reading)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert_eq!(store.consumed(), 1);
    assert!(store.get(&key).await.unwrap().is_none());
}
