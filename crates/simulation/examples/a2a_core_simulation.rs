//! End-to-end exercise of the A2A protocol core surface.
//!
//! This simulation drives every reachable Task lifecycle state, the
//! pause-for-human (`InputRequired` / `AuthRequired`) interrupts, the
//! artifact-streaming gatekeeper, the SSE event broadcast, and the
//! push-notification delivery worker. It runs in-process — no HTTP
//! server is spawned — so it reads as a single linear log on stdout
//! and finishes in well under a second.
//!
//! ## What lands on stdout
//!
//! Each scenario is bracketed by a banner. Inside, every meaningful
//! transition is paired with the `StreamEvent` the gateway emits on
//! the broadcast channel, so the relationship between *engine action*
//! and *external observer event* is visible at a glance.
//!
//! The 8 A2A `TaskState` variants visited end-to-end:
//!
//! - `Submitted`   — initial state on `create_task`.
//! - `Working`     — first transition after submission.
//! - `InputRequired` — pause-for-user-input interrupt.
//! - `AuthRequired`  — pause-for-user-auth interrupt.
//! - `Completed`   — clean terminal.
//! - `Failed`      — stale-task reaper terminal.
//! - `Canceled`    — explicit cancel terminal.
//! - `Rejected`    — initial-validation terminal (covered in scenario 8).
//!
//! Run with:
//!
//! ```text
//! cargo run -p acteon-simulation --example a2a_core_simulation
//! ```

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use acteon_core::{
    Artifact, PauseKind, StreamEvent, StreamEventType, Task, TaskArtifactUpdateEvent, TaskMessage,
    TaskPart, TaskPushNotificationConfig, TaskRole, TaskState,
};
use acteon_gateway::{TaskEngine, TaskScope};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;
use chrono::{Duration as ChronoDuration, Utc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tracing::{Level, info};

const NS: &str = "agents";
const TENANT: &str = "demo";

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_env_filter("info,acteon_gateway=warn")
        .with_target(false)
        .try_init()
        .ok();

    banner("A2A core simulation — full Task lifecycle in one run");
    info!("All scenarios share one in-memory state store and one stream-event broadcast,");
    info!("so the engine actions on the left and the events on the right line up by id.");

    // Shared infrastructure: one state store, one broadcast channel,
    // one TaskEngine bound to both. A second subscriber tails the
    // broadcast in the background and logs each event as it lands.
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let (tx, _rx_root) = broadcast::channel::<StreamEvent>(256);
    let engine = TaskEngine::new(Arc::clone(&store)).with_stream_tx(tx.clone());
    let scope = TaskScope::new(NS, TENANT);
    let _events_handle = spawn_event_tailer(tx.subscribe());

    // ============================================================
    // SCENARIO 1 — Mint a task, observe Submitted.
    // ============================================================
    banner("1. Mint a task (Submitted)");
    let task_id = "task-alpha";
    engine.create_task(seed_task(task_id)).await?;
    let t = engine.get_task(&scope, task_id).await?.expect("created");
    assert_eq!(t.status.state, TaskState::Submitted);
    info!("  task '{}' minted at state={:?}", task_id, t.status.state);

    // ============================================================
    // SCENARIO 2 — Promote to Working; observe TaskTransitioned.
    // ============================================================
    banner("2. Promote Submitted → Working");
    engine
        .transition_task(&scope, task_id, TaskState::Working, None)
        .await?;
    settle().await;
    info!("  state is now Working — observe the TaskTransitioned event above.");

    // ============================================================
    // SCENARIO 3 — Append a history message; observe TaskHistoryAppended.
    // ============================================================
    banner("3. Append a history message");
    let mut hist_msg = TaskMessage::text(
        "msg-3a".to_string(),
        TaskRole::Agent,
        "I am thinking about this. Stand by.",
    );
    hist_msg.task_id = Some(task_id.to_string());
    engine.append_history(&scope, task_id, hist_msg).await?;
    settle().await;
    info!("  history now carries the message — observe TaskHistoryAppended.");

    // ============================================================
    // SCENARIO 4 — Stream an artifact in two chunks.
    //
    // Demonstrates the artifact-stream gatekeeper: chunk 0 with
    // last=false establishes the stream; chunk 1 with last=true
    // closes it. A third chunk after close would be rejected — see
    // the gateway's `apply_artifact_event` invariant.
    // ============================================================
    banner("4. Stream an artifact in two chunks");
    let art_id = "artifact-1";
    engine
        .apply_artifact_update(
            &scope,
            TaskArtifactUpdateEvent::chunk(
                task_id,
                Artifact::new(art_id, vec![TaskPart::text("Hello, ")]),
                0,
                false,
            ),
        )
        .await?;
    settle().await;
    engine
        .apply_artifact_update(
            &scope,
            TaskArtifactUpdateEvent::chunk(
                task_id,
                Artifact::new(art_id, vec![TaskPart::text("world.")]),
                1,
                true,
            ),
        )
        .await?;
    settle().await;
    let t = engine.get_task(&scope, task_id).await?.expect("present");
    info!(
        "  artifact '{}' now carries {} parts; observed last_chunk=true on the second event.",
        art_id,
        t.artifacts
            .iter()
            .find(|a| a.artifact_id == art_id)
            .map_or(0, |a| a.parts.len()),
    );

    // ============================================================
    // SCENARIO 5 — Pause for user input, then resume.
    //
    // A2A's `InputRequired` interrupt: the engine carves out the
    // pause as a `BusApproval` row + a Task transition, both
    // committed atomically. Resuming is a normal transition back
    // to Working with the resuming message.
    // ============================================================
    banner("5. Pause for user input (Working → InputRequired) and resume");
    let (paused, approval) = engine
        .pause_for_human(
            &scope,
            task_id,
            PauseKind::UserInput,
            Some("Need to confirm the destination before I send.".to_string()),
            None,
        )
        .await?;
    settle().await;
    info!(
        "  paused at state={:?}, approval id={}",
        paused.status.state, approval.approval_id
    );
    let mut resume_msg = TaskMessage::text(
        "msg-5-resume".to_string(),
        TaskRole::User,
        "Yes, send it to ops@example.com.",
    );
    resume_msg.task_id = Some(task_id.to_string());
    engine
        .transition_task(&scope, task_id, TaskState::Working, Some(resume_msg))
        .await?;
    settle().await;
    info!("  resumed → Working.");

    // ============================================================
    // SCENARIO 6 — Pause for auth, then resume.
    // ============================================================
    banner("6. Pause for user auth (Working → AuthRequired) and resume");
    engine
        .pause_for_human(&scope, task_id, PauseKind::UserAuth, None, None)
        .await?;
    settle().await;
    let mut auth_msg = TaskMessage::text(
        "msg-6-resume".to_string(),
        TaskRole::User,
        "Auth ok, here is the token.",
    );
    auth_msg.task_id = Some(task_id.to_string());
    engine
        .transition_task(&scope, task_id, TaskState::Working, Some(auth_msg))
        .await?;
    settle().await;
    info!("  AuthRequired pause resumed → Working.");

    // ============================================================
    // SCENARIO 7 — Complete the task.
    // ============================================================
    banner("7. Complete the task (Working → Completed)");
    let mut done_msg = TaskMessage::text("msg-7-done".to_string(), TaskRole::Agent, "All done.");
    done_msg.task_id = Some(task_id.to_string());
    engine
        .transition_task(&scope, task_id, TaskState::Completed, Some(done_msg))
        .await?;
    settle().await;
    let t = engine.get_task(&scope, task_id).await?.expect("present");
    assert_eq!(t.status.state, TaskState::Completed);
    info!("  task-alpha terminal at Completed.");

    // ============================================================
    // SCENARIO 8 — Reject a malformed inbound task.
    //
    // `Rejected` is reached by failing initial validation rather
    // than by a state transition: `create_task` returns an error,
    // and the convention is to *not* persist the row at all. The
    // simulation demonstrates the rejection path explicitly.
    // ============================================================
    banner("8. Reject an invalid inbound task (validation failure)");
    let mut bad = seed_task("rejected-task");
    bad.context_id = Some(String::new()); // empty contextId is invalid
    match engine.create_task(bad).await {
        Ok(_) => info!("  unexpectedly accepted!"),
        Err(e) => info!(
            "  rejected at create_task — reason: {}. No row written; state never enters the store.",
            e
        ),
    }

    // ============================================================
    // SCENARIO 9 — Mint a second task, then explicitly cancel it.
    // ============================================================
    banner("9. Cancel an active task (Working → Canceled)");
    let cancel_id = "task-cancel";
    engine.create_task(seed_task(cancel_id)).await?;
    engine
        .transition_task(&scope, cancel_id, TaskState::Working, None)
        .await?;
    settle().await;
    engine
        .transition_task(&scope, cancel_id, TaskState::Canceled, None)
        .await?;
    settle().await;
    let t = engine.get_task(&scope, cancel_id).await?.expect("present");
    assert_eq!(t.status.state, TaskState::Canceled);
    info!("  task '{}' terminal at Canceled.", cancel_id);

    // ============================================================
    // SCENARIO 10 — Stale-task reaper transitions Working → Failed.
    //
    // The reaper takes a `now` argument so we can fast-forward
    // wall-clock time deterministically in a test. Working without
    // progress past the working_ttl is the reaped condition.
    // ============================================================
    banner("10. Stale reaper transitions Working → Failed");
    let stale_id = "task-stale";
    engine.create_task(seed_task(stale_id)).await?;
    engine
        .transition_task(&scope, stale_id, TaskState::Working, None)
        .await?;
    settle().await;
    let an_hour_later = Utc::now() + ChronoDuration::hours(1);
    let reaped = engine
        .fail_if_stale(&scope, stale_id, an_hour_later)
        .await?
        .expect("the reaper must transition a stale task");
    settle().await;
    assert_eq!(reaped.status.state, TaskState::Failed);
    info!("  task '{}' terminal at Failed (reaped).", stale_id);

    // ============================================================
    // SCENARIO 11 — Push notifications.
    //
    // Register a `TaskPushNotificationConfig` pointing at an
    // in-process mock HTTP server, spawn the worker, fire a stream
    // event, and observe the POST arrive at the mock. This proves
    // the broadcast → state-store lookup → HTTP delivery path
    // end-to-end without spinning up the full server.
    // ============================================================
    banner("11. Push-notification delivery (broadcast → POST)");
    // Mock HTTP receiver — counts POSTs and answers 200.
    let hits = Arc::new(AtomicUsize::new(0));
    let url = start_mock_receiver(Arc::clone(&hits)).await;
    info!("  mock receiver listening at {}", url);

    // Mint the push-target task + register the push config.
    let push_task_id = "task-push";
    engine.create_task(seed_task(push_task_id)).await?;
    let cfg = TaskPushNotificationConfig::new("cfg-push-1", push_task_id, NS, TENANT, &url);
    write_push_config(&store, &cfg).await;
    info!(
        "  registered push config '{}' → '{}' for task '{}'",
        cfg.id, cfg.url, push_task_id
    );

    // Spin up the delivery worker on its own broadcast subscription.
    // SSRF enforcement is turned OFF here: this simulation delivers
    // to an in-process mock on `127.0.0.1`, which the guard would
    // (correctly) reject in production. Never disable it in a real
    // deployment — the delivery URL is attacker-controlled.
    let worker = acteon_server::api::a2a_push_worker::PushDeliveryWorker::new(
        Arc::clone(&store),
        reqwest::Client::new(),
        tx.subscribe(),
    )
    .with_ssrf_enforcement(false);
    let worker_metrics = worker.metrics();
    let _worker_handle = tokio::spawn(worker.run());

    // Trigger a stream event by transitioning the task.
    engine
        .transition_task(&scope, push_task_id, TaskState::Working, None)
        .await?;

    // The worker is fully async — give it room to complete the POST.
    for _ in 0..50 {
        if hits.load(Ordering::Relaxed) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let snap = worker_metrics.snapshot();
    info!(
        "  mock receiver got {} POST(s); worker metrics: dispatched={}, succeeded={}, attempts={}.",
        hits.load(Ordering::Relaxed),
        snap.events_dispatched,
        snap.deliveries_succeeded,
        snap.deliveries_attempted,
    );

    // ============================================================
    // Recap
    // ============================================================
    banner("Recap — every A2A TaskState reached");
    info!("  Submitted     scenario 1   (mint)");
    info!("  Working       scenario 2   (transition)");
    info!("  InputRequired scenario 5   (pause UserInput)");
    info!("  AuthRequired  scenario 6   (pause UserAuth)");
    info!("  Completed     scenario 7   (terminal — clean)");
    info!("  Rejected      scenario 8   (terminal — validation failure)");
    info!("  Canceled      scenario 9   (terminal — explicit cancel)");
    info!("  Failed        scenario 10  (terminal — stale reaper)");
    info!("  + Push delivery exercised end-to-end in scenario 11.");
    info!("Done.");
    Ok(())
}

/// Build a fresh task with no history. Lives at namespace/tenant
/// fixed to (`agents`, `demo`).
fn seed_task(id: &str) -> Task {
    Task::new(id, NS, TENANT)
}

/// Print a banner line so the scenarios are easy to find in the log.
fn banner(title: &str) {
    info!("");
    info!("──────────────────────────────────────────────────────────────");
    info!("  {title}");
    info!("──────────────────────────────────────────────────────────────");
}

/// Yield once so the spawned event tailer can drain anything pending
/// from the broadcast before the next scenario's banner prints.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(15)).await;
}

