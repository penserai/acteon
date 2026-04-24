//! End-to-end exercise of the Phase 2 subscription surface.
//!
//! What this simulation demonstrates:
//!
//! 1. Create a primary topic + a dead-letter topic.
//! 2. Produce a batch of records, some of which the consumer will mark
//!    as failures (destined for the DLQ).
//! 3. A consumer attaches, processes each record, commits successes,
//!    and routes failures to the DLQ.
//! 4. Reconnect a fresh consumer in the same group — it resumes from
//!    the committed offset.
//! 5. Query `consumer_lag` to prove the commit took effect.
//! 6. Tail the DLQ and print the failure records.
//!
//! Prerequisite: `docker compose --profile kafka up -d`.
//!
//! Run with:
//! ```text
//! ACTEON_KAFKA_BOOTSTRAP=localhost:9092 \
//!   cargo run -p acteon-simulation --features bus \
//!   --example bus_subscription_simulation
//! ```

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use tracing::{Level, info};

use acteon_bus::{
    BusBackend, BusMessage, KafkaBackend, KafkaBusConfig, OffsetPosition, StartOffset,
};
use acteon_core::Topic;

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_env_filter("info,acteon_bus=info")
        .try_init()
        .ok();

    let bootstrap =
        std::env::var("ACTEON_KAFKA_BOOTSTRAP").unwrap_or_else(|_| "localhost:9092".to_string());
    let cfg = KafkaBusConfig {
        bootstrap_servers: bootstrap,
        client_id: "bus-sub-sim".into(),
        produce_timeout_ms: 8_000,
        extra: Vec::new(),
    };
    let backend: Arc<dyn BusBackend> = KafkaBackend::new(&cfg)?;

    let run_id = uuid::Uuid::new_v4().simple().to_string();
    let mut orders = Topic::new(format!("orders-{run_id}"), "agents", "demo");
    orders.partitions = 1;
    orders.replication_factor = 1;
    backend.create_topic(&orders).await?;

    let mut dlq = Topic::new(format!("orders-dlq-{run_id}"), "agents", "demo");
    dlq.partitions = 1;
    dlq.replication_factor = 1;
    backend.create_topic(&dlq).await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let orders_name = orders.kafka_topic_name();
    let dlq_name = dlq.kafka_topic_name();
    let group = format!("order-processor-{run_id}");

    info!(topic = %orders_name, dlq = %dlq_name, %group, "topics ready");

    // Produce 6 records. Seq 2 and 4 are destined for the DLQ.
    for i in 0..6 {
        backend
            .produce(
                BusMessage::new(
                    orders_name.clone(),
                    serde_json::json!({ "seq": i, "amount": 10 + i }),
                )
                .with_header("x-trace-id", format!("tr-{i}")),
            )
            .await?;
    }

    // Phase A: consumer #1 processes 3 records (DLQing seq 2),
    // collects the final offset, drops the consumer, *then* commits.
    // Kafka only allows a commit from a consumer that currently owns
    // the partition — since Phase 2's `commit_offset` API uses its own
    // short-lived consumer, the primary must be dropped first.
    let mut last_committed: Option<(i32, i64)> = None;
    {
        let mut stream = backend
            .subscribe(&orders_name, &group, StartOffset::Earliest)
            .await?;
        let mut processed = 0usize;
        while processed < 3 {
            let Ok(Some(Ok(msg))) =
                tokio::time::timeout(Duration::from_secs(10), stream.next()).await
            else {
                break;
            };
            let seq = msg.payload["seq"].as_i64().unwrap_or(-1);
            if seq == 2 {
                let dlq_record = BusMessage::new(dlq_name.clone(), msg.payload.clone())
                    .with_key(msg.key.clone().unwrap_or_default());
                backend.produce(dlq_record).await?;
                info!(seq, offset = ?msg.offset, "routed to DLQ");
            } else {
                info!(seq, offset = ?msg.offset, "processed");
            }
            if let (Some(p), Some(o)) = (msg.partition, msg.offset) {
                last_committed = Some((p, o));
            }
            processed += 1;
        }
        // `stream` (and its consumer) drops here, unregistering from the group.
    }
    // Give the broker a moment for the LeaveGroup to settle.
    tokio::time::sleep(Duration::from_millis(500)).await;

    if let Some((p, o)) = last_committed {
        backend
            .commit_offset(
                &orders_name,
                &group,
                OffsetPosition {
                    partition: p,
                    offset: o,
                },
            )
            .await?;
        info!(partition = p, offset = o, "committed after phase A");
    }

    let lag_after_phase_a = backend.consumer_lag(&orders_name, &group).await?;
    info!(?lag_after_phase_a, "lag after phase A");

    // Phase B: fresh consumer in the same group — should resume after
    // the committed offset (records 3, 4, 5). We deliberately do *not*
    // commit inline here: the Phase 2 `commit_offset` API spins up its
    // own consumer and can't join a group that already has an active
    // member. Batch-committing after the stream drops (as in phase A)
    // is the supported pattern until a future phase introduces a
    // stateful subscription registry that shares one consumer for both
    // reads and commits.
    let mut seen = Vec::new();
    {
        let mut stream = backend
            .subscribe(&orders_name, &group, StartOffset::Latest)
            .await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        while seen.len() < 3 {
            match tokio::time::timeout_at(deadline, stream.next()).await {
                Ok(Some(Ok(m))) => {
                    let seq = m.payload["seq"].as_i64().unwrap_or(-1);
                    info!(seq, offset = ?m.offset, "resumed consumer");
                    seen.push((m.partition, m.offset, seq));
                }
                _ => break,
            }
        }
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    if let Some((Some(p), Some(o), _)) = seen.last() {
        backend
            .commit_offset(
                &orders_name,
                &group,
                OffsetPosition {
                    partition: *p,
                    offset: *o,
                },
            )
            .await?;
    }
    info!(resumed_seqs = ?seen.iter().map(|(_, _, s)| *s).collect::<Vec<_>>(), "phase B saw these seqs (should start at 3)");

    let lag_final = backend.consumer_lag(&orders_name, &group).await?;
    info!(?lag_final, "lag after phase B");

    // Phase C: tail the DLQ so operators see the failure record.
    {
        let tail_group = format!("dlq-tail-{run_id}");
        let mut stream = backend
            .subscribe(&dlq_name, &tail_group, StartOffset::Earliest)
            .await?;
        let mut dlq_seen = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while let Ok(Some(Ok(m))) = tokio::time::timeout_at(deadline, stream.next()).await {
            dlq_seen.push(m.payload.clone());
            info!(payload = %m.payload, "DLQ record");
        }
        info!(count = dlq_seen.len(), "DLQ tail done");
    }

    // Cleanup.
    backend.delete_topic(&orders_name).await.ok();
    backend.delete_topic(&dlq_name).await.ok();
    info!("bus subscription simulation complete");
    Ok(())
}
