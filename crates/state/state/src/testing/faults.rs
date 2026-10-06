//! Controlled, one-shot store interruptions for recovery and authorization contracts.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use tokio::sync::oneshot;

use crate::{CasResult, KeyKind, StateError, StateKey, StateStore};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOperation {
    Set,
    CheckAndSet,
    CompareAndSwap,
    CompareAndDelete,
    Delete,
    IndexTimeout,
    IndexChainReady,
}

/// Read boundaries at which a test can deterministically change external authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOperation {
    Get,
    GetVersioned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operation {
    Write(WriteOperation),
    Read(ReadOperation),
}
impl From<WriteOperation> for Operation {
    fn from(value: WriteOperation) -> Self {
        Self::Write(value)
    }
}
impl From<ReadOperation> for Operation {
    fn from(value: ReadOperation) -> Self {
        Self::Read(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultTiming {
    Before,
    After,
}

enum Interruption {
    Fail,
    Pause(oneshot::Receiver<()>),
    ReadPause {
        reached: oneshot::Sender<()>,
        resume: oneshot::Receiver<()>,
    },
}
struct ArmedFault {
    kind: KeyKind,
    operation: Operation,
    timing: FaultTiming,
    matches_before_interrupt: usize,
    interruption: Interruption,
}

/// Wrap a real store with one explicitly armed failure or pause. An `After`
/// failure models lost acknowledgement: the write already reached the store.
/// Pauses are controlled by the returned sender; no wall-clock sleep is used.
/// This is a test adapter, not a production retry policy.
pub struct FaultStore {
    inner: Arc<dyn StateStore>,
    armed: Mutex<Option<ArmedFault>>,
    consumed: AtomicUsize,
}

impl FaultStore {
    #[must_use]
    pub fn new(inner: Arc<dyn StateStore>) -> Self {
        Self {
            inner,
            armed: Mutex::new(None),
            consumed: AtomicUsize::new(0),
        }
    }
    pub fn fail_next(
        &self,
        kind: KeyKind,
        operation: WriteOperation,
        timing: FaultTiming,
    ) -> Result<(), StateError> {
        self.arm(ArmedFault {
            kind,
            operation: operation.into(),
            timing,
            matches_before_interrupt: 0,
            interruption: Interruption::Fail,
        })
    }

    /// Fail after this many matching operations have completed normally.
    ///
    /// This targets a later persistence boundary in one production operation
    /// without using timing or a test-only production hook. For example, a
    /// terminal task projection may persist artifact enrichment before its
    /// final status transition; `matches_before_interrupt = 1` interrupts the
    /// latter write.
    pub fn fail_after_matches(
        &self,
        kind: KeyKind,
        operation: WriteOperation,
        timing: FaultTiming,
        matches_before_interrupt: usize,
    ) -> Result<(), StateError> {
        self.arm(ArmedFault {
            kind,
            operation: operation.into(),
            timing,
            matches_before_interrupt,
            interruption: Interruption::Fail,
        })
    }
    pub fn pause_next(
        &self,
        kind: KeyKind,
        operation: WriteOperation,
        timing: FaultTiming,
    ) -> Result<oneshot::Sender<()>, StateError> {
        let (tx, rx) = oneshot::channel();
        self.arm(ArmedFault {
            kind,
            operation: operation.into(),
            timing,
            matches_before_interrupt: 0,
            interruption: Interruption::Pause(rx),
        })?;
        Ok(tx)
    }
    /// Pause a read before or after observing the underlying backend. The first
    /// receiver signals that the boundary was reached; the sender resumes it.
    /// This uses the same single armed fault as writes, with no polling or sleep.
    pub fn pause_next_read(
        &self,
        kind: KeyKind,
        operation: ReadOperation,
        timing: FaultTiming,
    ) -> Result<(oneshot::Receiver<()>, oneshot::Sender<()>), StateError> {
        let (reached_tx, reached_rx) = oneshot::channel();
        let (resume_tx, resume_rx) = oneshot::channel();
        self.arm(ArmedFault {
            kind,
            operation: operation.into(),
            timing,
            matches_before_interrupt: 0,
            interruption: Interruption::ReadPause {
                reached: reached_tx,
                resume: resume_rx,
            },
        })?;
        Ok((reached_rx, resume_tx))
    }

    #[must_use]
    pub fn consumed(&self) -> usize {
        self.consumed.load(Ordering::SeqCst)
    }
    fn arm(&self, fault: ArmedFault) -> Result<(), StateError> {
        let mut armed = self.armed.lock().expect("fault controller");
        if armed.is_some() {
            return Err(StateError::Backend("a store fault is already armed".into()));
        }
        *armed = Some(fault);
        Ok(())
    }
    async fn interrupt(
        &self,
        key: &StateKey,
        operation: Operation,
        timing: FaultTiming,
    ) -> Result<(), StateError> {
        let interruption = {
            let mut armed = self.armed.lock().expect("fault controller");
            let Some(fault) = armed.as_mut() else {
                return Ok(());
            };
            if fault.kind != key.kind || fault.operation != operation || fault.timing != timing {
                None
            } else if fault.matches_before_interrupt > 0 {
                fault.matches_before_interrupt -= 1;
                None
            } else {
                armed.take().map(|fault| fault.interruption)
            }
        };
        let Some(interruption) = interruption else {
            return Ok(());
        };
        self.consumed.fetch_add(1, Ordering::SeqCst);
        match interruption {
            Interruption::Fail => Err(StateError::Connection("injected write interruption".into())),
            Interruption::ReadPause { reached, resume } => {
                reached.send(()).map_err(|()| {
                    StateError::Connection("read boundary observer dropped".into())
                })?;
                resume
                    .await
                    .map_err(|_| StateError::Connection("read pause controller dropped".into()))
            }
            Interruption::Pause(rx) => rx
                .await
                .map_err(|_| StateError::Connection("write pause controller dropped".into())),
        }
    }
}

#[async_trait::async_trait]
impl StateStore for FaultStore {
    async fn compare_and_delete(
        &self,
        key: &StateKey,
        expected_version: u64,
    ) -> Result<bool, StateError> {
        self.interrupt(
            key,
            WriteOperation::CompareAndDelete.into(),
            FaultTiming::Before,
        )
        .await?;
        let deleted = self.inner.compare_and_delete(key, expected_version).await?;
        if deleted {
            self.interrupt(
                key,
                WriteOperation::CompareAndDelete.into(),
                FaultTiming::After,
            )
            .await?;
        }
        Ok(deleted)
    }

    async fn compare_and_swap(
        &self,
        key: &StateKey,
        expected_version: u64,
        new_value: &str,
        ttl: Option<Duration>,
    ) -> Result<CasResult, StateError> {
        self.interrupt(
            key,
            WriteOperation::CompareAndSwap.into(),
            FaultTiming::Before,
        )
        .await?;
        let result = self
            .inner
            .compare_and_swap(key, expected_version, new_value, ttl)
            .await?;
        if result == CasResult::Ok {
            self.interrupt(
                key,
                WriteOperation::CompareAndSwap.into(),
                FaultTiming::After,
            )
            .await?;
        }
        Ok(result)
    }
    async fn check_and_set(
        &self,
        key: &StateKey,
        value: &str,
        ttl: Option<Duration>,
    ) -> Result<bool, StateError> {
        self.interrupt(key, WriteOperation::CheckAndSet.into(), FaultTiming::Before)
            .await?;
        let result = self.inner.check_and_set(key, value, ttl).await?;
        if result {
            self.interrupt(key, WriteOperation::CheckAndSet.into(), FaultTiming::After)
                .await?;
        }
        Ok(result)
    }
    async fn set(
        &self,
        key: &StateKey,
        value: &str,
        ttl: Option<Duration>,
    ) -> Result<(), StateError> {
        self.interrupt(key, WriteOperation::Set.into(), FaultTiming::Before)
            .await?;
        self.inner.set(key, value, ttl).await?;
        self.interrupt(key, WriteOperation::Set.into(), FaultTiming::After)
            .await
    }
    async fn delete(&self, key: &StateKey) -> Result<bool, StateError> {
        self.interrupt(key, WriteOperation::Delete.into(), FaultTiming::Before)
            .await?;
        let result = self.inner.delete(key).await?;
        if result {
            self.interrupt(key, WriteOperation::Delete.into(), FaultTiming::After)
                .await?;
        }
        Ok(result)
    }
    async fn get(&self, key: &StateKey) -> Result<Option<String>, StateError> {
        self.interrupt(key, ReadOperation::Get.into(), FaultTiming::Before)
            .await?;
        let value = self.inner.get(key).await?;
        self.interrupt(key, ReadOperation::Get.into(), FaultTiming::After)
            .await?;
        Ok(value)
    }
    async fn get_versioned(&self, key: &StateKey) -> Result<Option<(String, u64)>, StateError> {
        self.interrupt(key, ReadOperation::GetVersioned.into(), FaultTiming::Before)
            .await?;
        let value = self.inner.get_versioned(key).await?;
        self.interrupt(key, ReadOperation::GetVersioned.into(), FaultTiming::After)
            .await?;
        Ok(value)
    }
    async fn increment(
        &self,
        key: &StateKey,
        delta: i64,
        ttl: Option<Duration>,
    ) -> Result<i64, StateError> {
        self.inner.increment(key, delta, ttl).await
    }
    async fn scan_keys(
        &self,
        namespace: &str,
        tenant: &str,
        kind: KeyKind,
        prefix: Option<&str>,
    ) -> Result<Vec<(String, String)>, StateError> {
        self.inner.scan_keys(namespace, tenant, kind, prefix).await
    }
    async fn scan_keys_by_kind(&self, kind: KeyKind) -> Result<Vec<(String, String)>, StateError> {
        self.inner.scan_keys_by_kind(kind).await
    }
    async fn index_timeout(&self, key: &StateKey, expires_at_ms: i64) -> Result<(), StateError> {
        self.interrupt(
            key,
            WriteOperation::IndexTimeout.into(),
            FaultTiming::Before,
        )
        .await?;
        self.inner.index_timeout(key, expires_at_ms).await?;
        self.interrupt(key, WriteOperation::IndexTimeout.into(), FaultTiming::After)
            .await
    }
    async fn remove_timeout_index(&self, key: &StateKey) -> Result<(), StateError> {
        self.inner.remove_timeout_index(key).await
    }
    async fn get_expired_timeouts(&self, now_ms: i64) -> Result<Vec<String>, StateError> {
        self.inner.get_expired_timeouts(now_ms).await
    }
    async fn index_chain_ready(&self, key: &StateKey, ready_at_ms: i64) -> Result<(), StateError> {
        self.interrupt(
            key,
            WriteOperation::IndexChainReady.into(),
            FaultTiming::Before,
        )
        .await?;
        self.inner.index_chain_ready(key, ready_at_ms).await?;
        self.interrupt(
            key,
            WriteOperation::IndexChainReady.into(),
            FaultTiming::After,
        )
        .await
    }
    async fn remove_chain_ready_index(&self, key: &StateKey) -> Result<(), StateError> {
        self.inner.remove_chain_ready_index(key).await
    }
    async fn get_ready_chains(&self, now_ms: i64) -> Result<Vec<String>, StateError> {
        self.inner.get_ready_chains(now_ms).await
    }
}
