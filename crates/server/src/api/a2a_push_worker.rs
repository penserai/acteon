//! A2A push-notification delivery worker (Phase 4.2).
//!
//! Consumes the gateway-wide `StreamEvent` broadcast and, for every
//! event tagged `action_type = "a2a.task"`, fans the envelope out to
//! the `TaskPushNotificationConfig` rows registered for that
//! `action_id` (= `task_id`).
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
//! - Per-config retry loop with bounded attempts and capped
//!   exponential backoff. The transient-vs-terminal classification
//!   treats `408`, `425`, and `429` as transient — they are exactly
//!   the codes a well-behaved server uses to ask the client to back
//!   off, so retrying is correct.
//! - No DLQ in this slice — terminal and exhausted failures are
//!   counted in `PushDeliveryMetrics` and logged with `error!`. A
//!   persistent DLQ can layer on later without touching the
//!   broadcast subscriber.
//!
//! The HTTP client is provided by the caller — typically the shared
//! `reqwest::Client` `main.rs` builds once and threads through every
//! provider, so connection pooling and any mTLS or proxy settings are
//! shared with the rest of the server.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use acteon_core::{StreamEvent, TaskPushNotificationConfig};
use acteon_state::{KeyKind, StateStore};
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
    /// succeeding. Candidates for the future DLQ.
    pub deliveries_exhausted: AtomicU64,
    /// `scan_keys` calls actually issued to the state store after
    /// the config-cache check. The cache hit rate is
    /// `1 - state_store_scans / events_dispatched` (approximately).
    pub state_store_scans: AtomicU64,
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
}

/// Internal state held behind an `Arc` and shared across every
/// spawned dispatch + delivery task.
struct Shared {
    state: Arc<dyn StateStore>,
    http: reqwest::Client,
    delivery_permits: Arc<Semaphore>,
    cache: Mutex<HashMap<CacheKey, CacheEntry>>,
    metrics: Arc<PushDeliveryMetrics>,
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
        http: reqwest::Client,
        rx: broadcast::Receiver<StreamEvent>,
    ) -> Self {
        Self::with_metrics(state, http, rx, Arc::new(PushDeliveryMetrics::default()))
    }

    /// As [`Self::new`], but with a caller-owned metrics handle so
    /// the metrics can be read out by an exporter / test harness.
    #[must_use]
    pub fn with_metrics(
        state: Arc<dyn StateStore>,
        http: reqwest::Client,
        rx: broadcast::Receiver<StreamEvent>,
        metrics: Arc<PushDeliveryMetrics>,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                state,
                http,
                delivery_permits: Arc::new(Semaphore::new(MAX_INFLIGHT_DELIVERIES)),
                cache: Mutex::new(HashMap::new()),
                metrics,
            }),
            rx,
        }
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
    /// The loop body is intentionally tiny — receive, filter,
    /// detach. Every per-event step (config lookup, fan-out,
    /// delivery, retry) runs in a spawned task so a slow webhook
    /// cannot block the loop from draining the broadcast.
    pub async fn run(mut self) {
        loop {
            match self.rx.recv().await {
                Ok(event) => {
                    if event.action_type.as_deref() != Some(A2A_TASK_ACTION_TYPE) {
                        continue;
                    }
                    self.shared
                        .metrics
                        .events_dispatched
                        .fetch_add(1, Ordering::Relaxed);
                    let shared = Arc::clone(&self.shared);
                    // Detached — the dispatch task lives independent
                    // of this loop and the loop returns to recv
                    // immediately.
                    tokio::spawn(async move {
                        Shared::dispatch_event(shared, event).await;
                    });
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    self.shared
                        .metrics
                        .events_lagged
                        .fetch_add(n, Ordering::Relaxed);
                    warn!(skipped = n, "A2A push worker lagged behind the broadcast");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    debug!("A2A push worker: broadcast closed, shutting down");
                    return;
                }
            }
        }
    }
}

