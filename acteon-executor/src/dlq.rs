use std::sync::Mutex;
use std::time::SystemTime;

use acteon_core::Action;
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

/// Trait for dead-letter queue backends.
///
/// Implementations must be `Send + Sync` for use across async tasks.
#[async_trait]
pub trait DeadLetterSink: Send + Sync {
    /// Append a failed action to the dead-letter queue.
    async fn push(&self, action: Action, error: String, attempts: u32);

    /// Drain all entries from the queue, returning them.
    async fn drain(&self) -> Vec<DeadLetterEntry>;

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
        Self {
            entries: Mutex::new(Vec::new()),
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
            timestamp: SystemTime::now(),
        };
        self.entries.lock().expect("dlq mutex poisoned").push(entry);
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
    async fn push(&self, action: Action, error: String, attempts: u32) {
        DeadLetterQueue::push(self, action, error, attempts);
    }

    async fn drain(&self) -> Vec<DeadLetterEntry> {
        DeadLetterQueue::drain(self)
    }

    async fn len(&self) -> usize {
        DeadLetterQueue::len(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[allow(dead_code)]
    fn _assert_dyn_sink(_: &dyn DeadLetterSink) {}
}