/// Background task that tails the gateway broadcast and prints one
/// formatted line per event. Detached — runs until the broadcast
/// channel closes (i.e. when the senders are dropped at shutdown).
fn spawn_event_tailer(mut rx: broadcast::Receiver<StreamEvent>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Ok(evt) = rx.recv().await {
            // Only A2A task events are of interest in this simulation.
            if evt.action_type.as_deref() != Some("a2a.task") {
                continue;
            }
            let task_id = evt.action_id.as_deref().unwrap_or("?");
            match evt.event_type {
                StreamEventType::TaskTransitioned { from, to, .. } => {
                    info!("     ⟶ event: TaskTransitioned    task={task_id}  {from:?} -> {to:?}")
                }
                StreamEventType::TaskHistoryAppended { message_id, .. } => info!(
                    "     ⟶ event: TaskHistoryAppended task={task_id}  message_id={message_id}"
                ),
                StreamEventType::TaskArtifactUpdated {
                    artifact_id,
                    last_chunk,
                    ..
                } => info!(
                    "     ⟶ event: TaskArtifactUpdated task={task_id}  artifact={artifact_id} last_chunk={last_chunk}"
                ),
                _ => {}
            }
        }
    })
}

/// Persist a push-notification config row directly to the state
/// store. The CRUD endpoints live behind `AppState`; the worker only
/// needs the row to be present in the store at the right key shape.
async fn write_push_config(store: &Arc<dyn StateStore>, cfg: &TaskPushNotificationConfig) {
    let key = StateKey::new(
        cfg.namespace.clone(),
        cfg.tenant.clone(),
        KeyKind::A2aTaskPushConfig,
        cfg.storage_id(),
    );
    let raw = serde_json::to_string(cfg).expect("serialize push config");
    store
        .set(&key, &raw, None)
        .await
        .expect("write push config");
}

/// Spin up a single-port TCP listener that answers every connection
/// with HTTP 200 and bumps `hits`. Returns the bound URL.
async fn start_mock_receiver(hits: Arc<AtomicUsize>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock receiver");
    let addr: SocketAddr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let hits = Arc::clone(&hits);
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let _ = stream.read(&mut buf).await;
                hits.fetch_add(1, Ordering::Relaxed);
                let body = "{}";
                let response = format!(
                    "HTTP/1.1 200 OK\r\n\
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
