//! A2A push-notification delivery worker (Phase 4.2).
//!
//! Consumes the gateway-wide `StreamEvent` broadcast and, for every
//! event tagged `action_type = "a2a.task"`, fans the envelope out to
//! the `TaskPushNotificationConfig` rows registered for that
//! `action_id` (= `task_id`).
//!
//! Architecture:
//!
//! - One background `tokio::spawn`'d task per server, owning a
//!   dedicated `broadcast::Receiver<StreamEvent>`.
//! - Per-config retry loop with bounded attempts and capped
//!   exponential backoff. A 4xx response is **terminal** (the URL is
//!   permanently rejecting the payload); a 5xx, a timeout, or a
//!   connection error is **transient** and retried.
//! - No DLQ in this slice — terminal and exhausted failures are
//!   logged with `error!`. A persistent DLQ can layer on later
//!   without touching the broadcast subscriber.
//! - The worker is independent of the `make_event_stream` helper used
//!   by the SSE consumer endpoint. Both subscribe to the same
//!   broadcast; they do not share filter state, so a slow HTTP
//!   destination cannot lag the SSE clients (or vice versa).
//!
//! The HTTP client is provided by the caller — typically the shared
//! `reqwest::Client` `main.rs` builds once and threads through every
//! provider, so connection pooling and any mTLS or proxy settings are
//! shared with the rest of the server.

use std::sync::Arc;
use std::time::Duration;

use acteon_core::{StreamEvent, TaskPushNotificationConfig};
use acteon_state::{KeyKind, StateStore};
use tokio::sync::broadcast;
use tracing::{debug, error, warn};

/// `action_type` value the gateway stamps on every A2A task event.
/// Used to filter the broadcast subscriber down to push-relevant
/// events.
const A2A_TASK_ACTION_TYPE: &str = "a2a.task";

/// Number of retry attempts per delivery, *including* the initial
/// send. Three attempts with the backoff below gives an upper bound
/// of `1s + 2s = 3s` of waiting per terminal failure.
const MAX_DELIVERY_ATTEMPTS: usize = 3;

/// Base delay for the exponential backoff between transient-failure
/// retries. Attempt `n` (zero-indexed) waits `BASE * 2^n` before
/// retrying, capped by [`MAX_BACKOFF`].
const BASE_BACKOFF: Duration = Duration::from_secs(1);

/// Hard cap on the backoff between retries. Stops `BASE * 2^n` from
/// growing unboundedly if `MAX_DELIVERY_ATTEMPTS` is later raised.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Per-request timeout for one delivery attempt. Independent of (and
/// shorter than) any global timeout on the shared `reqwest::Client`
/// so a hung webhook cannot stall the worker's progression through
/// the broadcast.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The push-delivery worker. Construct one per server, attach its
/// [`broadcast::Receiver`] and the shared HTTP client, then `spawn`
/// it.
pub struct PushDeliveryWorker {
    state: Arc<dyn StateStore>,
    http: reqwest::Client,
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
        Self { state, http, rx }
    }

    /// Drive the worker until the broadcast channel closes (server
    /// shutdown). Intended to be `tokio::spawn`'d at startup.
    pub async fn run(mut self) {
        loop {
            match self.rx.recv().await {
                Ok(event) => {
                    if event.action_type.as_deref() != Some(A2A_TASK_ACTION_TYPE) {
                        continue;
                    }
                    self.dispatch_event(event).await;
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(skipped = n, "A2A push worker lagged behind the broadcast");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    debug!("A2A push worker: broadcast closed, shutting down");
                    return;
                }
            }
        }
    }

    /// Look up every config registered for `event.action_id` (=
    /// `task_id`) under the event's namespace + tenant and fan the
    /// delivery out to each.
    async fn dispatch_event(&self, event: StreamEvent) {
        let Some(task_id) = event.action_id.as_deref() else {
            return;
        };
        if task_id.is_empty() {
            return;
        }
        let configs = match self
            .load_configs(&event.namespace, &event.tenant, task_id)
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
        for config in configs {
            self.deliver_with_retry(config, Arc::clone(&event)).await;
        }
    }

    /// Load every config bound to one task. Mirrors
    /// `a2a_push::list_configs` but does not require an
    /// `AppState` — the worker holds the state store directly.
    async fn load_configs(
        &self,
        namespace: &str,
        tenant: &str,
        task_id: &str,
    ) -> Result<Vec<TaskPushNotificationConfig>, String> {
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

    /// Deliver one event to one config with bounded retries.
    async fn deliver_with_retry(
        &self,
        config: TaskPushNotificationConfig,
        event: Arc<StreamEvent>,
    ) {
        for attempt in 0..MAX_DELIVERY_ATTEMPTS {
            match self.post_once(&config, &event).await {
                Ok(()) => {
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
    /// terminal classification. `reqwest::Client` failures (DNS,
    /// connect, timeout, body) are transient; a 4xx response is
    /// terminal; a 5xx response is transient.
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
            Err(DeliveryError::Terminal(format!("HTTP {status}")))
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Minimal mock HTTP server that counts inbound connections and
    /// answers each with the supplied status. Returns the bound URL.
    /// Lives for `expected` connections, then shuts down.
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

    /// Start a mock listener. Each accepted connection bumps `hits`
    /// and answers with `status`. Returns the bound URL.
    async fn start_mock(status: u16, hits: Arc<AtomicUsize>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                hits.fetch_add(1, Ordering::Relaxed);
                // Drain the request bytes (best effort).
                let mut buf = vec![0u8; 8192];
                let _ = stream.read(&mut buf).await;
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
        // Give the worker a moment to deliver.
        tokio::time::sleep(Duration::from_millis(150)).await;
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
        // Tag the event with a non-A2A action_type — must be skipped.
        let mut ev = mk_event("task-1");
        ev.action_type = Some("webhook".to_string());
        tx.send(ev).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
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
        // Bring up a mock so a stray POST would be visible; never
        // register a config.
        let _url = start_mock(200, Arc::clone(&hits)).await;
        let (tx, rx) = broadcast::channel::<StreamEvent>(8);
        let worker = PushDeliveryWorker::new(store, http, rx);
        let handle = tokio::spawn(worker.run());
        tx.send(mk_event("task-1")).unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;
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
        tokio::time::sleep(Duration::from_millis(250)).await;
        drop(tx);
        let _ = handle.await;
        assert_eq!(
            hits.load(Ordering::Relaxed),
            3,
            "expected one POST per registered config"
        );
    }

    #[tokio::test(flavor = "current_thread", start_paused = false)]
    async fn does_not_retry_on_4xx_terminal() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let http = reqwest::Client::new();
        let hits = Arc::new(AtomicUsize::new(0));
        let url = start_mock(404, Arc::clone(&hits)).await;
        let cfg = TaskPushNotificationConfig::new("cfg-1", "task-1", "agents", "demo", &url);
        save_config(&store, &cfg).await;
        let (tx, rx) = broadcast::channel::<StreamEvent>(8);
        let worker = PushDeliveryWorker::new(store, http, rx);
        let handle = tokio::spawn(worker.run());
        tx.send(mk_event("task-1")).unwrap();
        // Wait long enough that retries WOULD have fired (BASE_BACKOFF
        // is 1s, so a single-attempt failure plus retries would take
        // multiple seconds). 800ms is enough to confirm "no retry
        // happened" without paying the full backoff.
        tokio::time::sleep(Duration::from_millis(800)).await;
        drop(tx);
        let _ = handle.await;
        assert_eq!(hits.load(Ordering::Relaxed), 1, "a 4xx must not be retried");
    }
}
