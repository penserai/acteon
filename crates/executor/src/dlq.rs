use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use acteon_core::Action;
use acteon_time::{Clock, SystemClock};
use async_trait::async_trait;

/// An entry in the dead-letter queue representing a permanently failed action.
#[derive(Debug)]
pub struct DeadLetterEntry {
    /// The action that could not be executed successfully.
    pub action: Action,
    /// Human-readable description of the final error.
    pub error: String,
    /// Number of execution attempts made before the action was abandoned.
    pub attempts: u32,
    /// Wall-clock time at which the entry was created.
    pub timestamp: SystemTime,
}

/// A failed dead-letter write. Callers must not assume the action was retained.
#[derive(Debug, thiserror::Error)]
#[error("dead-letter persistence failed: {0}")]
pub struct DeadLetterError(pub String);

/// Trait for dead-letter queue backends.
///
/// Implementations must be `Send + Sync` for use across async tasks.
#[async_trait]
pub trait DeadLetterSink: Send + Sync {
    /// Append a failed action to the dead-letter queue.
    async fn push(
        &self,
        action: Action,
        error: String,
        attempts: u32,
    ) -> Result<(), DeadLetterError>;

    /// Cumulative storage or encryption failures since this sink was started.
    fn failure_count(&self) -> u64 {
        0
    }

    /// Drain all entries from the queue, returning them.
    async fn drain(&self) -> Vec<DeadLetterEntry>;

    /// Remove entries older than the sink's configured retention window.
    ///
    /// Sinks without a local retention policy return zero. Persistent sinks
    /// should enforce retention in their backend; this hook lets in-memory
    /// sinks participate in the gateway's periodic cleanup cadence.
    async fn cleanup_expired(&self) -> usize {
        0
    }

    /// Return the number of entries in the queue.
    async fn len(&self) -> usize;

    /// Return true if the queue is empty.
    async fn is_empty(&self) -> bool {
        self.len().await == 0
    }
}

/// In-memory dead-letter queue for actions that exhausted all retry attempts.
///
/// The DLQ is a simple append-only buffer guarded by a [`Mutex`]. In a
/// production system this would be backed by durable storage (e.g. a database
/// or message queue). The current implementation is suitable for tests and
/// development.
///
/// # Thread safety
///
/// All methods acquire the internal lock for the minimum duration needed.
/// Because the lock is a standard `Mutex` (not `tokio::sync::Mutex`), callers
/// must not hold the lock across `.await` points. The public API ensures this
/// by never returning a guard.
pub struct DeadLetterQueue {
    entries: Mutex<Vec<DeadLetterEntry>>,
    clock: Arc<dyn Clock>,
    retention: Option<Duration>,
}

impl DeadLetterQueue {
    /// Create a new empty dead-letter queue.
    ///
    /// # Examples
    ///
    /// ```
    /// use acteon_executor::dlq::DeadLetterQueue;
    ///
    /// let dlq = DeadLetterQueue::new();
    /// assert!(dlq.is_empty());
    /// ```
    pub fn new() -> Self {
        Self::with_clock_and_retention(Arc::new(SystemClock::default()), None)
    }

