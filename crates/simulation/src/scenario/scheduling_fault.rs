//! One-shot persistence failure at the scheduled-outcome boundary.
use acteon_state::{CasResult, KeyKind, StateError, StateKey, StateStore};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

pub(super) struct CompletionFault {
    inner: Arc<dyn StateStore>,
    armed: AtomicBool,
    pub failures: AtomicUsize,
}
impl CompletionFault {
    pub fn new(inner: Arc<dyn StateStore>) -> Self {
        Self {
            inner,
            armed: AtomicBool::new(false),
            failures: AtomicUsize::new(0),
        }
    }
    pub fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }
}
#[async_trait::async_trait]
impl StateStore for CompletionFault {
    async fn compare_and_delete(
        &self,
        key: &StateKey,
        expected_version: u64,
    ) -> Result<bool, StateError> {
        self.inner.compare_and_delete(key, expected_version).await
    }
    async fn compare_and_swap(
        &self,
        key: &StateKey,
        expected_version: u64,
        new_value: &str,
        ttl: Option<Duration>,
    ) -> Result<CasResult, StateError> {
        let is_completion = key.kind == KeyKind::ScheduledAction
            && serde_json::from_str::<serde_json::Value>(new_value)
                .is_ok_and(|row| row.get("completed_at").is_some_and(|at| !at.is_null()));
        if is_completion && self.armed.swap(false, Ordering::SeqCst) {
            self.failures.fetch_add(1, Ordering::SeqCst);
            return Err(StateError::Connection(
                "injected scheduled-outcome write outage".into(),
            ));
        }
        self.inner
            .compare_and_swap(key, expected_version, new_value, ttl)
            .await
    }
    async fn check_and_set(
        &self,
        key: &StateKey,
        value: &str,
        ttl: Option<Duration>,
    ) -> Result<bool, StateError> {
        self.inner.check_and_set(key, value, ttl).await
    }
    async fn get(&self, key: &StateKey) -> Result<Option<String>, StateError> {
        self.inner.get(key).await
    }
    async fn get_versioned(&self, key: &StateKey) -> Result<Option<(String, u64)>, StateError> {
        self.inner.get_versioned(key).await
    }
    async fn set(
        &self,
        key: &StateKey,
        value: &str,
        ttl: Option<Duration>,
    ) -> Result<(), StateError> {
        self.inner.set(key, value, ttl).await
    }
    async fn delete(&self, key: &StateKey) -> Result<bool, StateError> {
        self.inner.delete(key).await
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
        self.inner.index_timeout(key, expires_at_ms).await
    }
    async fn remove_timeout_index(&self, key: &StateKey) -> Result<(), StateError> {
        self.inner.remove_timeout_index(key).await
    }
    async fn get_expired_timeouts(&self, now_ms: i64) -> Result<Vec<String>, StateError> {
        self.inner.get_expired_timeouts(now_ms).await
    }
    async fn index_chain_ready(&self, key: &StateKey, ready_at_ms: i64) -> Result<(), StateError> {
        self.inner.index_chain_ready(key, ready_at_ms).await
    }
    async fn remove_chain_ready_index(&self, key: &StateKey) -> Result<(), StateError> {
        self.inner.remove_chain_ready_index(key).await
    }
    async fn get_ready_chains(&self, now_ms: i64) -> Result<Vec<String>, StateError> {
        self.inner.get_ready_chains(now_ms).await
    }
}
