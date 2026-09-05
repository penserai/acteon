use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::{Clock, Sleep};

/// An invalid virtual-time advance. Failed advances leave time and timers intact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdvanceError {
    #[error("virtual time cannot move backwards")]
    Backwards,
    #[error("virtual time exceeds the representable UTC range")]
    Overflow,
}

#[derive(Debug, Default)]
struct State {
    elapsed: Duration,
    next_id: u64,
    timers: BTreeMap<(Duration, u64), Waker>,
}

/// Explicit virtual clock with a fixed UTC epoch and ordered, cancel-safe timers.
///
/// Advancement wakes due timers in deadline/registration order. It does not run
/// tasks or prescribe Tokio task order. A scenario scheduler must poll its work
/// between advances and explicitly order simultaneous external events.
#[derive(Debug, Clone)]
pub struct ManualClock {
    epoch: DateTime<Utc>,
    state: Arc<Mutex<State>>,
}

impl ManualClock {
    pub fn new(epoch: DateTime<Utc>) -> Self {
        Self {
            epoch,
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    /// Advance to an absolute elapsed time, waking due timers outside the lock.
    pub fn advance_to(&self, elapsed: Duration) -> Result<(), AdvanceError> {
        let mut state = self.state.lock().expect("clock lock poisoned");
        if elapsed < state.elapsed {
            return Err(AdvanceError::Backwards);
        }
        let delta = chrono::Duration::from_std(elapsed).map_err(|_| AdvanceError::Overflow)?;
        self.epoch
            .checked_add_signed(delta)
            .ok_or(AdvanceError::Overflow)?;
        state.elapsed = elapsed;
        let mut due = Vec::new();
        while state
            .timers
            .first_key_value()
            .is_some_and(|((at, _), _)| *at <= elapsed)
        {
            due.push(state.timers.pop_first().expect("due timer exists").1);
        }
        drop(state);
        for waker in due {
            waker.wake();
        }
        Ok(())
    }

    /// Earliest registered deadline. Cancelled timers do not remain registered.
    pub fn next_deadline(&self) -> Option<Duration> {
        self.state
            .lock()
            .expect("clock lock poisoned")
            .timers
            .first_key_value()
            .map(|((at, _), _)| *at)
    }

    pub fn pending_timers(&self) -> usize {
        self.state.lock().expect("clock lock poisoned").timers.len()
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        self.epoch
            + chrono::Duration::from_std(self.monotonic()).expect("advance validated UTC range")
    }
    fn monotonic(&self) -> Duration {
        self.state.lock().expect("clock lock poisoned").elapsed
    }
    fn sleep_until(&self, deadline: Duration) -> Sleep {
        Box::pin(ManualSleep {
            state: Arc::clone(&self.state),
            deadline,
            id: None,
        })
    }
}

struct ManualSleep {
    state: Arc<Mutex<State>>,
    deadline: Duration,
    id: Option<u64>,
}

impl Future for ManualSleep {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = &mut *self;
        let mut state = this.state.lock().expect("clock lock poisoned");
        if state.elapsed >= this.deadline {
            if let Some(id) = this.id.take() {
                state.timers.remove(&(this.deadline, id));
            }
            return Poll::Ready(());
        }
        let id = *this.id.get_or_insert_with(|| {
            let id = state.next_id;
            state.next_id = id.checked_add(1).expect("timer identifier exhausted");
            id
        });
        state.timers.insert((this.deadline, id), cx.waker().clone());
        Poll::Pending
    }
}

impl Drop for ManualSleep {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            self.state
                .lock()
                .expect("clock lock poisoned")
                .timers
                .remove(&(self.deadline, id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn clock() -> ManualClock {
        ManualClock::new(DateTime::from_timestamp(1_700_000_000, 0).unwrap())
    }
    fn poll(future: &mut Sleep) -> Poll<()> {
        future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
    }

    #[test]
    fn utc_ttls_and_timers_share_exact_boundaries() {
        let clock = clock();
        let epoch = clock.now();
        let mut timer = clock.sleep(Duration::from_secs(2));
        assert!(poll(&mut timer).is_pending());
        clock.advance_to(Duration::from_millis(1999)).unwrap();
        assert!(poll(&mut timer).is_pending());
        clock.advance_to(Duration::from_secs(2)).unwrap();
        assert!(poll(&mut timer).is_ready());
        assert_eq!(clock.now(), epoch + chrono::Duration::seconds(2));
        assert_eq!(clock.pending_timers(), 0);
    }

    #[test]
    fn timers_wake_in_deadline_then_registration_order() {
        struct Recorder(u8, Arc<Mutex<Vec<u8>>>);
        impl std::task::Wake for Recorder {
            fn wake(self: Arc<Self>) {
                self.1.lock().unwrap().push(self.0);
            }
        }
        let clock = clock();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut timers = Vec::new();
        for (id, at) in [(1, 20), (2, 10), (3, 10)] {
            let waker = Waker::from(Arc::new(Recorder(id, seen.clone())));
            let mut timer = clock.sleep(Duration::from_millis(at));
            assert!(
                timer
                    .as_mut()
                    .poll(&mut Context::from_waker(&waker))
                    .is_pending()
            );
            timers.push(timer);
        }
        clock.advance_to(Duration::from_millis(20)).unwrap();
        assert_eq!(*seen.lock().unwrap(), vec![2, 3, 1]);
        assert_eq!(clock.pending_timers(), 0);
        assert!(timers.iter_mut().all(|timer| poll(timer).is_ready()));
    }

    #[test]
    fn cancellation_and_repoll_do_not_leak_timers() {
        let clock = clock();
        let mut first = clock.sleep(Duration::from_secs(2));
        let mut second = clock.sleep(Duration::from_secs(1));
        assert!(poll(&mut first).is_pending());
        assert!(poll(&mut second).is_pending());
        assert!(poll(&mut second).is_pending());
        assert_eq!(clock.pending_timers(), 2);
        drop(second);
        assert_eq!(clock.next_deadline(), Some(Duration::from_secs(2)));
        drop(first);
        assert_eq!(clock.next_deadline(), None);
    }

    #[test]
    fn advancement_before_first_poll_and_rejected_advances() {
        let clock = clock();
        let mut timer = clock.sleep(Duration::from_secs(1));
        clock.advance_to(Duration::from_secs(1)).unwrap();
        assert!(poll(&mut timer).is_ready());
        assert_eq!(
            clock.advance_to(Duration::ZERO),
            Err(AdvanceError::Backwards)
        );
        assert_eq!(clock.advance_to(Duration::MAX), Err(AdvanceError::Overflow));
        assert_eq!(clock.monotonic(), Duration::from_secs(1));
    }

    #[tokio::test]
    async fn due_deadline_wins_over_ready_work() {
        let clock = clock();
        assert_eq!(
            crate::timeout(&clock, Duration::ZERO, async { 42 }).await,
            Err(crate::Elapsed)
        );
        assert_eq!(
            crate::timeout(&clock, Duration::from_secs(1), async { 42 }).await,
            Ok(42)
        );
        assert_eq!(clock.pending_timers(), 0);
    }
}
