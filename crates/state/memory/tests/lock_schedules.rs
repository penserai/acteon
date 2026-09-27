//! Bounded API-order exploration, not a model of `DashMap`'s internal scheduling.

use std::sync::Arc;
use std::time::Duration;

use acteon_state::error::StateError;
use acteon_state::lock::DistributedLock;
use acteon_state_memory::MemoryDistributedLock;
use acteon_time::ManualClock;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Event {
    Renew,
    Expire,
    Release,
    Acquire,
    Sweep,
}

fn permutations(events: &mut [Event], start: usize, output: &mut Vec<Vec<Event>>) {
    if start == events.len() {
        output.push(events.to_vec());
    } else {
        for index in start..events.len() {
            events.swap(start, index);
            permutations(events, start + 1, output);
            events.swap(start, index);
        }
    }
}

#[tokio::test]
async fn all_lease_boundary_event_orders_match_reference() {
    let mut schedules = Vec::new();
    permutations(
        &mut [
            Event::Renew,
            Event::Expire,
            Event::Release,
            Event::Acquire,
            Event::Sweep,
        ],
        0,
        &mut schedules,
    );
    assert_eq!(schedules.len(), 120);
    assert_eq!(
        schedules
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        120
    );
    for schedule in schedules {
        let clock = Arc::new(ManualClock::new(chrono::DateTime::UNIX_EPOCH));
        let lock = MemoryDistributedLock::with_clock(clock.clone());
        let mut first = lock
            .try_acquire("lease", Duration::from_secs(10))
            .await
            .unwrap();
        let mut second = None;
        // Independent single-key reference: owner 1 is the original guard,
        // owner 2 is the contender. Expired entries remain until removed.
        let mut entry = Some((1, 10));
        let mut now = 0;
        for (step, event) in schedule.iter().enumerate() {
            let context = format!("schedule={schedule:?}, step={step}");
            match event {
                Event::Renew => {
                    if let Some(guard) = &first {
                        let live = entry.is_some_and(|(owner, until)| owner == 1 && now < until);
                        let result = guard.extend(Duration::from_secs(20)).await;
                        if live {
                            result.unwrap();
                            entry = Some((1, now + 20));
                        } else {
                            assert!(
                                matches!(result, Err(StateError::LockExpired(_))),
                                "{context}"
                            );
                        }
                    }
                }
                Event::Expire => {
                    now = 10;
                    clock.advance_to(Duration::from_secs(now)).unwrap();
                }
                Event::Release => {
                    first.take().unwrap().release().await.unwrap();
                    if entry.is_some_and(|(owner, _)| owner == 1) {
                        entry = None;
                    }
                }
                Event::Acquire => {
                    let available = entry.is_none_or(|(_, until)| now >= until);
                    second = lock
                        .try_acquire("lease", Duration::from_secs(30))
                        .await
                        .unwrap();
                    assert_eq!(second.is_some(), available, "{context}");
                    if available {
                        entry = Some((2, now + 30));
                    }
                }
                Event::Sweep => {
                    let expired = entry.is_some_and(|(_, until)| now >= until);
                    assert_eq!(lock.sweep_expired(), usize::from(expired), "{context}");
                    if expired {
                        entry = None;
                    }
                }
            }
            for (owner, guard) in [(1, &first), (2, &second)] {
                if let Some(guard) = guard {
                    assert_eq!(
                        guard.is_held().await.unwrap(),
                        entry.is_some_and(|(current, until)| current == owner && now < until),
                        "{context}, owner={owner}"
                    );
                }
            }
        }
        if let Some(guard) = second {
            guard.release().await.unwrap();
        }
        // Neither stale release nor sweep may leave an unobservable live lease.
        assert!(
            lock.try_acquire("lease", Duration::from_secs(1))
                .await
                .unwrap()
                .is_some()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_release_racing_replacement_renewal_preserves_new_owner() {
    for trial in 0..64 {
        let clock = Arc::new(ManualClock::new(chrono::DateTime::UNIX_EPOCH));
        let lock = MemoryDistributedLock::with_clock(clock.clone());
        let stale = lock
            .try_acquire("lease", Duration::from_secs(10))
            .await
            .unwrap()
            .unwrap();
        clock.advance_to(Duration::from_secs(10)).unwrap();
        let current = lock
            .try_acquire("lease", Duration::from_secs(10))
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            stale.extend(Duration::from_secs(100)).await,
            Err(StateError::LockExpired(_))
        ));
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let release_barrier = barrier.clone();
        let release = tokio::spawn(async move {
            release_barrier.wait().await;
            stale.release().await.unwrap();
        });
        let renewal = tokio::spawn(async move {
            barrier.wait().await;
            current.extend(Duration::from_secs(30)).await.unwrap();
            current
        });
        release.await.unwrap();
        let current = renewal.await.unwrap();
        clock.advance_to(Duration::from_secs(20)).unwrap();
        assert_eq!(lock.sweep_expired(), 0, "trial={trial}");
        assert!(current.is_held().await.unwrap(), "trial={trial}");
        assert!(
            lock.try_acquire("lease", Duration::from_secs(1))
                .await
                .unwrap()
                .is_none(),
            "trial={trial}"
        );
        clock.advance_to(Duration::from_secs(40)).unwrap();
        assert!(!current.is_held().await.unwrap(), "trial={trial}");
        assert_eq!(lock.sweep_expired(), 1, "trial={trial}");
        current.release().await.unwrap();
        assert!(
            lock.try_acquire("lease", Duration::from_secs(1))
                .await
                .unwrap()
                .is_some(),
            "trial={trial}"
        );
    }
}
