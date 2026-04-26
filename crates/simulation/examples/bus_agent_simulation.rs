//! Phase 4 agent-registry + shared-inbox demo.
//!
//! This sim drives the core `Agent` type and the in-memory bus
//! backend directly — no Kafka, no HTTP — so you can see the shape
//! of Phase 4 in isolation. The actual HTTP endpoints
//! (`POST /v1/bus/agents`, `/heartbeat`, `/send`) delegate to the
//! same types on the server side.
//!
//! Scenarios:
//!
//! 1. Register two agents in the same `(namespace, tenant)` — both
//!    share one inbox topic.
//! 2. Discover agents by capability.
//! 3. Show status transitions: Unknown → Online → Idle → Dead as the
//!    last-heartbeat timestamp ages past the TTL thresholds.
//! 4. Produce messages keyed by `agent_id` and observe them arriving
//!    on the shared inbox topic. Since Kafka's partitioner is
//!    deterministic on key, each agent's traffic stays on one
//!    partition (we verify the key makes it through the backend
//!    round-trip).
//!
//! Run with:
//! ```text
//! cargo run -p acteon-simulation --features bus --example bus_agent_simulation
//! ```

use std::time::Duration;

use chrono::Utc;
use futures::StreamExt;
use tracing::{Level, info};

use acteon_bus::{BusMessage, MemoryBackend, StartOffset};
use acteon_core::{Agent, AgentStatus, Topic};

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_env_filter("info,acteon_bus=info")
        .init();

    let backend: acteon_bus::SharedBackend = MemoryBackend::new();

    // -----------------------------------------------------------------
    // 1. Register two agents, both pointing at the default shared inbox
    // -----------------------------------------------------------------

    let mut planner = Agent::new("planner-1", "agents", "demo");
    planner.display_name = Some("Planner One".into());
    planner.capabilities = vec!["planning".into(), "reasoning".into()];
    planner.validate()?;

    let mut ocr = Agent::new("ocr-svc", "agents", "demo");
    ocr.display_name = Some("OCR Worker".into());
    ocr.capabilities = vec!["ocr".into(), "vision".into()];
    ocr.validate()?;

    assert_eq!(planner.effective_inbox_topic(), "agents.demo.agents-inbox");
    assert_eq!(planner.effective_inbox_topic(), ocr.effective_inbox_topic());
    info!(
        inbox = %planner.effective_inbox_topic(),
        "registered two agents on the shared inbox"
    );

    // Provision the inbox topic once (the server does this on first
    // register_agent; here we do it directly).
    let inbox = Topic::new("agents-inbox", "agents", "demo");
    backend.create_topic(&inbox).await?;

    // -----------------------------------------------------------------
    // 2. Capability discovery
    // -----------------------------------------------------------------

    let agents = [&planner, &ocr];
    let ocr_candidates: Vec<&Agent> = agents
        .iter()
        .copied()
        .filter(|a| a.capabilities.iter().any(|c| c == "ocr"))
        .collect();
    info!(
        count = ocr_candidates.len(),
        picked = %ocr_candidates[0].agent_id,
        "discovered agents advertising the 'ocr' capability"
    );

    // -----------------------------------------------------------------
    // 3. Heartbeat-derived status transitions
    // -----------------------------------------------------------------

    let now = Utc::now();
    // No heartbeat → Unknown.
    assert_eq!(planner.status_at(now), AgentStatus::Unknown);
    info!("fresh agent reports status=Unknown");

    planner.last_heartbeat_at = Some(now);
    assert_eq!(planner.status_at(now), AgentStatus::Online);
    info!("after heartbeat, status=Online");

    // Move "now" forward past single TTL (60s default).
    let t_idle = now + chrono::Duration::milliseconds(planner.heartbeat_ttl_ms + 5_000);
    assert_eq!(planner.status_at(t_idle), AgentStatus::Idle);
    info!(
        age_ms = planner.heartbeat_ttl_ms + 5_000,
        "after ttl but within 2*ttl, status=Idle"
    );

    // Past 2x TTL → Dead.
    let t_dead = now + chrono::Duration::milliseconds((planner.heartbeat_ttl_ms * 2) + 5_000);
    assert_eq!(planner.status_at(t_dead), AgentStatus::Dead);
    info!(
        age_ms = (planner.heartbeat_ttl_ms * 2) + 5_000,
        "past 2*ttl, status=Dead"
    );

    // -----------------------------------------------------------------
    // 4. Send-to-agent: produce keyed by agent_id and observe delivery
    // -----------------------------------------------------------------

    // Attach a subscriber before producing.
    let mut stream = backend
        .subscribe(
            &planner.effective_inbox_topic(),
            "sim-consumer",
            StartOffset::Earliest,
        )
        .await?;

    // Send one message to each agent. `with_header` filters reserved
    // `acteon.*` keys (an anti-spoofing guard on user input), so the
    // server — and this simulation — populate them via direct
    // `headers.insert`.
    let mut planner_msg = BusMessage::new(
        planner.effective_inbox_topic(),
        serde_json::json!({"task": "break down user request"}),
    )
    .with_key(&planner.agent_id);
    planner_msg
        .headers
        .insert("acteon.agent.id".into(), planner.agent_id.clone());
    let mut ocr_msg = BusMessage::new(
        ocr.effective_inbox_topic(),
        serde_json::json!({"task": "extract text", "image": "s3://.../receipt.png"}),
    )
    .with_key(&ocr.agent_id);
    ocr_msg
        .headers
        .insert("acteon.agent.id".into(), ocr.agent_id.clone());
    backend.produce(planner_msg).await?;
    backend.produce(ocr_msg).await?;

    let mut seen: Vec<(String, String)> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while seen.len() < 2 && tokio::time::Instant::now() < deadline {
        tokio::select! {
            maybe = stream.next() => {
                if let Some(Ok(msg)) = maybe {
                    let key = msg.key.clone().unwrap_or_default();
                    let recipient = msg
                        .headers
                        .get("acteon.agent.id")
                        .cloned()
                        .unwrap_or_default();
                    seen.push((key, recipient));
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
    assert_eq!(seen.len(), 2, "expected both inbox messages");
    for (key, recipient) in &seen {
        assert_eq!(key, recipient, "inbox key must equal agent_id header");
        info!(agent = %key, "inbox message received");
    }

    info!("agent simulation complete");
    Ok(())
}
