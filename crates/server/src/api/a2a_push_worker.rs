//! A2A push-notification delivery worker (Phase 4.2).
//!
//! Delivers the Task envelope's ordered callback log under CAS-fenced leases.
//! The gateway broadcast is a low-latency wake-up only; bounded cross-scope
//! Task pages recover work after restart, lag, or a missed wake-up.
//!
//! Architecture (post-#178 rebuild — concurrency + cache + retry
//! refinement):
//!
//! - The broadcast loop is non-blocking: a qualifying event is handed
//!   off to a `tokio::spawn`'d dispatch task and the loop immediately
//!   returns to draining the receiver. A slow webhook can no longer
//!   stall the loop and starve every other tenant.
//! - Per-delivery concurrency is bounded by an `Arc<Semaphore>` so
//!   an external endpoint's behaviour cannot blow up the worker's
//!   memory or fd budget. The default cap is
//!   `MAX_INFLIGHT_DELIVERIES` (private module constant).
//! - Config lookups go through a short-TTL cache so a burst of events
//!   for the same task (e.g. 50 artifact chunks) doesn't hammer the
//!   state store with redundant `scan_keys`. TTL is intentionally
//!   short so a `DELETE` of a config takes effect fast enough not to
//!   surprise users.
//! - Retry attempts, deadlines, acknowledgements, and dead letters are stored
//!   in the same Task envelope as the event. The transient-vs-terminal classification
//!   treats `408`, `425`, and `429` as transient — they are exactly
//!   the codes a well-behaved server uses to ask the client to back
//!   off, so retrying is correct.
//! - Terminal and exhausted failures block the callback at its failed sequence
//!   and land in the Task envelope's bounded dead-letter state.
//!
//! The HTTP client is provided by the caller — typically the shared
//! `reqwest::Client` `main.rs` builds once and threads through every
//! provider, so connection pooling and any mTLS or proxy settings are
//! shared with the rest of the server.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use acteon_core::{DlqFailureKind, StreamEvent, TaskPushNotificationConfig};
use acteon_gateway::{TaskEngine, TaskPushDeliveryClaim, TaskPushReadyHint, TaskScope};
use acteon_state::{KeyKind, StateKey, StateStore};
use tokio::sync::{Mutex, Semaphore, broadcast};
use tracing::{debug, error, warn};

/// `action_type` value the gateway stamps on every A2A task event.
/// Used to filter the broadcast subscriber down to push-relevant
/// events.
const A2A_TASK_ACTION_TYPE: &str = "a2a.task";

/// Number of retry attempts per delivery, *including* the initial
/// send. Three attempts with the backoff below gives an upper bound
/// of `1s + 2s = 3s` of waiting per exhausted delivery.
const MAX_DELIVERY_ATTEMPTS: usize = 3;

/// Base delay for the exponential backoff between transient-failure
/// retries. Attempt `n` (zero-indexed) waits `BASE * 2^n` before
/// retrying, capped by [`MAX_BACKOFF`].
const BASE_BACKOFF: Duration = Duration::from_secs(1);

/// Hard cap on the backoff between retries. Stops `BASE * 2^n` from
/// growing unboundedly if `MAX_DELIVERY_ATTEMPTS` is later raised.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Per-request timeout for one delivery attempt.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A claim comfortably outlives one request timeout. If a process stalls
/// longer, another replica takes over and the stable event ID makes the
/// resulting at-least-once duplicate identifiable.
const DELIVERY_LEASE: Duration = Duration::from_secs(60);

const READY_RECOVERY_INTERVAL: Duration = Duration::from_secs(1);
const FULL_REPAIR_INTERVAL: Duration = Duration::from_secs(5);
const RECOVERY_PAGE_SIZE: usize = 128;

/// Cap on in-flight HTTP deliveries across **all** tasks and
/// configs. A misbehaving destination can stall up to this many
/// concurrent slots, but cannot block the broadcast loop or the rest
/// of the worker's deliveries past this number.
const MAX_INFLIGHT_DELIVERIES: usize = 64;

/// How long a `(namespace, tenant, task_id) -> Vec<config>` lookup
/// stays cached. Short enough that a `DELETE /pushNotificationConfigs`
/// takes effect fast enough not to surprise users; long enough to
/// fold a burst of events for the same task into a single state-store
/// scan.
const CONFIG_CACHE_TTL: Duration = Duration::from_millis(500);

#[cfg(test)]
fn project_push_payload(
    task: &acteon_core::Task,
    event: &StreamEvent,
) -> Option<serde_json::Value> {
    match &event.event_type {
        acteon_core::StreamEventType::TaskTransitioned { to, .. } => {
            super::a2a::project_v1_status_update(task, *to, event.timestamp)
        }
        acteon_core::StreamEventType::TaskArtifactUpdated {
            artifact_id,
            last_chunk,
            ..
        } => super::a2a::project_v1_artifact_update(task, artifact_id, *last_chunk),
        _ => None,
    }
}

/// HTTP status codes that the spec or convention designates as "ask
/// for retry, do not give up." Everything else in 4xx is treated as
/// terminal (the payload is permanently rejected). Critically this
/// includes `429 Too Many Requests` — without this, a server asking
/// us to back off would silently kill its own subscription.
fn is_transient_client_error(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 425 | 429)
}

/// Counters exposed for observability. Updated on the hot path with
/// `Ordering::Relaxed`; reading produces a *snapshot* that is not
/// internally consistent across counters (this is fine for the
/// "what's the failure rate?" use case).
#[derive(Debug, Default)]
pub struct PushDeliveryMetrics {
    /// Total A2A-tagged events the broadcast loop has accepted for
    /// dispatch. Does not double-count non-A2A events.
    pub events_dispatched: AtomicU64,
    /// Events skipped because the broadcast loop saw `Lagged(n)` —
    /// `n` per skip is added on each occurrence.
    pub events_lagged: AtomicU64,
    /// Total delivery attempts started (one per `post_once` call).
    pub deliveries_attempted: AtomicU64,
    /// Successful deliveries (HTTP 2xx).
    pub deliveries_succeeded: AtomicU64,
    /// Deliveries permanently rejected (HTTP 4xx outside the
    /// transient set). The retry loop stops on these.
    pub deliveries_terminal: AtomicU64,
    /// Deliveries that exhausted the per-call retry budget without
    /// succeeding. Each one writes a DLQ entry — see
    /// [`Self::dlq_writes`].
    pub deliveries_exhausted: AtomicU64,
    /// `scan_keys` calls actually issued to the state store after
    /// the config-cache check. The cache hit rate is
    /// `1 - state_store_scans / events_dispatched` (approximately).
    pub state_store_scans: AtomicU64,
    /// DLQ entries successfully written to the state store. Lower
    /// than `deliveries_terminal + deliveries_exhausted` when the
    /// state store itself rejects a DLQ write — see
    /// [`Self::dlq_write_failures`].
    pub dlq_writes: AtomicU64,
    /// DLQ entries the worker tried to write but failed (state
    /// store error, serialization error). Logged with `warn!` so an
    /// operator can investigate.
    pub dlq_write_failures: AtomicU64,
    /// Deliveries refused by the SSRF guard because the config URL
    /// resolved to a blocked (loopback / private / metadata)
    /// address. A subset of `deliveries_terminal` — a non-zero rate
    /// here means a tenant is probing the internal network and is
    /// worth alerting on.
    pub deliveries_ssrf_blocked: AtomicU64,
}

impl PushDeliveryMetrics {
    /// Take an internally-non-consistent snapshot. Cheap (six
    /// relaxed loads) and useful for the eventual `/metrics`
    /// exporter.
    #[must_use]
    pub fn snapshot(&self) -> PushDeliveryMetricsSnapshot {
        PushDeliveryMetricsSnapshot {
            events_dispatched: self.events_dispatched.load(Ordering::Relaxed),
            events_lagged: self.events_lagged.load(Ordering::Relaxed),
            deliveries_attempted: self.deliveries_attempted.load(Ordering::Relaxed),
            deliveries_succeeded: self.deliveries_succeeded.load(Ordering::Relaxed),
            deliveries_terminal: self.deliveries_terminal.load(Ordering::Relaxed),
            deliveries_exhausted: self.deliveries_exhausted.load(Ordering::Relaxed),
            state_store_scans: self.state_store_scans.load(Ordering::Relaxed),
            dlq_writes: self.dlq_writes.load(Ordering::Relaxed),
            dlq_write_failures: self.dlq_write_failures.load(Ordering::Relaxed),
            deliveries_ssrf_blocked: self.deliveries_ssrf_blocked.load(Ordering::Relaxed),
        }
    }
}

