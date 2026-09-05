//! Wall time, monotonic deadlines, and timers from one injectable clock.
//!
//! Production uses [`SystemClock`]. [`ManualClock`] moves only when its owner
//! explicitly advances it; it neither pauses Tokio nor changes process time.
//! Components sharing state must use the same clock domain.

mod manual;

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use chrono::{DateTime, Utc};

pub use manual::{AdvanceError, ManualClock};

/// An owned, cancel-safe timer future.
pub type Sleep = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Time source shared by decisions, TTLs, retries, and timeouts.
pub trait Clock: Send + Sync + std::fmt::Debug {
    /// Current UTC time for persisted timestamps and calendar decisions.
    fn now(&self) -> DateTime<Utc>;
    /// Elapsed time in this clock's domain. It must never go backwards.
    fn monotonic(&self) -> Duration;
    /// Wait until an absolute monotonic deadline, or complete immediately if due.
    fn sleep_until(&self, deadline: Duration) -> Sleep;
    /// Wait for a duration measured from this call (not the first poll).
    fn sleep(&self, duration: Duration) -> Sleep {
        self.sleep_until(self.monotonic().saturating_add(duration))
    }
}

/// Real UTC time and Tokio's monotonic timers. The default production clock.
#[derive(Debug)]
pub struct SystemClock {
    origin: tokio::time::Instant,
}

impl Default for SystemClock {
    fn default() -> Self {
        Self {
            origin: tokio::time::Instant::now(),
        }
    }
}

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
    fn monotonic(&self) -> Duration {
        self.origin.elapsed()
    }
    fn sleep_until(&self, deadline: Duration) -> Sleep {
        let instant = self.origin.checked_add(deadline);
        Box::pin(async move {
            match instant {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        })
    }
}

/// A future did not complete before its deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("deadline elapsed")]
pub struct Elapsed;

/// Run a future before an absolute deadline in the supplied clock's domain.
///
/// The deadline is exclusive: when work and expiration are both ready,
/// expiration wins. Dropping this future drops both work and timer.
pub async fn timeout_at<F: Future>(
    clock: &dyn Clock,
    deadline: Duration,
    future: F,
) -> Result<F::Output, Elapsed> {
    tokio::select! {
        biased;
        () = clock.sleep_until(deadline) => Err(Elapsed),
        result = future => Ok(result),
    }
}

/// Run a future with a timeout in the supplied clock's domain.
pub async fn timeout<F: Future>(
    clock: &dyn Clock,
    duration: Duration,
    future: F,
) -> Result<F::Output, Elapsed> {
    timeout_at(clock, clock.monotonic().saturating_add(duration), future).await
}