impl Shared {
    /// Look up every config registered for `event.action_id` (=
    /// `task_id`) under the event's namespace + tenant and fan the
    /// delivery out to each — **concurrently**.
    async fn dispatch_event(self: Arc<Self>, event: StreamEvent) {
        let Some(task_id) = event.action_id.as_deref() else {
            return;
        };
        if task_id.is_empty() {
            return;
        }
        let configs = match self
            .load_configs_cached(&event.namespace, &event.tenant, task_id)
            .await
        {
            Ok(c) => c,
            Err(e) => {
                warn!(
                    error = %e,
                    namespace = %event.namespace,
                    tenant = %event.tenant,
                    task_id,
                    "A2A push worker: config lookup failed; skipping event",
                );
                return;
            }
        };
        if configs.is_empty() {
            return;
        }
        let event = Arc::new(event);
        // Fan out: each delivery runs in its own task, bounded by
        // the global semaphore. A slow URL holds a permit but does
        // not stall the other deliveries for the same event (or
        // future events).
        for config in configs.iter() {
            let shared = Arc::clone(&self);
            let event = Arc::clone(&event);
            let config = config.clone();
            tokio::spawn(async move {
                // Acquire a permit *inside* the spawned task so the
                // detach happens fast and the semaphore back-pressures
                // queued deliveries rather than the dispatch loop.
                let Ok(_permit) = shared.delivery_permits.acquire().await else {
                    // The semaphore was closed — shutdown.
                    return;
                };
                shared.deliver_with_retry(config, event).await;
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
        let prefix = format!("{task_id}:");
        let entries = self
            .state
            .scan_keys(namespace, tenant, KeyKind::A2aTaskPushConfig, Some(&prefix))
            .await
            .map_err(|e| e.to_string())?;
        let mut out = Vec::with_capacity(entries.len());
        for (_, raw) in entries {
            if let Ok(c) = serde_json::from_str::<TaskPushNotificationConfig>(&raw) {
                out.push(c);
            }
        }
        Ok(out)
    }

    /// Deliver one event to one config with bounded retries. Holds
    /// a delivery-permit for the duration of all attempts so a
    /// retry-storm under one URL doesn't double-count against the
    /// concurrency budget.
    async fn deliver_with_retry(
        &self,
        config: TaskPushNotificationConfig,
        event: Arc<StreamEvent>,
    ) {
        for attempt in 0..MAX_DELIVERY_ATTEMPTS {
            self.metrics
                .deliveries_attempted
                .fetch_add(1, Ordering::Relaxed);
            match self.post_once(&config, &event).await {
                Ok(()) => {
                    self.metrics
                        .deliveries_succeeded
                        .fetch_add(1, Ordering::Relaxed);
                    debug!(
                        url = %config.url,
                        config_id = %config.id,
                        task_id = %config.task_id,
                        attempt,
                        "A2A push delivered",
                    );
                    return;
                }
                Err(DeliveryError::Terminal(reason)) => {
                    self.metrics
                        .deliveries_terminal
                        .fetch_add(1, Ordering::Relaxed);
                    error!(
                        url = %config.url,
                        config_id = %config.id,
                        task_id = %config.task_id,
                        attempt,
                        reason = %reason,
                        "A2A push permanently rejected; not retrying",
                    );
                    return;
                }
                Err(DeliveryError::Transient(reason)) => {
                    let last = attempt + 1 == MAX_DELIVERY_ATTEMPTS;
                    if last {
                        self.metrics
                            .deliveries_exhausted
                            .fetch_add(1, Ordering::Relaxed);
                        error!(
                            url = %config.url,
                            config_id = %config.id,
                            task_id = %config.task_id,
                            attempt,
                            reason = %reason,
                            "A2A push exhausted retries",
                        );
                        return;
                    }
                    let delay = backoff(attempt);
                    warn!(
                        url = %config.url,
                        config_id = %config.id,
                        task_id = %config.task_id,
                        attempt,
                        retry_in_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                        reason = %reason,
                        "A2A push transient failure, retrying",
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    /// Send one POST. Maps the result into the worker's transient /
    /// terminal classification. Network and timeout failures are
    /// transient; HTTP 429 / 408 / 425 are transient (per spec and
    /// convention — these are the codes a server uses to ask for a
    /// retry); other 4xx are terminal; everything 5xx is transient.
    async fn post_once(
        &self,
        config: &TaskPushNotificationConfig,
        event: &StreamEvent,
    ) -> Result<(), DeliveryError> {
        let mut req = self
            .http
            .post(&config.url)
            .timeout(REQUEST_TIMEOUT)
            .json(event);
        if let Some(token) = &config.token {
            req = req.bearer_auth(token);
        }
        let resp = req.send().await.map_err(|e| {
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
    use acteon_core::{StreamEventType, TaskState};
    use acteon_state::{StateKey, StateStore};
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
        let key = StateKey::new(
            c.namespace.clone(),
            c.tenant.clone(),
            KeyKind::A2aTaskPushConfig,
            c.storage_id(),
        );
        let payload = serde_json::to_string(c).unwrap();
        store.set(&key, &payload, None).await.unwrap();
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

    #[tokio::test(flavor = "current_thread", start_paused = false)]
    async fn delivers_one_event_to_one_config() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let http = reqwest::Client::new();
        let hits = Arc::new(AtomicUsize::new(0));
        let url = start_mock(200, Arc::clone(&hits)).await;
        let cfg = TaskPushNotificationConfig::new("cfg-1", "task-1", "agents", "demo", &url);
        save_config(&store, &cfg).await;
        let (tx, rx) = broadcast::channel::<StreamEvent>(8);
        let worker = PushDeliveryWorker::new(store, http, rx);
        let handle = tokio::spawn(worker.run());
        tx.send(mk_event("task-1")).unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        drop(tx);
        let _ = handle.await;
        assert_eq!(hits.load(Ordering::Relaxed), 1, "expected one POST");
    }

    #[tokio::test(flavor = "current_thread", start_paused = false)]
    async fn ignores_non_a2a_events() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let http = reqwest::Client::new();
        let hits = Arc::new(AtomicUsize::new(0));
        let url = start_mock(200, Arc::clone(&hits)).await;
        let cfg = TaskPushNotificationConfig::new("cfg-1", "task-1", "agents", "demo", &url);
        save_config(&store, &cfg).await;
        let (tx, rx) = broadcast::channel::<StreamEvent>(8);
        let worker = PushDeliveryWorker::new(store, http, rx);
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
        let http = reqwest::Client::new();
        let hits = Arc::new(AtomicUsize::new(0));
        let _url = start_mock(200, Arc::clone(&hits)).await;
        let (tx, rx) = broadcast::channel::<StreamEvent>(8);
        let worker = PushDeliveryWorker::new(store, http, rx);
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
        let http = reqwest::Client::new();
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
        let worker = PushDeliveryWorker::new(store, http, rx);
        let handle = tokio::spawn(worker.run());
        tx.send(mk_event("task-1")).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
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
        let http = reqwest::Client::new();
        let hits = Arc::new(AtomicUsize::new(0));
        let url = start_mock(404, Arc::clone(&hits)).await;
        let cfg = TaskPushNotificationConfig::new("cfg-1", "task-1", "agents", "demo", &url);
        save_config(&store, &cfg).await;
        let metrics = Arc::new(PushDeliveryMetrics::default());
        let (tx, rx) = broadcast::channel::<StreamEvent>(8);
        let worker = PushDeliveryWorker::with_metrics(store, http, rx, Arc::clone(&metrics));
        let handle = tokio::spawn(worker.run());
        tx.send(mk_event("task-1")).unwrap();
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
    }

    /// Adversarial review #3: a 429 must be retried, not terminally
    /// rejected — that was the bug.
    #[tokio::test(flavor = "current_thread", start_paused = false)]
    async fn retries_on_429_rate_limited() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let http = reqwest::Client::new();
        let hits = Arc::new(AtomicUsize::new(0));
        let url = start_mock_first_then_ok(429, Arc::clone(&hits)).await;
        let cfg = TaskPushNotificationConfig::new("cfg-1", "task-1", "agents", "demo", &url);
        save_config(&store, &cfg).await;
        let metrics = Arc::new(PushDeliveryMetrics::default());
        let (tx, rx) = broadcast::channel::<StreamEvent>(8);
        let worker = PushDeliveryWorker::with_metrics(store, http, rx, Arc::clone(&metrics));
        let handle = tokio::spawn(worker.run());
        tx.send(mk_event("task-1")).unwrap();
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

    /// Adversarial review #1: a slow webhook on one tenant must not
    /// block deliveries on another tenant. Two events fire roughly
    /// at the same time; the fast one must complete long before the
    /// slow one.
    #[tokio::test(flavor = "current_thread", start_paused = false)]
    async fn slow_webhook_does_not_starve_other_tenants() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let http = reqwest::Client::new();
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
        let worker = PushDeliveryWorker::new(store, http, rx);
        let handle = tokio::spawn(worker.run());
        // Slow event first, fast event right behind. A sequential
        // worker would only reach the fast event after the slow
        // one's 2s delay finished.
        tx.send(mk_event("task-slow")).unwrap();
        tx.send(mk_event("task-fast")).unwrap();
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
        let http = reqwest::Client::new();
        let hits = Arc::new(AtomicUsize::new(0));
        let url = start_mock(200, Arc::clone(&hits)).await;
        let cfg = TaskPushNotificationConfig::new("cfg-1", "task-1", "agents", "demo", &url);
        save_config(&store, &cfg).await;
        let metrics = Arc::new(PushDeliveryMetrics::default());
        let (tx, rx) = broadcast::channel::<StreamEvent>(64);
        let worker = PushDeliveryWorker::with_metrics(store, http, rx, Arc::clone(&metrics));
        let handle = tokio::spawn(worker.run());
        // Fire 20 events for the same task in a tight burst (well
        // inside CONFIG_CACHE_TTL = 500ms).
        for _ in 0..20 {
            tx.send(mk_event("task-1")).unwrap();
        }
        // Wait for the deliveries to settle.
        tokio::time::sleep(Duration::from_millis(400)).await;
        drop(tx);
        let _ = handle.await;
        // 20 events → 20 POSTs delivered.
        assert_eq!(hits.load(Ordering::Relaxed), 20);
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
        let http = reqwest::Client::new();
        let hits = Arc::new(AtomicUsize::new(0));
        let url = start_mock(200, Arc::clone(&hits)).await;
        let cfg = TaskPushNotificationConfig::new("cfg-1", "task-1", "agents", "demo", &url);
        save_config(&store, &cfg).await;
        let metrics = Arc::new(PushDeliveryMetrics::default());
        let (tx, rx) = broadcast::channel::<StreamEvent>(8);
        let worker = PushDeliveryWorker::with_metrics(store, http, rx, Arc::clone(&metrics));
        let handle = tokio::spawn(worker.run());
        tx.send(mk_event("task-1")).unwrap();
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