/// Snapshot of [`PushDeliveryMetrics`]. Exposed as a plain struct
/// for test assertions and the future Prometheus exporter.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PushDeliveryMetricsSnapshot {
    pub events_dispatched: u64,
    pub events_lagged: u64,
    pub deliveries_attempted: u64,
    pub deliveries_succeeded: u64,
    pub deliveries_terminal: u64,
    pub deliveries_exhausted: u64,
    pub state_store_scans: u64,
    pub dlq_writes: u64,
    pub dlq_write_failures: u64,
    pub deliveries_ssrf_blocked: u64,
}

/// Internal state held behind an `Arc` and shared across every
/// spawned dispatch + delivery task.
struct Shared {
    state: Arc<dyn StateStore>,
    http: acteon_http::GuardedClient,
    worker_id: String,
    delivery_permits: Arc<Semaphore>,
    cache: Mutex<HashMap<CacheKey, CacheEntry>>,
    metrics: Arc<PushDeliveryMetrics>,
    /// When `true` (the production default), every delivery URL is
    /// run through the DNS-resolving SSRF guard before the POST.
    /// Tests and the in-process simulation flip it off because they
    /// deliver to `127.0.0.1` mock servers. An `AtomicBool` so the
    /// `with_ssrf_enforcement` builder can flip it through the
    /// already-`Arc`'d `Shared`.
    enforce_ssrf: AtomicBool,
}

type CacheKey = (String, String, String);

struct CacheEntry {
    inserted_at: Instant,
    configs: Arc<Vec<TaskPushNotificationConfig>>,
}

/// The push-delivery worker. Construct one per server, attach its
/// [`broadcast::Receiver`] and the shared HTTP client, then `spawn`
/// it.
pub struct PushDeliveryWorker {
    shared: Arc<Shared>,
    rx: broadcast::Receiver<StreamEvent>,
}

impl PushDeliveryWorker {
    /// Build a new worker. `rx` should be freshly subscribed from
    /// the gateway's `stream_tx()` so no in-flight events are
    /// missed; the caller is responsible for the lifetime ordering.
    #[must_use]
    pub fn new(
        state: Arc<dyn StateStore>,
        http: acteon_http::GuardedClient,
        rx: broadcast::Receiver<StreamEvent>,
    ) -> Self {
        Self::with_metrics(state, http, rx, Arc::new(PushDeliveryMetrics::default()))
    }

    /// As [`Self::new`], but with a caller-owned metrics handle so
    /// the metrics can be read out by an exporter / test harness.
    #[must_use]
    pub fn with_metrics(
        state: Arc<dyn StateStore>,
        http: acteon_http::GuardedClient,
        rx: broadcast::Receiver<StreamEvent>,
        metrics: Arc<PushDeliveryMetrics>,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                state,
                http,
                worker_id: uuid::Uuid::now_v7().to_string(),
                delivery_permits: Arc::new(Semaphore::new(MAX_INFLIGHT_DELIVERIES)),
                cache: Mutex::new(HashMap::new()),
                metrics,
                // Secure by default — the production worker built in
                // `main.rs` keeps this on.
                enforce_ssrf: AtomicBool::new(true),
            }),
            rx,
        }
    }

    /// Builder: turn the SSRF guard off. Used by the worker's own
    /// tests and the in-process simulation, both of which deliver to
    /// `127.0.0.1` mock servers the guard would otherwise (correctly)
    /// reject. **Never call this in production** — a delivery URL is
    /// attacker-controlled.
    #[must_use]
    pub fn with_ssrf_enforcement(self, enforce: bool) -> Self {
        self.shared.enforce_ssrf.store(enforce, Ordering::Relaxed);
        self
    }

    /// Borrow the metrics handle. Useful from tests and from the
    /// eventual Prometheus exporter.
    #[must_use]
    pub fn metrics(&self) -> Arc<PushDeliveryMetrics> {
        Arc::clone(&self.shared.metrics)
    }

    /// Drive the worker until the broadcast channel closes (server
    /// shutdown). Intended to be `tokio::spawn`'d at startup.
    ///
    /// Broadcasts trigger a fast, detached drain. A periodic bounded page
    /// scan repairs missed wake-ups and resumes work after restart.
    pub async fn run(mut self) {
        let mut ready_recovery = tokio::time::interval(READY_RECOVERY_INTERVAL);
        ready_recovery.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut full_repair = tokio::time::interval(FULL_REPAIR_INTERVAL);
        full_repair.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut ready_cursor = None;
        let mut repair_cursor = None;
        loop {
            tokio::select! {
                received = self.rx.recv() => match received {
                    Ok(event) => {
                        if event.action_type.as_deref() != Some(A2A_TASK_ACTION_TYPE) {
                            continue;
                        }
                        self.shared
                            .metrics
                            .events_dispatched
                            .fetch_add(1, Ordering::Relaxed);
                        let Some(task_id) = event.action_id else {
                            continue;
                        };
                        let shared = Arc::clone(&self.shared);
                        tokio::spawn(async move {
                            Shared::dispatch_task(shared, event.namespace, event.tenant, task_id).await;
                        });
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        self.shared
                            .metrics
                            .events_lagged
                            .fetch_add(n, Ordering::Relaxed);
                        warn!(skipped = n, "A2A push worker lagged; durable recovery will repair it");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!("A2A push worker: broadcast closed, shutting down");
                        return;
                    }
                },
                _ = ready_recovery.tick() => {
                    match self.shared.recover_ready_page(ready_cursor.as_deref()).await {
                        Ok(next) => ready_cursor = next,
                        Err(error) => warn!(%error, "A2A push ready-index page failed"),
                    }
                }
                _ = full_repair.tick() => {
                    match self.shared.recover_task_page(repair_cursor.as_deref()).await {
                        Ok(next) => repair_cursor = next,
                        Err(error) => warn!(%error, "A2A push Task repair page failed"),
                    }
                }
            }
        }
    }
}

impl Shared {
    async fn recover_ready_page(
        self: &Arc<Self>,
        after: Option<&str>,
    ) -> Result<Option<String>, String> {
        let page = self
            .state
            .scan_keys_by_kind_page(KeyKind::A2aPushReady, after, RECOVERY_PAGE_SIZE)
            .await
            .map_err(|error| error.to_string())?;
        let now = TaskEngine::new(self.state.clone()).clock().now();
        for (key, raw) in page.entries {
            let hint = match serde_json::from_str::<TaskPushReadyHint>(&raw) {
                Ok(hint) => hint,
                Err(error) => {
                    warn!(%key, %error, "A2A push ready index contains malformed state");
                    continue;
                }
            };
            let expected = StateKey::new(
                hint.namespace.as_str(),
                hint.tenant.as_str(),
                KeyKind::A2aPushReady,
                &hint.task_id,
            )
            .canonical();
            if key != expected {
                warn!(%key, "A2A push ready hint identity mismatch");
                continue;
            }
            if hint.ready_at <= now {
                let shared = Arc::clone(self);
                tokio::spawn(async move {
                    Shared::dispatch_task(shared, hint.namespace, hint.tenant, hint.task_id).await;
                });
            }
        }
        Ok(page.next_cursor)
    }

