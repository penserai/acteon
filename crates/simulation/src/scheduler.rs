//! Bounded execution of explicitly controlled futures and external events.
//!
//! This scheduler polls one root future and advances only its shared manual
//! clock. It deliberately rejects waits on OS I/O or independently spawned tasks.
//! It is not an operating-system or distributed-process scheduler.

use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

use acteon_time::{Clock, ManualClock};
use serde::{Deserialize, Serialize};

/// One observed clock advance or applied external event, in execution order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleRecord {
    pub at: Duration,
    pub event: ScheduleOperation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleOperation {
    Advance,
    Event { id: String },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ScheduleError {
    #[error(
        "event must have a unique nonempty ID, a future millisecond deadline, and fit the event budget"
    )]
    InvalidEvent,
    #[error("scheduler step budget exhausted")]
    BudgetExhausted,
    #[error("future waits without a registered virtual timer or scheduled event")]
    UncontrolledWait,
    #[error("invalid virtual time: {0}")]
    Time(#[from] acteon_time::AdvanceError),
}

#[derive(Default)]
struct WakeFlag(AtomicBool);

impl Wake for WakeFlag {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Timer/fault scheduler with stable insertion order for simultaneous events.
///
/// Due external events are applied before the root future is polled. Within
/// execution timeouts, the exclusive deadline still wins over ready work.
/// Each poll, clock advance, and event application consumes one step.
pub struct DeterministicScheduler<E> {
    clock: Arc<ManualClock>,
    events: BTreeMap<(Duration, usize), (String, E)>,
    ids: HashSet<String>,
    budget: usize,
    steps: usize,
    trace: Vec<ScheduleRecord>,
}

impl<E> DeterministicScheduler<E> {
    /// Bound both scheduling input and execution work with a finite step budget.
    pub fn new(clock: Arc<ManualClock>, budget: usize) -> Self {
        Self {
            clock,
            events: BTreeMap::new(),
            ids: HashSet::new(),
            budget,
            steps: 0,
            trace: Vec::new(),
        }
    }

    /// Schedule an external event at an absolute elapsed millisecond.
    pub fn schedule(
        &mut self,
        at_ms: u64,
        id: impl Into<String>,
        event: E,
    ) -> Result<(), ScheduleError> {
        let id = id.into();
        let at = Duration::from_millis(at_ms);
        if id.is_empty()
            || at < self.clock.monotonic()
            || self.ids.len() >= self.budget
            || self.ids.contains(&id)
        {
            return Err(ScheduleError::InvalidEvent);
        }
        let sequence = self.ids.len();
        self.ids.insert(id.clone());
        self.events.insert((at, sequence), (id, event));
        Ok(())
    }

    pub fn trace(&self) -> &[ScheduleRecord] {
        &self.trace
    }
    pub fn pending_events(&self) -> usize {
        self.events.len()
    }

    fn step(&mut self) -> Result<(), ScheduleError> {
        if self.steps >= self.budget {
            return Err(ScheduleError::BudgetExhausted);
        }
        self.steps += 1;
        Ok(())
    }

    fn record(&mut self, event: ScheduleOperation) {
        self.trace.push(ScheduleRecord {
            at: self.clock.monotonic(),
            event,
        });
    }

