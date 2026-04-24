//! End-to-end exercise of the agentic message bus (Phase 1).
//!
//! What this simulation demonstrates:
//!
//! 1. Create two Kafka-backed topics (`agents.inbox`, `agents.replies`).
//! 2. Start **two** competing consumers on a shared group so work is
//!    distributed.
//! 3. Publish a batch of messages split across two partition keys so
//!    per-key ordering is visible.
//! 4. Each consumer echoes a reply on `agents.replies`.
//! 5. A third consumer tails `agents.replies` and prints the round-trip.
//!
//! Prerequisite: the `kafka` compose profile is up. Run with
//!
//! ```text
//! docker compose --profile kafka up -d
//! ACTEON_KAFKA_BOOTSTRAP=localhost:9092 \
//!   cargo run -p acteon-simulation --features bus \
//!   --example bus_simulation
//! ```

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use tracing::{Level, info};

use acteon_bus::{BusBackend, BusMessage, KafkaBackend, KafkaBusConfig, StartOffset};
use acteon_core::Topic;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_env_filter("info,acteon_bus=info")
        .try_init()
        .ok();

    let bootstrap =
        std::env::var("ACTEON_KAFKA_BOOTSTRAP").unwrap_or_else(|_| "localhost:9092".to_string());

    info!(%bootstrap, "starting bus simulation");

    let cfg = KafkaBusConfig {
        bootstrap_servers: bootstrap,
        client_id: "bus-sim".into(),
        produce_timeout_ms: 8_000,
        extra: Vec::new(),
    };
    let backend: Arc<dyn BusBackend> = KafkaBackend::new(&cfg)?;

    // ---- 1. Create the two topics with unique suffixes per run. -----------
    let run_id = uuid::Uuid::new_v4().simple().to_string();
    let mut inbox = Topic::new(format!("inbox-{run_id}"), "agents", "demo");
    inbox.partitions = 2;
    inbox.replication_factor = 1;
    backend.create_topic(&inbox).await?;

    let mut replies = Topic::new(format!("replies-{run_id}"), "agents", "demo");
    replies.partitions = 1;
    replies.replication_factor = 1;
    backend.create_topic(&replies).await?;

    info!(
        inbox = %inbox.kafka_topic_name(),
        replies = %replies.kafka_topic_name(),
        "created topics"
    );
    // Let the broker propagate topic metadata before consumers attach.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // ---- 2. Start two competing consumers on the same group. ---------------
    let group = format!("agents-{run_id}");
    let agent_a = spawn_echo_agent("agent-A", backend.clone(), &inbox, &replies, &group).await?;
    let agent_b = spawn_echo_agent("agent-B", backend.clone(), &inbox, &replies, &group).await?;

    // ---- 3. A third consumer tails replies and logs them. ------------------
    let tail_group = format!("tail-{run_id}");
    let replies_topic_name = replies.kafka_topic_name();
    let tail_backend = backend.clone();
    let tail = tokio::spawn(async move {
        let mut stream = tail_backend
            .subscribe(&replies_topic_name, &tail_group, StartOffset::Latest)
            .await
            .expect("subscribe replies");
        let mut seen = 0usize;
        while seen < 8 {
            match tokio::time::timeout(Duration::from_secs(15), stream.next()).await {
                Ok(Some(Ok(msg))) => {
                    info!(
                        offset = ?msg.offset,
                        key = msg.key.as_deref().unwrap_or(""),
                        payload = %msg.payload,
                        "observed reply"
                    );
                    seen += 1;
                }
                _ => break,
            }
        }
    });

    // Give the agent + tail consumers time to join their groups before
    // we produce — avoids losing messages on the Latest-offset tail.
    tokio::time::sleep(Duration::from_secs(2)).await;

    // ---- 4. Publish 8 messages across two partition keys. ------------------
    for i in 0..8 {
        let key = if i % 2 == 0 { "alpha" } else { "beta" };
        let msg = BusMessage::new(
            inbox.kafka_topic_name(),
            serde_json::json!({ "seq": i, "key": key }),
        )
        .with_key(key)
        .with_header("x-trace-id", format!("trace-{i}"));
        let receipt = backend.produce(msg).await?;
        info!(
            seq = i,
            key,
            partition = receipt.partition,
            offset = receipt.offset,
            "published"
        );
    }

    // ---- 5. Wait for agents and tail to drain. -----------------------------
    let _ = tokio::time::timeout(Duration::from_secs(20), tail).await;
    agent_a.abort();
    agent_b.abort();

    // ---- 6. Clean up. ------------------------------------------------------
    backend.delete_topic(&inbox.kafka_topic_name()).await.ok();
    backend.delete_topic(&replies.kafka_topic_name()).await.ok();

    info!("bus simulation complete");
    Ok(())
}

async fn spawn_echo_agent(
    agent: &'static str,
    backend: Arc<dyn BusBackend>,
    inbox: &Topic,
    replies: &Topic,
    group: &str,
) -> Result<tokio::task::JoinHandle<()>, Box<dyn std::error::Error>> {
    let mut stream = backend
        .subscribe(&inbox.kafka_topic_name(), group, StartOffset::Latest)
        .await?;
    let replies_name = replies.kafka_topic_name();
    let b = backend.clone();
    let handle = tokio::spawn(async move {
        while let Some(item) = stream.next().await {
            match item {
                Ok(msg) => {
                    info!(
                        agent,
                        offset = ?msg.offset,
                        key = msg.key.as_deref().unwrap_or(""),
                        payload = %msg.payload,
                        "consumed"
                    );
                    let reply = BusMessage::new(
                        replies_name.clone(),
                        serde_json::json!({
                            "from": agent,
                            "original": msg.payload,
                        }),
                    )
                    .with_key(msg.key.clone().unwrap_or_default());
                    if let Err(e) = b.produce(reply).await {
                        tracing::warn!(agent, error = %e, "failed to produce reply");
                    }
                }
                Err(e) => {
                    tracing::warn!(agent, error = %e, "consumer error");
                    break;
                }
            }
        }
    });
    Ok(handle)
}