    async fn recover_task_page(
        self: &Arc<Self>,
        after: Option<&str>,
    ) -> Result<Option<String>, String> {
        let page = self
            .state
            .scan_keys_by_kind_page(KeyKind::A2aTask, after, RECOVERY_PAGE_SIZE)
            .await
            .map_err(|error| error.to_string())?;
        for (key, _) in page.entries {
            let parts: Vec<_> = key.splitn(4, ':').collect();
            if parts.len() != 4 || parts[2] != KeyKind::A2aTask.to_string() {
                warn!(%key, "A2A push recovery skipped malformed task key");
                continue;
            }
            let shared = Arc::clone(self);
            let namespace = parts[0].to_string();
            let tenant = parts[1].to_string();
            let task_id = parts[3].to_string();
            tokio::spawn(async move {
                let scope = TaskScope::new(&namespace, &tenant);
                let engine = TaskEngine::new(shared.state.clone());
                match engine.repair_push_ready_hint(&scope, &task_id).await {
                    Ok(Some(hint)) if hint.ready_at <= engine.clock().now() => {
                        Shared::dispatch_task(shared, namespace, tenant, task_id).await;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        warn!(%error, namespace, tenant, task_id, "A2A push ready hint repair failed");
                    }
                }
            });
        }
        Ok(page.next_cursor)
    }

    /// Wake every callback registered for one Task. Claiming inside the Task
    /// CAS determines ownership and ordering; duplicate wake-ups are harmless.
    async fn dispatch_task(self: Arc<Self>, namespace: String, tenant: String, task_id: String) {
        if task_id.is_empty() {
            return;
        }
        let configs = match self
            .load_configs_cached(&namespace, &tenant, &task_id)
            .await
        {
            Ok(c) => c,
            Err(e) => {
                warn!(
                    error = %e,
                    namespace,
                    tenant,
                    task_id,
                    "A2A push worker: config lookup failed",
                );
                return;
            }
        };
        if configs.is_empty() {
            return;
        }
        for config in configs.iter() {
            let shared = Arc::clone(&self);
            let namespace = namespace.clone();
            let tenant = tenant.clone();
            let task_id = task_id.clone();
            let config_id = config.id.clone();
            tokio::spawn(async move {
                shared
                    .drain_callback(namespace, tenant, task_id, config_id)
                    .await;
            });
        }
    }

    /// Cached wrapper around [`Self::load_configs`]. Cache TTL is
    /// [`CONFIG_CACHE_TTL`] — short enough for `DELETE` to take
    /// effect quickly, long enough to fold a burst of events for the
    /// same task into a single scan.
    async fn load_configs_cached(
        &self,
        namespace: &str,
        tenant: &str,
        task_id: &str,
    ) -> Result<Arc<Vec<TaskPushNotificationConfig>>, String> {
        let key: CacheKey = (
            namespace.to_string(),
            tenant.to_string(),
            task_id.to_string(),
        );
        // Try cache hit first.
        {
            let cache = self.cache.lock().await;
            if let Some(entry) = cache.get(&key)
                && entry.inserted_at.elapsed() < CONFIG_CACHE_TTL
            {
                return Ok(Arc::clone(&entry.configs));
            }
        }
        // Miss — go to the store. Two concurrent misses for the
        // same key will each issue a scan; that's fine in v1.
        let configs = Arc::new(self.load_configs(namespace, tenant, task_id).await?);
        let mut cache = self.cache.lock().await;
        // Best-effort cache eviction of stale entries while we hold
        // the lock anyway. Linear in cache size; the cache stays
        // small because the TTL is tight.
        let now = Instant::now();
        cache.retain(|_, e| now.duration_since(e.inserted_at) < CONFIG_CACHE_TTL);
        cache.insert(
            key,
            CacheEntry {
                inserted_at: now,
                configs: Arc::clone(&configs),
            },
        );
        Ok(configs)
    }

    /// The state-store side of the config lookup — bumps the
    /// `state_store_scans` counter every time it's reached.
    async fn load_configs(
        &self,
        namespace: &str,
        tenant: &str,
        task_id: &str,
    ) -> Result<Vec<TaskPushNotificationConfig>, String> {
        self.metrics
            .state_store_scans
            .fetch_add(1, Ordering::Relaxed);
        let scope = TaskScope::new(namespace, tenant);
        let engine = TaskEngine::new(self.state.clone());
        if !engine
            .push_config_migration_complete(&scope, task_id)
            .await
            .map_err(|error| error.to_string())?
        {
            let prefix = format!("{task_id}:");
            let entries = self
                .state
                .scan_keys(namespace, tenant, KeyKind::A2aTaskPushConfig, Some(&prefix))
                .await
                .map_err(|error| error.to_string())?;
            let mut legacy = Vec::with_capacity(entries.len());
            for (_, raw) in entries {
                legacy.push(
                    serde_json::from_str::<TaskPushNotificationConfig>(&raw)
                        .map_err(|_| "malformed stored push configuration".to_string())?,
                );
            }
            engine
                .import_legacy_push_configs(&scope, task_id, legacy)
                .await
                .map_err(|error| error.to_string())?;
        }
        let out = engine
            .list_push_configs(&scope, task_id)
            .await
            .map_err(|error| error.to_string())?;
        for config in &out {
            if config.namespace != namespace
                || config.tenant != tenant
                || config.task_id != task_id
                || config.validate().is_err()
            {
                return Err("stored push configuration failed identity or validation".into());
            }
        }
        Ok(out)
    }

    /// Drain one callback in strict sequence order. State transitions happen
    /// through fenced Task CAS operations; this method holds only a process
    /// semaphore permit and therefore remains safe across replicas.
    async fn drain_callback(
        &self,
        namespace: String,
        tenant: String,
        task_id: String,
        config_id: String,
    ) {
        let Ok(_permit) = self.delivery_permits.acquire().await else {
            return;
        };
        let scope = TaskScope::new(namespace, tenant);
        let engine = TaskEngine::new(self.state.clone());
        loop {
            let claim = match engine
                .claim_push_delivery(
                    &scope,
                    &task_id,
                    &config_id,
                    &self.worker_id,
                    DELIVERY_LEASE,
                )
                .await
            {
                Ok(Some(claim)) => claim,
                Ok(None) => return,
                Err(error) => {
                    warn!(%error, task_id, config_id, "A2A push claim failed");
                    return;
                }
            };
            self.metrics
                .deliveries_attempted
                .fetch_add(1, Ordering::Relaxed);
            if !self
                .complete_claim(&engine, &scope, &task_id, &config_id, claim)
                .await
            {
                return;
            }
        }
    }

    /// Apply one HTTP outcome to its fenced claim. `true` means the
    /// acknowledgement committed and the caller may claim the next sequence.
    async fn complete_claim(
        &self,
        engine: &TaskEngine,
        scope: &TaskScope,
        task_id: &str,
        config_id: &str,
        claim: TaskPushDeliveryClaim,
    ) -> bool {
        match self
            .post_once(
                &claim.config,
                &claim.payload,
                &claim.event_id,
                claim.sequence,
            )
            .await
        {
            Ok(()) => {
                self.metrics
                    .deliveries_succeeded
                    .fetch_add(1, Ordering::Relaxed);
                if let Err(error) = engine
                    .acknowledge_push_delivery(scope, task_id, config_id, &claim.lease_id)
                    .await
                {
                    warn!(%error, task_id, config_id, sequence = claim.sequence, "A2A push acknowledgement was not committed");
                    return false;
                }
                debug!(
                    task_id,
                    config_id,
                    sequence = claim.sequence,
                    attempt = claim.attempt,
                    "A2A push delivered and acknowledged"
                );
                true
            }
            Err(DeliveryError::Terminal(reason)) => {
                self.metrics
                    .deliveries_terminal
                    .fetch_add(1, Ordering::Relaxed);
                error!(
                    task_id,
                    config_id,
                    sequence = claim.sequence,
                    attempt = claim.attempt,
                    "A2A push permanently rejected"
                );
                self.persist_dead_letter(engine, scope, &claim, DlqFailureKind::Terminal, &reason)
                    .await;
                false
            }
            Err(DeliveryError::Transient(reason)) => {
                if usize::try_from(claim.attempt).unwrap_or(usize::MAX) >= MAX_DELIVERY_ATTEMPTS {
                    self.metrics
                        .deliveries_exhausted
                        .fetch_add(1, Ordering::Relaxed);
                    error!(
                        task_id,
                        config_id,
                        sequence = claim.sequence,
                        attempt = claim.attempt,
                        "A2A push exhausted retries"
                    );
                    self.persist_dead_letter(
                        engine,
                        scope,
                        &claim,
                        DlqFailureKind::Exhausted,
                        &reason,
                    )
                    .await;
                    return false;
                }
                let attempt_index =
                    usize::try_from(claim.attempt.saturating_sub(1)).unwrap_or(usize::MAX);
                let delay = backoff(attempt_index);
                let retry_at = engine.clock().now()
                    + chrono::Duration::from_std(delay).unwrap_or(chrono::Duration::MAX);
                if let Err(error) = engine
                    .retry_push_delivery_at(scope, task_id, config_id, &claim.lease_id, retry_at)
                    .await
                {
                    warn!(%error, task_id, config_id, sequence = claim.sequence, "A2A push retry deadline was not committed");
                }
                false
            }
        }
    }

