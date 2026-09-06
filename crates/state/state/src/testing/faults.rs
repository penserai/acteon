//! Controlled, one-shot write interruptions for recovery contracts.

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
    Delete,
    IndexTimeout,
    IndexChainReady,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultTiming {
    Before,
    After,
}

enum Interruption {
    Fail,
    Pause(oneshot::Receiver<()>),
}
struct ArmedFault {
    kind: KeyKind,
    operation: WriteOperation,
    timing: FaultTiming,
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
            operation,
            timing,
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
            operation,
            timing,
            interruption: Interruption::Pause(rx),
        })?;
        Ok(tx)
    }
    #[must_use]
    pub fn consumed(&self) -> usize {
        self.consumed.load(Ordering::SeqCst)
    }
    fn arm(&self, fault: ArmedFault) -> Result<(), StateError> {
        let mut armed = self.armed.lock().expect("fault controller");
        if armed.is_some() {
            return Err(StateError::Backend("a write fault is already armed".into()));
        }
        *armed = Some(fault);
        Ok(())
    }
    async fn interrupt(
        &self,
        key: &StateKey,
        operation: WriteOperation,
        timing: FaultTiming,
    ) -> Result<(), StateError> {
        let interruption = {
            let mut armed = self.armed.lock().expect("fault controller");
            if armed.as_ref().is_some_and(|fault| {
                fault.kind == key.kind && fault.operation == operation && fault.timing == timing
            }) {
                armed.take().map(|fault| fault.interruption)
            } else {
                None
            }
        };
        let Some(interruption) = interruption else {
            return Ok(());
        };
        self.consumed.fetch_add(1, Ordering::SeqCst);
        match interruption {
            Interruption::Fail => Err(StateError::Connection("injected write interruption".into())),
            Interruption::Pause(rx) => rx
                .await
                .map_err(|_| StateError::Connection("write pause controller dropped".into())),
        }
    }
}

#[async_trait::async_trait]
impl StateStore for FaultStore {
    async fn compare_and_swap(
        &self,
        key: &StateKey,
        expected_version: u64,
        new_value: &str,
        ttl: Option<Duration>,
    ) -> Result<CasResult, StateError> {
        self.interrupt(key, WriteOperation::CompareAndSwap, FaultTiming::Before)
            .await?;
        let result = self
            .inner
            .compare_and_swap(key, expected_version, new_value, ttl)
            .await?;
        if result == CasResult::Ok {
            self.interrupt(key, WriteOperation::CompareAndSwap, FaultTiming::After)
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
        self.interrupt(key, WriteOperation::CheckAndSet, FaultTiming::Before)
            .await?;
        let result = self.inner.check_and_set(key, value, ttl).await?;
        if result {
            self.interrupt(key, WriteOperation::CheckAndSet, FaultTiming::After)
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
        self.interrupt(key, WriteOperation::Set, FaultTiming::Before)
            .await?;
        self.inner.set(key, value, ttl).await?;
        self.interrupt(key, WriteOperation::Set, FaultTiming::After)
            .await
    }
    async fn delete(&self, key: &StateKey) -> Result<bool, StateError> {
        self.interrupt(key, WriteOperation::Delete, FaultTiming::Before)
            .await?;
        let result = self.inner.delete(key).await?;
        if result {
            self.interrupt(key, WriteOperation::Delete, FaultTiming::After)
                .await?;
        }
        Ok(result)
    }
    async fn get(&self, key: &StateKey) -> Result<Option<String>, StateError> {
        self.inner.get(key).await
    }
    async fn get_versioned(&self, key: &StateKey) -> Result<Option<(String, u64)>, StateError> {
        self.inner.get_versioned(key).await
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
        self.interrupt(key, WriteOperation::IndexTimeout, FaultTiming::Before)
            .await?;
        self.inner.index_timeout(key, expires_at_ms).await?;
        self.interrupt(key, WriteOperation::IndexTimeout, FaultTiming::After)
            .await
    }
    async fn remove_timeout_index(&self, key: &StateKey) -> Result<(), StateError> {
        self.inner.remove_timeout_index(key).await
    }
    async fn get_expired_timeouts(&self, now_ms: i64) -> Result<Vec<String>, StateError> {
        self.inner.get_expired_timeouts(now_ms).await
    }
    async fn index_chain_ready(&self, key: &StateKey, ready_at_ms: i64) -> Result<(), StateError> {
        self.interrupt(key, WriteOperation::IndexChainReady, FaultTiming::Before)
            .await?;
        self.inner.index_chain_ready(key, ready_at_ms).await?;
        self.interrupt(key, WriteOperation::IndexChainReady, FaultTiming::After)
            .await
    }
    async fn remove_chain_ready_index(&self, key: &StateKey) -> Result<(), StateError> {
        self.inner.remove_chain_ready_index(key).await
    }
    async fn get_ready_chains(&self, now_ms: i64) -> Result<Vec<String>, StateError> {
        self.inner.get_ready_chains(now_ms).await
    }
}