    /// Create a queue that uses `clock` for timestamps and retention checks.
    #[must_use]
    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self::with_clock_and_retention(clock, None)
    }

    /// Create a queue with an optional retention window.
    ///
    /// An entry expires when `timestamp + retention <= clock.now()`. A
    /// `None` retention keeps the historical unbounded in-memory behavior.
    #[must_use]
    pub fn with_clock_and_retention(clock: Arc<dyn Clock>, retention: Option<Duration>) -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
            clock,
            retention,
        }
    }

    /// Append an action to the dead-letter queue.
    ///
    /// The entry is timestamped with the current system time.
    pub fn push(&self, action: Action, error: String, attempts: u32) {
        let entry = DeadLetterEntry {
            action,
            error,
            attempts,
            timestamp: self.clock.now().into(),
        };
        self.entries.lock().expect("dlq mutex poisoned").push(entry);
    }

    /// Remove entries whose retention window has elapsed.
    pub fn cleanup_expired(&self) -> usize {
        let Some(retention) = self.retention else {
            return 0;
        };
        let now: SystemTime = self.clock.now().into();
        let mut guard = self.entries.lock().expect("dlq mutex poisoned");
        let before = guard.len();
        guard.retain(|entry| {
            now.duration_since(entry.timestamp)
                .map_or(true, |age| age < retention)
        });
        before - guard.len()
    }

    /// Drain all entries from the queue, returning them as a `Vec`.
    ///
    /// After this call the queue is empty.
    pub fn drain(&self) -> Vec<DeadLetterEntry> {
        let mut guard = self.entries.lock().expect("dlq mutex poisoned");
        std::mem::take(&mut *guard)
    }

    /// Return the number of entries currently in the queue.
    pub fn len(&self) -> usize {
        self.entries.lock().expect("dlq mutex poisoned").len()
    }

    /// Return `true` if the queue contains no entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for DeadLetterQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DeadLetterSink for DeadLetterQueue {
    async fn push(
        &self,
        action: Action,
        error: String,
        attempts: u32,
    ) -> Result<(), DeadLetterError> {
        DeadLetterQueue::push(self, action, error, attempts);
        Ok(())
    }

    async fn drain(&self) -> Vec<DeadLetterEntry> {
        DeadLetterQueue::drain(self)
    }

    async fn cleanup_expired(&self) -> usize {
        DeadLetterQueue::cleanup_expired(self)
    }

    async fn len(&self) -> usize {
        DeadLetterQueue::len(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_time::ManualClock;
    use chrono::{TimeZone, Utc};
    use std::sync::Arc;

    fn test_action() -> Action {
        Action::new("ns", "t", "p", "type", serde_json::Value::Null)
    }

    #[test]
    fn new_queue_is_empty() {
        let dlq = DeadLetterQueue::new();
        assert!(dlq.is_empty());
        assert_eq!(dlq.len(), 0);
    }

    #[test]
    fn push_increments_len() {
        let dlq = DeadLetterQueue::new();
        dlq.push(test_action(), "err1".into(), 3);
        assert_eq!(dlq.len(), 1);
        dlq.push(test_action(), "err2".into(), 5);
        assert_eq!(dlq.len(), 2);
        assert!(!dlq.is_empty());
    }

    #[test]
    fn drain_returns_all_entries_and_empties_queue() {
        let dlq = DeadLetterQueue::new();
        dlq.push(test_action(), "e1".into(), 1);
        dlq.push(test_action(), "e2".into(), 2);
        dlq.push(test_action(), "e3".into(), 3);

        let entries = dlq.drain();
        assert_eq!(entries.len(), 3);
        assert!(dlq.is_empty());

        // Verify ordering and content.
        assert_eq!(entries[0].error, "e1");
        assert_eq!(entries[0].attempts, 1);
        assert_eq!(entries[1].error, "e2");
        assert_eq!(entries[2].error, "e3");
        assert_eq!(entries[2].attempts, 3);
    }

    #[test]
    fn drain_on_empty_returns_empty_vec() {
        let dlq = DeadLetterQueue::new();
        let entries = dlq.drain();
        assert!(entries.is_empty());
    }

    #[test]
    fn entries_have_timestamps() {
        let before = SystemTime::now();
        let dlq = DeadLetterQueue::new();
        dlq.push(test_action(), "err".into(), 1);
        let after = SystemTime::now();

        let entries = dlq.drain();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].timestamp >= before);
        assert!(entries[0].timestamp <= after);
    }

    #[test]
    fn default_creates_empty_queue() {
        let dlq = DeadLetterQueue::default();
        assert!(dlq.is_empty());
    }

    #[test]
    fn retention_expires_at_exact_clock_boundary() {
        let clock = Arc::new(ManualClock::new(
            Utc.with_ymd_and_hms(2026, 9, 12, 0, 0, 0).unwrap(),
        ));
        let dlq = DeadLetterQueue::with_clock_and_retention(
            Arc::clone(&clock) as Arc<dyn Clock>,
            Some(Duration::from_secs(10)),
        );
        dlq.push(test_action(), "expired".into(), 1);

        assert_eq!(dlq.cleanup_expired(), 0);
        clock
            .advance_to(Duration::from_secs(9))
            .expect("manual clock advance");
        assert_eq!(dlq.cleanup_expired(), 0);
        clock
            .advance_to(Duration::from_secs(10))
            .expect("manual clock advance");
        assert_eq!(dlq.cleanup_expired(), 1);
        assert_eq!(dlq.cleanup_expired(), 0);
        assert!(dlq.is_empty());
    }

    #[test]
    fn no_retention_preserves_entries() {
        let clock = Arc::new(ManualClock::new(
            Utc.with_ymd_and_hms(2026, 9, 12, 0, 0, 0).unwrap(),
        ));
        let dlq = DeadLetterQueue::with_clock(Arc::clone(&clock) as Arc<dyn Clock>);
        dlq.push(test_action(), "kept".into(), 1);
        clock
            .advance_to(Duration::from_secs(86_400))
            .expect("manual clock advance");
        assert_eq!(dlq.cleanup_expired(), 0);
        assert_eq!(dlq.len(), 1);
    }

    #[allow(dead_code)]
    fn _assert_dyn_sink(_: &dyn DeadLetterSink) {}
}