    async fn persist_dead_letter(
        &self,
        engine: &TaskEngine,
        scope: &TaskScope,
        claim: &TaskPushDeliveryClaim,
        failure_kind: DlqFailureKind,
        last_error: &str,
    ) {
        match engine
            .dead_letter_push_delivery(
                scope,
                &claim.task_id,
                &claim.config.id,
                &claim.lease_id,
                failure_kind,
                last_error,
            )
            .await
        {
            Ok(entry) => {
                self.metrics.dlq_writes.fetch_add(1, Ordering::Relaxed);
                debug!(
                    task_id = claim.task_id,
                    config_id = claim.config.id,
                    entry_id = entry.id,
                    "A2A push dead letter committed"
                );
            }
            Err(error) => {
                self.metrics
                    .dlq_write_failures
                    .fetch_add(1, Ordering::Relaxed);
                warn!(%error, task_id = claim.task_id, config_id = claim.config.id, "A2A push dead letter was not committed");
            }
        }
    }

    /// Send one POST. Maps the result into the worker's transient /
    /// terminal classification. Network and timeout failures are
    /// transient; HTTP 429 / 408 / 425 are transient (per spec and
    /// convention — these are the codes a server uses to ask for a
    /// retry); other 4xx are terminal; everything 5xx is transient.
    ///
    /// Before any of that, the SSRF guard re-checks the URL with DNS
    /// resolution. A config can be edited (or a hostname can be
    /// re-pointed) after registration, so the delivery-time check is
    /// the authoritative one. A blocked URL is a `Terminal` failure:
    /// it will not become un-blocked, so retrying is pointless, and
    /// the DLQ entry records exactly why it was refused.
    async fn post_once(
        &self,
        config: &TaskPushNotificationConfig,
        payload: &serde_json::Value,
        event_id: &str,
        sequence: u64,
    ) -> Result<(), DeliveryError> {
        if self.enforce_ssrf.load(Ordering::Relaxed)
            && let Err(reason) = super::a2a_ssrf::check_url_resolved(&config.url).await
        {
            self.metrics
                .deliveries_ssrf_blocked
                .fetch_add(1, Ordering::Relaxed);
            return Err(DeliveryError::Terminal(format!(
                "SSRF guard refused delivery URL: {reason}"
            )));
        }
        let request = self.build_request(config, payload, event_id, sequence)?;
        let resp = request.send().await.map_err(|e| {
            // Network / timeout / serialization — treat as transient.
            DeliveryError::Transient(e.to_string())
        })?;
        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else if status.is_client_error() {
            if is_transient_client_error(status) {
                Err(DeliveryError::Transient(format!("HTTP {status}")))
            } else {
                Err(DeliveryError::Terminal(format!("HTTP {status}")))
            }
        } else {
            // 5xx, 3xx-without-follow, or any non-success
            // non-client-error: treat as transient.
            Err(DeliveryError::Transient(format!("HTTP {status}")))
        }
    }

    fn build_request(
        &self,
        config: &TaskPushNotificationConfig,
        payload: &serde_json::Value,
        event_id: &str,
        sequence: u64,
    ) -> Result<reqwest::RequestBuilder, DeliveryError> {
        let mut request = self
            .http
            .post(&config.url)
            .map_err(|e| DeliveryError::Terminal(e.to_string()))?
            .timeout(REQUEST_TIMEOUT)
            .header(reqwest::header::CONTENT_TYPE, "application/a2a+json")
            .header("a2a-version", super::a2a::A2A_PROTOCOL_VERSION)
            .header("x-acteon-delivery-id", event_id)
            .header("x-acteon-delivery-sequence", sequence.to_string())
            .json(payload);
        if let Some(token) = &config.token {
            request = request.header("x-a2a-notification-token", token);
        }
        if let Some(authentication) = &config.authentication
            && let ([scheme], Some(credentials)) = (
                authentication.schemes.as_slice(),
                &authentication.credentials,
            )
        {
            let value = reqwest::header::HeaderValue::from_str(&format!("{scheme} {credentials}"))
                .map_err(|_| DeliveryError::Terminal("invalid push authentication".into()))?;
            request = request.header(reqwest::header::AUTHORIZATION, value);
        }
        Ok(request)
    }
}

/// Internal classification of a single-attempt delivery failure.
/// Terminal failures stop the retry loop; transient ones back off
/// and try again.
#[derive(Debug)]
enum DeliveryError {
    Terminal(String),
    Transient(String),
}