    /// Drive controlled work to completion without sleeping or spawning tasks.
    ///
    /// Call within a Tokio runtime if the work uses Tokio primitives. Pending
    /// work must register a clock timer, wake itself cooperatively, or have a
    /// declared external event. On any error the root future is dropped, which
    /// cancels its timers. Remaining events stay inspectable.
    pub fn run<F: Future>(
        &mut self,
        future: F,
        mut apply: impl FnMut(E),
    ) -> Result<F::Output, ScheduleError> {
        let wake = Arc::new(WakeFlag::default());
        let waker = Waker::from(Arc::clone(&wake));
        let mut context = Context::from_waker(&waker);
        // Tokio's ambient cooperative budget otherwise depends on how much
        // work the calling task did before entering this scheduler. This
        // scheduler has its own explicit poll/event budget and advances no OS
        // tasks, so an ambient budget yield cannot make progress here.
        let mut future = std::pin::pin!(tokio::task::unconstrained(future));
        loop {
            while self
                .events
                .first_key_value()
                .is_some_and(|((at, _), _)| *at <= self.clock.monotonic())
            {
                self.step()?;
                let (_, (id, event)) = self.events.pop_first().expect("due event exists");
                apply(event);
                self.record(ScheduleOperation::Event { id });
            }
            self.step()?;
            wake.0.store(false, Ordering::SeqCst);
            if let Poll::Ready(result) = future.as_mut().poll(&mut context) {
                return Ok(result);
            }
            if wake.0.load(Ordering::SeqCst) {
                continue;
            }
            let timer = self.clock.next_deadline();
            let event = self.events.first_key_value().map(|((at, _), _)| *at);
            let next = timer
                .into_iter()
                .chain(event)
                .min()
                .ok_or(ScheduleError::UncontrolledWait)?;
            self.step()?;
            self.clock.advance_to(next)?;
            self.record(ScheduleOperation::Advance);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn clock() -> Arc<ManualClock> {
        Arc::new(ManualClock::new(
            chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        ))
    }

    #[test]
    fn simultaneous_faults_follow_insertion_order_and_replay() {
        fn run() -> (Vec<u8>, Vec<ScheduleRecord>) {
            let clock = clock();
            let mut scheduler = DeterministicScheduler::new(Arc::clone(&clock), 20);
            scheduler.schedule(10, "fail", 1).unwrap();
            scheduler.schedule(10, "recover", 2).unwrap();
            let mut events = Vec::new();
            scheduler
                .run(clock.sleep(Duration::from_millis(11)), |event| {
                    events.push(event);
                })
                .unwrap();
            assert_eq!(scheduler.pending_events(), 0);
            assert_eq!(clock.pending_timers(), 0);
            (events, scheduler.trace().to_vec())
        }
        let first = run();
        assert_eq!(first.0, vec![1, 2]);
        assert_eq!(first, run());
    }

    #[test]
    fn undeclared_wait_and_self_wake_loop_fail_closed() {
        let clock = clock();
        let mut scheduler = DeterministicScheduler::<()>::new(Arc::clone(&clock), 4);
        assert_eq!(
            scheduler.run(std::future::pending::<()>(), |()| {}),
            Err(ScheduleError::UncontrolledWait)
        );
        let spin = std::future::poll_fn(|cx| {
            cx.waker().wake_by_ref();
            Poll::<()>::Pending
        });
        assert_eq!(
            scheduler.run(spin, |()| {}),
            Err(ScheduleError::BudgetExhausted)
        );
        assert_eq!(clock.monotonic(), Duration::ZERO);
    }

    #[test]
    fn cancellation_and_input_limits() {
        let clock = clock();
        let mut scheduler = DeterministicScheduler::new(Arc::clone(&clock), 1);
        scheduler.schedule(10, "fault", ()).unwrap();
        assert_eq!(
            scheduler.schedule(10, "fault", ()),
            Err(ScheduleError::InvalidEvent)
        );
        assert_eq!(
            scheduler.schedule(20, "extra", ()),
            Err(ScheduleError::InvalidEvent)
        );
        assert_eq!(
            scheduler.run(clock.sleep(Duration::from_secs(1)), |()| {}),
            Err(ScheduleError::BudgetExhausted)
        );
        assert_eq!(clock.pending_timers(), 0);
        clock.advance_to(Duration::from_secs(1)).unwrap();
        let mut scheduler = DeterministicScheduler::new(clock, 4);
        assert_eq!(
            scheduler.schedule(999, "past", ()),
            Err(ScheduleError::InvalidEvent)
        );
    }

    #[tokio::test]
    async fn virtual_work_is_independent_of_tokio_cooperative_budget() {
        let clock = clock();
        let semaphore = tokio::sync::Semaphore::new(1);
        let mut scheduler = DeterministicScheduler::<()>::new(clock.clone(), 2_000);
        scheduler
            .run(
                async {
                    for _ in 0..512 {
                        let permit = semaphore.acquire().await.unwrap();
                        clock.sleep(Duration::from_millis(1)).await;
                        drop(permit);
                    }
                },
                |()| {},
            )
            .unwrap();
        assert_eq!(clock.monotonic(), Duration::from_millis(512));
        assert_eq!(clock.pending_timers(), 0);
    }
}