/// Exponential backoff for retry attempt `n` (zero-indexed), capped
/// at [`MAX_BACKOFF`]. Pure function — split out so the cap behavior
/// is unit-testable without spawning the worker.
fn backoff(attempt: usize) -> Duration {
    let scale = 1u64 << attempt.min(31); // saturate the shift, not the value
    let raw = BASE_BACKOFF.saturating_mul(u32::try_from(scale).unwrap_or(u32::MAX));
    raw.min(MAX_BACKOFF)
}

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_core::{PushAuthentication, StreamEventType, Task, TaskState};
    use acteon_state::{
        StateKey, StateStore,
        testing::faults::{FaultStore, FaultTiming, WriteOperation},
    };
    use acteon_state_memory::MemoryStateStore;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn mk_event(task_id: &str) -> StreamEvent {
        StreamEvent {
            id: uuid::Uuid::now_v7().to_string(),
            timestamp: chrono::Utc::now(),
            event_type: StreamEventType::TaskTransitioned {
                task_id: task_id.to_string(),
                from: TaskState::Submitted,
                to: TaskState::Working,
            },
            namespace: "agents".to_string(),
            tenant: "demo".to_string(),
            action_type: Some(A2A_TASK_ACTION_TYPE.to_string()),
            action_id: Some(task_id.to_string()),
        }
    }

    async fn save_config(store: &Arc<dyn StateStore>, c: &TaskPushNotificationConfig) {
        let engine = TaskEngine::new(Arc::clone(store));
        let scope = TaskScope::new(&c.namespace, &c.tenant);
        if engine.get_task(&scope, &c.task_id).await.unwrap().is_none() {
            engine
                .create_task(Task::new(&c.task_id, &c.namespace, &c.tenant))
                .await
                .unwrap();
        }
        engine
            .upsert_push_config(&scope, &c.task_id, c.clone())
            .await
            .unwrap();
    }

    async fn persist_transition(store: &Arc<dyn StateStore>, task_id: &str) -> StreamEvent {
        TaskEngine::new(Arc::clone(store))
            .transition_task(
                &TaskScope::new("agents", "demo"),
                task_id,
                TaskState::Working,
                None,
            )
            .await
            .unwrap();
        mk_event(task_id)
    }

    /// Mock that responds with `status` after a configurable delay,
    /// counting hits and recording connections.
    async fn start_mock_with_delay(status: u16, delay: Duration, hits: Arc<AtomicUsize>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let hits_inner = Arc::clone(&hits);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let _ = stream.read(&mut buf).await;
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                    hits_inner.fetch_add(1, Ordering::Relaxed);
                    let body = "{}";
                    let response = format!(
                        "HTTP/1.1 {status} OK\r\n\
                         Content-Type: application/json\r\n\
                         Content-Length: {len}\r\n\
                         Connection: close\r\n\
                         \r\n{body}",
                        len = body.len(),
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        format!("http://{addr}")
    }

    async fn start_mock(status: u16, hits: Arc<AtomicUsize>) -> String {
        start_mock_with_delay(status, Duration::ZERO, hits).await
    }

    #[derive(Debug, PartialEq, Eq)]
    struct CapturedDeliveryHeaders {
        token: Option<String>,
        authorization: Option<String>,
        delivery_id: String,
        sequence: String,
    }

    async fn start_header_capture_mock(
        captures: Arc<Mutex<Vec<CapturedDeliveryHeaders>>>,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let captures = Arc::clone(&captures);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let read = stream.read(&mut buf).await.unwrap();
                    let request = String::from_utf8_lossy(&buf[..read]);
                    let header = |name: &str| {
                        request.lines().find_map(|line| {
                            let (candidate, value) = line.split_once(':')?;
                            candidate
                                .eq_ignore_ascii_case(name)
                                .then(|| value.trim().to_owned())
                        })
                    };
                    captures.lock().await.push(CapturedDeliveryHeaders {
                        token: header("x-a2a-notification-token"),
                        authorization: header("authorization"),
                        delivery_id: header("x-acteon-delivery-id").unwrap(),
                        sequence: header("x-acteon-delivery-sequence").unwrap(),
                    });
                    let body = "{}";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\n\
                         Content-Type: application/json\r\n\
                         Content-Length: {len}\r\n\
                         Connection: close\r\n\
                         \r\n{body}",
                        len = body.len(),
                    );
                    stream.write_all(response.as_bytes()).await.unwrap();
                    stream.shutdown().await.unwrap();
                });
            }
        });
        format!("http://{addr}")
    }

    /// Mock that returns `first_status` once, then `200` afterward.
    /// Used for transient-retry tests.
    async fn start_mock_first_then_ok(first_status: u16, hits: Arc<AtomicUsize>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut count = 0usize;
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                count += 1;
                hits.fetch_add(1, Ordering::Relaxed);
                let mut buf = vec![0u8; 8192];
                let _ = stream.read(&mut buf).await;
                let status = if count == 1 { first_status } else { 200 };
                let body = "{}";
                let response = format!(
                    "HTTP/1.1 {status} OK\r\n\
                     Content-Type: application/json\r\n\
                     Content-Length: {len}\r\n\
                     Connection: close\r\n\
                     \r\n{body}",
                    len = body.len(),
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        format!("http://{addr}")
    }

    // The worker detaches delivery tasks, so joining its receive loop does
    // not prove delivery completed. Wait for settlement before test teardown.
    async fn wait_for_deliveries(metrics: &PushDeliveryMetrics, expected: u64) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while metrics.snapshot().deliveries_succeeded < expected {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("push deliveries did not settle before the deadline");
        assert_eq!(metrics.snapshot().deliveries_succeeded, expected);
    }

    #[test]
    fn backoff_doubles_then_caps() {
        assert_eq!(backoff(0), Duration::from_secs(1));
        assert_eq!(backoff(1), Duration::from_secs(2));
        assert_eq!(backoff(2), Duration::from_secs(4));
        assert_eq!(backoff(10), MAX_BACKOFF);
        // Saturation: very large attempts must not panic.
        assert_eq!(backoff(usize::MAX), MAX_BACKOFF);
    }

    #[test]
    fn is_transient_client_error_lists_408_425_429() {
        assert!(is_transient_client_error(
            reqwest::StatusCode::REQUEST_TIMEOUT
        ));
        assert!(is_transient_client_error(reqwest::StatusCode::TOO_EARLY));
        assert!(is_transient_client_error(
            reqwest::StatusCode::TOO_MANY_REQUESTS
        ));
        // Other 4xx must remain terminal.
        assert!(!is_transient_client_error(reqwest::StatusCode::BAD_REQUEST));
        assert!(!is_transient_client_error(
            reqwest::StatusCode::UNAUTHORIZED
        ));
        assert!(!is_transient_client_error(reqwest::StatusCode::FORBIDDEN));
        assert!(!is_transient_client_error(reqwest::StatusCode::NOT_FOUND));
        assert!(!is_transient_client_error(reqwest::StatusCode::CONFLICT));
        assert!(!is_transient_client_error(reqwest::StatusCode::GONE));
    }

    #[test]
    fn callback_payload_is_an_official_stream_response() {
        let task = Task::new("task-1", "agents", "demo");
        let payload = project_push_payload(&task, &mk_event("task-1")).unwrap();
        assert_eq!(payload["statusUpdate"]["taskId"], "task-1");
        assert_eq!(
            payload["statusUpdate"]["status"]["state"],
            "TASK_STATE_WORKING"
        );
        assert!(payload.get("namespace").is_none());
        assert!(payload.get("tenant").is_none());
        assert!(payload.get("event_type").is_none());
    }

    #[tokio::test]
    async fn worker_imports_legacy_standalone_configs_once() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let task = Task::new("legacy-task", "agents", "demo");
        let scope = TaskScope::new("agents", "demo");
        store
            .set(
                &StateKey::new("agents", "demo", KeyKind::A2aTask, &task.id),
                &serde_json::to_string(&task).unwrap(),
                None,
            )
            .await
            .unwrap();
        let config = TaskPushNotificationConfig::new(
            "legacy-config",
            &task.id,
            task.namespace.as_str(),
            task.tenant.as_str(),
            "https://example.com/hook",
        );
        let legacy_key = StateKey::new(
            task.namespace.as_str(),
            task.tenant.as_str(),
            KeyKind::A2aTaskPushConfig,
            config.storage_id(),
        );
        store
            .set(&legacy_key, &serde_json::to_string(&config).unwrap(), None)
            .await
            .unwrap();

        let http = acteon_http::GuardedClient::new(
            acteon_http::OutboundPolicy::default(),
            REQUEST_TIMEOUT,
        )
        .unwrap();
        let (_tx, rx) = broadcast::channel::<StreamEvent>(8);
        let worker = PushDeliveryWorker::new(Arc::clone(&store), http, rx);
        let configs = worker
            .shared
            .load_configs("agents", "demo", "legacy-task")
            .await
            .unwrap();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].id, "legacy-config");
        assert!(
            TaskEngine::new(store)
                .push_config_migration_complete(&scope, "legacy-task")
                .await
                .unwrap()
        );
    }

    #[tokio::test(flavor = "current_thread", start_paused = false)]
    async fn delivers_one_event_to_one_config() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let http = acteon_http::GuardedClient::new(
            acteon_http::OutboundPolicy {
                internal_hosts: vec!["127.0.0.1".into(), "localhost".into()],
            },
            REQUEST_TIMEOUT,
        )
        .unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let url = start_mock(200, Arc::clone(&hits)).await;
        let cfg = TaskPushNotificationConfig::new("cfg-1", "task-1", "agents", "demo", &url);
        save_config(&store, &cfg).await;
        let (tx, rx) = broadcast::channel::<StreamEvent>(8);
        let metrics = Arc::new(PushDeliveryMetrics::default());
        let worker =
            PushDeliveryWorker::with_metrics(Arc::clone(&store), http, rx, Arc::clone(&metrics))
                .with_ssrf_enforcement(false);
        let handle = tokio::spawn(worker.run());
        tx.send(persist_transition(&store, "task-1").await).unwrap();
        wait_for_deliveries(&metrics, 1).await;
        drop(tx);
        let _ = handle.await;
        assert_eq!(hits.load(Ordering::Relaxed), 1, "expected one POST");
    }

    #[tokio::test]
    async fn committed_acknowledgement_survives_lost_response_and_worker_replacement() {
        let memory: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let faults = Arc::new(FaultStore::new(memory));
        let store: Arc<dyn StateStore> = faults.clone();
        let hits = Arc::new(AtomicUsize::new(0));
        let url = start_mock(200, Arc::clone(&hits)).await;
        let cfg = TaskPushNotificationConfig::new(
            "cfg-lost-ack",
            "task-lost-ack",
            "agents",
            "demo",
            &url,
        );
        save_config(&store, &cfg).await;
        persist_transition(&store, "task-lost-ack").await;
        faults
            .fail_after_matches(
                KeyKind::A2aTask,
                WriteOperation::CompareAndSwap,
                FaultTiming::After,
                1,
            )
            .unwrap();

        let http = acteon_http::GuardedClient::new(
            acteon_http::OutboundPolicy {
                internal_hosts: vec!["127.0.0.1".into()],
            },
            REQUEST_TIMEOUT,
        )
        .unwrap();
        let (_tx, rx) = broadcast::channel::<StreamEvent>(8);
        let first = PushDeliveryWorker::new(Arc::clone(&store), http.clone(), rx)
            .with_ssrf_enforcement(false);
        first
            .shared
            .drain_callback(
                "agents".into(),
                "demo".into(),
                "task-lost-ack".into(),
                "cfg-lost-ack".into(),
            )
            .await;
        assert_eq!(faults.consumed(), 1, "the committed ACK response was lost");
        assert_eq!(hits.load(Ordering::Relaxed), 1);

        let (_tx, rx) = broadcast::channel::<StreamEvent>(8);
        let replacement =
            PushDeliveryWorker::new(Arc::clone(&store), http, rx).with_ssrf_enforcement(false);
        replacement
            .shared
            .drain_callback(
                "agents".into(),
                "demo".into(),
                "task-lost-ack".into(),
                "cfg-lost-ack".into(),
            )
            .await;
        assert_eq!(
            hits.load(Ordering::Relaxed),
            1,
            "replacement must observe the committed cursor and avoid redelivery"
        );
    }

    #[tokio::test]
    async fn credential_rotation_applies_only_to_subsequent_callback_events() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let captures = Arc::new(Mutex::new(Vec::new()));
        let url = start_header_capture_mock(Arc::clone(&captures)).await;
        let mut cfg = TaskPushNotificationConfig::new(
            "cfg-rotation",
            "task-rotation",
            "agents",
            "demo",
            &url,
        );
        cfg.token = Some("notification-token-v1".into());
        cfg.authentication = Some(PushAuthentication {
            schemes: vec!["Bearer".into()],
            credentials: Some("callback-credential-v1".into()),
        });
        save_config(&store, &cfg).await;
        persist_transition(&store, "task-rotation").await;

        let http = acteon_http::GuardedClient::new(
            acteon_http::OutboundPolicy {
                internal_hosts: vec!["127.0.0.1".into()],
            },
            REQUEST_TIMEOUT,
        )
        .unwrap();
        let (_tx, rx) = broadcast::channel::<StreamEvent>(8);
        let worker =
            PushDeliveryWorker::new(Arc::clone(&store), http, rx).with_ssrf_enforcement(false);
        worker
            .shared
            .drain_callback(
                "agents".into(),
                "demo".into(),
                "task-rotation".into(),
                "cfg-rotation".into(),
            )
            .await;

        cfg.token = Some("notification-token-v2".into());
        cfg.authentication = Some(PushAuthentication {
            schemes: vec!["Bearer".into()],
            credentials: Some("callback-credential-v2".into()),
        });
        cfg.updated_at += chrono::Duration::seconds(1);
        let scope = TaskScope::new("agents", "demo");
        let engine = TaskEngine::new(Arc::clone(&store));
        engine
            .upsert_push_config(&scope, "task-rotation", cfg)
            .await
            .unwrap();
        engine
            .transition_task(&scope, "task-rotation", TaskState::Completed, None)
            .await
            .unwrap();
        worker
            .shared
            .drain_callback(
                "agents".into(),
                "demo".into(),
                "task-rotation".into(),
                "cfg-rotation".into(),
            )
            .await;

        let captures = captures.lock().await;
        assert_eq!(captures.len(), 2);
        assert_eq!(captures[0].token.as_deref(), Some("notification-token-v1"));
        assert_eq!(
            captures[0].authorization.as_deref(),
            Some("Bearer callback-credential-v1")
        );
        assert_eq!(captures[0].sequence, "1");
        assert_eq!(captures[1].token.as_deref(), Some("notification-token-v2"));
        assert_eq!(
            captures[1].authorization.as_deref(),
            Some("Bearer callback-credential-v2")
        );
        assert_eq!(captures[1].sequence, "2");
        assert_ne!(captures[0].delivery_id, captures[1].delivery_id);
    }

    #[test]
    fn delivery_request_carries_stable_identity_headers() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let http = acteon_http::GuardedClient::new(
            acteon_http::OutboundPolicy::default(),
            REQUEST_TIMEOUT,
        )
        .unwrap();
        let (_tx, rx) = broadcast::channel::<StreamEvent>(8);
        let worker = PushDeliveryWorker::new(store, http, rx);
        let config = TaskPushNotificationConfig::new(
            "cfg-1",
            "task-1",
            "agents",
            "demo",
            "https://callbacks.example/hook",
        );
        let request = worker
            .shared
            .build_request(
                &config,
                &serde_json::json!({"statusUpdate": {}}),
                "0199d789-6900-7e03-8b2d-211e0bcbe296",
                7,
            )
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            request.headers()["x-acteon-delivery-id"],
            "0199d789-6900-7e03-8b2d-211e0bcbe296"
        );
        assert_eq!(request.headers()["x-acteon-delivery-sequence"], "7");
    }

    #[tokio::test(flavor = "current_thread", start_paused = false)]
    async fn ignores_non_a2a_events() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let http = acteon_http::GuardedClient::new(
            acteon_http::OutboundPolicy {
                internal_hosts: vec!["127.0.0.1".into(), "localhost".into()],
            },
            REQUEST_TIMEOUT,
        )
        .unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let url = start_mock(200, Arc::clone(&hits)).await;
        let cfg = TaskPushNotificationConfig::new("cfg-1", "task-1", "agents", "demo", &url);
        save_config(&store, &cfg).await;
        let (tx, rx) = broadcast::channel::<StreamEvent>(8);
        let worker = PushDeliveryWorker::new(store, http, rx).with_ssrf_enforcement(false);
        let handle = tokio::spawn(worker.run());
        let mut ev = mk_event("task-1");
        ev.action_type = Some("webhook".to_string());
        tx.send(ev).unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        drop(tx);
        let _ = handle.await;
        assert_eq!(
            hits.load(Ordering::Relaxed),
            0,
            "must not deliver non-A2A events"
        );
    }

    #[tokio::test(flavor = "current_thread", start_paused = false)]
    async fn skips_when_no_configs_registered() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let http = acteon_http::GuardedClient::new(
            acteon_http::OutboundPolicy {
                internal_hosts: vec!["127.0.0.1".into(), "localhost".into()],
            },
            REQUEST_TIMEOUT,
        )
        .unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let _url = start_mock(200, Arc::clone(&hits)).await;
        let (tx, rx) = broadcast::channel::<StreamEvent>(8);
        let worker = PushDeliveryWorker::new(store, http, rx).with_ssrf_enforcement(false);
        let handle = tokio::spawn(worker.run());
        tx.send(mk_event("task-1")).unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        drop(tx);
        let _ = handle.await;
        assert_eq!(hits.load(Ordering::Relaxed), 0);
    }

    #[tokio::test(flavor = "current_thread", start_paused = false)]
    async fn fan_outs_to_multiple_configs_for_same_task() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let http = acteon_http::GuardedClient::new(
            acteon_http::OutboundPolicy {
                internal_hosts: vec!["127.0.0.1".into(), "localhost".into()],
            },
            REQUEST_TIMEOUT,
        )
        .unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let url = start_mock(200, Arc::clone(&hits)).await;
        for n in 0..3 {
            let cfg = TaskPushNotificationConfig::new(
                format!("cfg-{n}"),
                "task-1",
                "agents",
                "demo",
                &url,
            );
            save_config(&store, &cfg).await;
        }
        let (tx, rx) = broadcast::channel::<StreamEvent>(8);
        let metrics = Arc::new(PushDeliveryMetrics::default());
        let worker =
            PushDeliveryWorker::with_metrics(Arc::clone(&store), http, rx, Arc::clone(&metrics))
                .with_ssrf_enforcement(false);
        let handle = tokio::spawn(worker.run());
        tx.send(persist_transition(&store, "task-1").await).unwrap();
        wait_for_deliveries(&metrics, 3).await;
        drop(tx);
        let _ = handle.await;
        assert_eq!(
            hits.load(Ordering::Relaxed),
            3,
            "expected one POST per registered config"
        );
    }

    #[tokio::test(flavor = "current_thread", start_paused = false)]
    async fn does_not_retry_on_non_transient_4xx() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let http = acteon_http::GuardedClient::new(
            acteon_http::OutboundPolicy {
                internal_hosts: vec!["127.0.0.1".into(), "localhost".into()],
            },
            REQUEST_TIMEOUT,
        )
        .unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let url = start_mock(404, Arc::clone(&hits)).await;
        let cfg = TaskPushNotificationConfig::new("cfg-1", "task-1", "agents", "demo", &url);
        save_config(&store, &cfg).await;
        let metrics = Arc::new(PushDeliveryMetrics::default());
        let (tx, rx) = broadcast::channel::<StreamEvent>(8);
        let worker =
            PushDeliveryWorker::with_metrics(Arc::clone(&store), http, rx, Arc::clone(&metrics))
                .with_ssrf_enforcement(false);
        let handle = tokio::spawn(worker.run());
        tx.send(persist_transition(&store, "task-1").await).unwrap();
        tokio::time::sleep(Duration::from_millis(800)).await;
        drop(tx);
        let _ = handle.await;
        assert_eq!(
            hits.load(Ordering::Relaxed),
            1,
            "a non-transient 4xx (404) must not be retried"
        );
        let snap = metrics.snapshot();
        assert_eq!(snap.deliveries_terminal, 1);
        assert_eq!(snap.deliveries_succeeded, 0);
        // A terminal failure must land a DLQ entry so an operator
        // can observe it through `/v1/a2a/.../push-dlq`.
        assert_eq!(snap.dlq_writes, 1);
        assert_eq!(snap.dlq_write_failures, 0);
        let dlq_entries = TaskEngine::new(Arc::clone(&store))
            .list_push_dead_letters(&TaskScope::new("agents", "demo"), "task-1")
            .await
            .unwrap();
        assert_eq!(dlq_entries.len(), 1, "expected one durable dead letter");
        let row = &dlq_entries[0];
        assert_eq!(row.failure_kind, DlqFailureKind::Terminal);
        assert_eq!(row.config_id, "cfg-1");
        assert_eq!(row.task_id, "task-1");
        // Attempts is the count of `post_once` calls — exactly 1 on
        // a terminal classification (no retry).
        assert_eq!(row.attempts, 1);
        // `last_error` carries the HTTP status snapshot.
        assert!(
            row.last_error.contains("404"),
            "last_error should carry the status: {}",
            row.last_error
        );
    }

    /// Adversarial review #3: a 429 must be retried, not terminally
    /// rejected — that was the bug.
    #[tokio::test(flavor = "current_thread", start_paused = false)]
    async fn retries_on_429_rate_limited() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let http = acteon_http::GuardedClient::new(
            acteon_http::OutboundPolicy {
                internal_hosts: vec!["127.0.0.1".into(), "localhost".into()],
            },
            REQUEST_TIMEOUT,
        )
        .unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let url = start_mock_first_then_ok(429, Arc::clone(&hits)).await;
        let cfg = TaskPushNotificationConfig::new("cfg-1", "task-1", "agents", "demo", &url);
        save_config(&store, &cfg).await;
        let metrics = Arc::new(PushDeliveryMetrics::default());
        let (tx, rx) = broadcast::channel::<StreamEvent>(8);
        let worker =
            PushDeliveryWorker::with_metrics(Arc::clone(&store), http, rx, Arc::clone(&metrics))
                .with_ssrf_enforcement(false);
        let handle = tokio::spawn(worker.run());
        tx.send(persist_transition(&store, "task-1").await).unwrap();
        // First attempt 429, BASE_BACKOFF=1s, then success. Give it
        // 1.5s of slack.
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        drop(tx);
        let _ = handle.await;
        assert_eq!(
            hits.load(Ordering::Relaxed),
            2,
            "429 must trigger a retry and reach the second mock response"
        );
        let snap = metrics.snapshot();
        assert_eq!(snap.deliveries_succeeded, 1);
        assert_eq!(snap.deliveries_terminal, 0);
    }

    /// Exhausted transient retries (MAX_DELIVERY_ATTEMPTS of 5xx)
    /// must land a DLQ entry tagged `Exhausted` with the full
    /// attempt count. Polls the state store rather than sleeping
    /// the full ~3s retry cycle to keep the test fast on success.
    #[tokio::test(flavor = "current_thread", start_paused = false)]
    async fn exhausted_retries_write_a_dlq_entry() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let http = acteon_http::GuardedClient::new(
            acteon_http::OutboundPolicy {
                internal_hosts: vec!["127.0.0.1".into(), "localhost".into()],
            },
            REQUEST_TIMEOUT,
        )
        .unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        // Every request returns 503 (transient) — the worker burns
        // through all retries and gives up.
        let url = start_mock(503, Arc::clone(&hits)).await;
        let cfg = TaskPushNotificationConfig::new("cfg-x", "task-x", "agents", "demo", &url);
        save_config(&store, &cfg).await;
        let metrics = Arc::new(PushDeliveryMetrics::default());
        let (tx, rx) = broadcast::channel::<StreamEvent>(8);
        let worker =
            PushDeliveryWorker::with_metrics(Arc::clone(&store), http, rx, Arc::clone(&metrics))
                .with_ssrf_enforcement(false);
        let handle = tokio::spawn(worker.run());
        tx.send(persist_transition(&store, "task-x").await).unwrap();
        // Poll for the DLQ row. The retry cycle is 3 attempts +
        // (1s + 2s) backoff, so allow up to ~5s.
        let dlq_row = loop {
            let entries = TaskEngine::new(Arc::clone(&store))
                .list_push_dead_letters(&TaskScope::new("agents", "demo"), "task-x")
                .await
                .unwrap();
            if !entries.is_empty() {
                break entries.into_iter().next().unwrap();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            if metrics.snapshot().deliveries_exhausted > 0 {
                // Worker is done but DLQ row not yet visible —
                // give it one more tick to flush.
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        };
        drop(tx);
        let _ = handle.await;
        assert_eq!(dlq_row.failure_kind, DlqFailureKind::Exhausted);
        assert_eq!(dlq_row.config_id, "cfg-x");
        assert_eq!(dlq_row.task_id, "task-x");
        // Exhausted entries record the full attempt count.
        assert_eq!(dlq_row.attempts, MAX_DELIVERY_ATTEMPTS as u32);
        let snap = metrics.snapshot();
        assert_eq!(snap.deliveries_exhausted, 1);
        assert_eq!(snap.dlq_writes, 1);
        assert_eq!(snap.dlq_write_failures, 0);
    }

    /// The production-default SSRF guard fails before network I/O and the
    /// failure is committed in the same Task envelope as the event.
    #[tokio::test]
    async fn ssrf_guard_dead_letters_durable_claim_without_network_io() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let http = acteon_http::GuardedClient::new(
            acteon_http::OutboundPolicy {
                internal_hosts: vec!["127.0.0.1".into(), "localhost".into()],
            },
            REQUEST_TIMEOUT,
        )
        .unwrap();
        let cfg = TaskPushNotificationConfig::new(
            "cfg-s",
            "task-s",
            "agents",
            "demo",
            "http://127.0.0.1:9/hook",
        );
        save_config(&store, &cfg).await;
        persist_transition(&store, "task-s").await;
        let metrics = Arc::new(PushDeliveryMetrics::default());
        let (_tx, rx) = broadcast::channel::<StreamEvent>(8);
        let worker =
            PushDeliveryWorker::with_metrics(Arc::clone(&store), http, rx, Arc::clone(&metrics));
        worker
            .shared
            .drain_callback(
                "agents".into(),
                "demo".into(),
                "task-s".into(),
                "cfg-s".into(),
            )
            .await;
        let snap = metrics.snapshot();
        assert_eq!(snap.deliveries_ssrf_blocked, 1);
        assert_eq!(snap.deliveries_terminal, 1);
        assert_eq!(snap.deliveries_exhausted, 0);
        let dlq = TaskEngine::new(Arc::clone(&store))
            .list_push_dead_letters(&TaskScope::new("agents", "demo"), "task-s")
            .await
            .unwrap();
        assert_eq!(dlq.len(), 1, "the refused delivery must land in the DLQ");
        let row = &dlq[0];
        assert_eq!(row.failure_kind, DlqFailureKind::Terminal);
        assert!(
            row.last_error.contains("SSRF"),
            "DLQ entry should name the SSRF guard: {}",
            row.last_error
        );
    }

    #[tokio::test]
    async fn recovery_page_repairs_a_missed_broadcast() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let http = acteon_http::GuardedClient::new(
            acteon_http::OutboundPolicy::default(),
            REQUEST_TIMEOUT,
        )
        .unwrap();
        let cfg = TaskPushNotificationConfig::new(
            "cfg-recovery",
            "task-recovery",
            "agents",
            "demo",
            "http://127.0.0.1:9/hook",
        );
        save_config(&store, &cfg).await;
        persist_transition(&store, "task-recovery").await;
        store
            .delete(&StateKey::new(
                "agents",
                "demo",
                KeyKind::A2aPushReady,
                "task-recovery",
            ))
            .await
            .unwrap();
        let (_tx, rx) = broadcast::channel::<StreamEvent>(8);
        let worker = PushDeliveryWorker::new(Arc::clone(&store), http, rx);

        worker.shared.recover_task_page(None).await.unwrap();

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let entries = TaskEngine::new(Arc::clone(&store))
                    .list_push_dead_letters(&TaskScope::new("agents", "demo"), "task-recovery")
                    .await
                    .unwrap();
                if entries.len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("recovery did not drain the durable event");
    }

    #[tokio::test]
    async fn ready_index_recovers_work_without_a_broadcast() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let http = acteon_http::GuardedClient::new(
            acteon_http::OutboundPolicy::default(),
            REQUEST_TIMEOUT,
        )
        .unwrap();
        let cfg = TaskPushNotificationConfig::new(
            "cfg-ready",
            "task-ready",
            "agents",
            "demo",
            "http://127.0.0.1:9/hook",
        );
        save_config(&store, &cfg).await;
        persist_transition(&store, "task-ready").await;
        let (_tx, rx) = broadcast::channel::<StreamEvent>(8);
        let worker = PushDeliveryWorker::new(Arc::clone(&store), http, rx);

        worker.shared.recover_ready_page(None).await.unwrap();

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let entries = TaskEngine::new(Arc::clone(&store))
                    .list_push_dead_letters(&TaskScope::new("agents", "demo"), "task-ready")
                    .await
                    .unwrap();
                if entries.len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("ready index did not drain the durable event");
    }

    /// Adversarial review #1: a slow webhook on one tenant must not
    /// block deliveries on another tenant. Two events fire roughly
    /// at the same time; the fast one must complete long before the
    /// slow one.
    #[tokio::test(flavor = "current_thread", start_paused = false)]
    async fn slow_webhook_does_not_starve_other_tenants() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let http = acteon_http::GuardedClient::new(
            acteon_http::OutboundPolicy {
                internal_hosts: vec!["127.0.0.1".into(), "localhost".into()],
            },
            REQUEST_TIMEOUT,
        )
        .unwrap();
        let fast_hits = Arc::new(AtomicUsize::new(0));
        let slow_hits = Arc::new(AtomicUsize::new(0));
        let fast = start_mock_with_delay(200, Duration::ZERO, Arc::clone(&fast_hits)).await;
        // 2 seconds is long enough to make a sequential worker
        // blatantly fail the timing assertion below.
        let slow = start_mock_with_delay(200, Duration::from_secs(2), Arc::clone(&slow_hits)).await;
        let slow_cfg =
            TaskPushNotificationConfig::new("slow-cfg", "task-slow", "agents", "demo", &slow);
        let fast_cfg =
            TaskPushNotificationConfig::new("fast-cfg", "task-fast", "agents", "demo", &fast);
        save_config(&store, &slow_cfg).await;
        save_config(&store, &fast_cfg).await;
        let (tx, rx) = broadcast::channel::<StreamEvent>(16);
        let worker =
            PushDeliveryWorker::new(Arc::clone(&store), http, rx).with_ssrf_enforcement(false);
        let handle = tokio::spawn(worker.run());
        // Slow event first, fast event right behind. A sequential
        // worker would only reach the fast event after the slow
        // one's 2s delay finished.
        tx.send(persist_transition(&store, "task-slow").await)
            .unwrap();
        tx.send(persist_transition(&store, "task-fast").await)
            .unwrap();
        let start = Instant::now();
        // Poll for the fast hit. It must land well before the slow
        // delay completes (2s).
        while fast_hits.load(Ordering::Relaxed) == 0 && start.elapsed() < Duration::from_secs(1) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            fast_hits.load(Ordering::Relaxed),
            1,
            "fast tenant's POST must land within 1s even while the slow tenant's POST is in flight",
        );
        // Now let the slow tenant complete and tear the worker down.
        tokio::time::sleep(Duration::from_millis(2_300)).await;
        drop(tx);
        let _ = handle.await;
        assert_eq!(slow_hits.load(Ordering::Relaxed), 1);
    }

    /// Adversarial review #4: a burst of events for the same task
    /// must not produce a `scan_keys` per event. The cache folds
    /// them into (ideally) a single scan within the TTL window.
    #[tokio::test(flavor = "current_thread", start_paused = false)]
    async fn config_cache_collapses_burst_to_single_scan() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let http = acteon_http::GuardedClient::new(
            acteon_http::OutboundPolicy {
                internal_hosts: vec!["127.0.0.1".into(), "localhost".into()],
            },
            REQUEST_TIMEOUT,
        )
        .unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let url = start_mock(200, Arc::clone(&hits)).await;
        let cfg = TaskPushNotificationConfig::new("cfg-1", "task-1", "agents", "demo", &url);
        save_config(&store, &cfg).await;
        let metrics = Arc::new(PushDeliveryMetrics::default());
        let (tx, rx) = broadcast::channel::<StreamEvent>(64);
        let worker =
            PushDeliveryWorker::with_metrics(Arc::clone(&store), http, rx, Arc::clone(&metrics))
                .with_ssrf_enforcement(false);
        let handle = tokio::spawn(worker.run());
        let wake = persist_transition(&store, "task-1").await;
        // Fire 20 duplicate wake-ups for one durable event in a tight burst.
        // Fenced claims must collapse them to one delivery.
        for _ in 0..20 {
            tx.send(wake.clone()).unwrap();
        }
        // Wait for the deliveries to settle.
        tokio::time::sleep(Duration::from_millis(400)).await;
        drop(tx);
        let _ = handle.await;
        assert_eq!(hits.load(Ordering::Relaxed), 1);
        let snap = metrics.snapshot();
        assert_eq!(snap.events_dispatched, 20);
        // The cache may produce a couple of redundant scans because
        // concurrent dispatch tasks race the cache lock — but it
        // must be *vastly* less than the event count.
        assert!(
            snap.state_store_scans <= 4,
            "expected the burst to fold into a few scans (≤4); got {}",
            snap.state_store_scans
        );
    }

    /// Counter sanity: a successful delivery bumps the right
    /// counters and leaves the others untouched.
    #[tokio::test(flavor = "current_thread", start_paused = false)]
    async fn metrics_count_success_and_attempts() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let http = acteon_http::GuardedClient::new(
            acteon_http::OutboundPolicy {
                internal_hosts: vec!["127.0.0.1".into(), "localhost".into()],
            },
            REQUEST_TIMEOUT,
        )
        .unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let url = start_mock(200, Arc::clone(&hits)).await;
        let cfg = TaskPushNotificationConfig::new("cfg-1", "task-1", "agents", "demo", &url);
        save_config(&store, &cfg).await;
        let metrics = Arc::new(PushDeliveryMetrics::default());
        let (tx, rx) = broadcast::channel::<StreamEvent>(8);
        let worker =
            PushDeliveryWorker::with_metrics(Arc::clone(&store), http, rx, Arc::clone(&metrics))
                .with_ssrf_enforcement(false);
        let handle = tokio::spawn(worker.run());
        tx.send(persist_transition(&store, "task-1").await).unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        drop(tx);
        let _ = handle.await;
        let snap = metrics.snapshot();
        assert_eq!(snap.events_dispatched, 1);
        assert_eq!(snap.deliveries_attempted, 1);
        assert_eq!(snap.deliveries_succeeded, 1);
        assert_eq!(snap.deliveries_terminal, 0);
        assert_eq!(snap.deliveries_exhausted, 0);
        // Cache should record at least one scan (the cold path).
        assert!(snap.state_store_scans >= 1);
    }
}
