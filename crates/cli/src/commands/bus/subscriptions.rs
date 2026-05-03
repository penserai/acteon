use std::collections::HashMap;

use acteon_ops::OpsClient;
use acteon_ops::acteon_client::{
    AckOffset, BusConsumeItem, BusSubscriptionFilter, ConsumeBusTopic, CreateSubscription,
};
use clap::{Args, Subcommand};
use futures::StreamExt;
use tracing::{info, warn};

use crate::OutputFormat;
use crate::commands::bus::parse_kv;

#[derive(Args, Debug)]
pub struct SubscriptionsArgs {
    #[command(subcommand)]
    pub command: SubscriptionsCommand,
}

#[derive(Subcommand, Debug)]
pub enum SubscriptionsCommand {
    /// List subscriptions, optionally filtered.
    List {
        #[arg(long)]
        namespace: Option<String>,
        #[arg(long)]
        tenant: Option<String>,
        #[arg(long)]
        topic: Option<String>,
    },
    /// Create a durable subscription (Kafka consumer group).
    Create {
        /// Subscription id (unique within `(namespace, tenant)`).
        #[arg(long)]
        id: String,
        /// Topic Kafka name to consume.
        #[arg(long)]
        topic: String,
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        /// `earliest` or `latest`.
        #[arg(long)]
        starting_offset: Option<String>,
        /// `auto` or `manual`.
        #[arg(long)]
        ack_mode: Option<String>,
        /// Optional dead-letter topic Kafka name.
        #[arg(long)]
        dead_letter_topic: Option<String>,
        #[arg(long)]
        ack_timeout_ms: Option<u64>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long = "label", value_parser = parse_kv)]
        labels: Vec<(String, String)>,
    },
    /// Delete a subscription by `(namespace, tenant, id)`.
    Delete {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        id: String,
    },
    /// Report per-partition lag for a subscription's group.
    Lag {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        id: String,
    },
    /// Commit an offset on behalf of the subscription's group.
    /// **Performance warning**: full broker round-trip per call —
    /// suitable for end-of-batch checkpoints, not per-record acks.
    Ack {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        id: String,
        #[arg(long)]
        partition: i32,
        #[arg(long)]
        offset: i64,
    },
    /// Consume a subscription via SSE. Prints each record as JSONL on
    /// stdout (one JSON object per line). Stops on Ctrl-C or after
    /// `--limit` records.
    Consume {
        /// Subscription id (Kafka consumer group).
        #[arg(long)]
        id: String,
        /// Full Kafka topic name (`namespace.tenant.name`).
        #[arg(long)]
        topic: String,
        /// `earliest` or `latest` (default: server default).
        #[arg(long)]
        from: Option<String>,
        /// Stop after this many records. Defaults to unbounded.
        #[arg(long)]
        limit: Option<usize>,
    },
}

#[allow(clippy::too_many_lines)]
pub async fn run(
    ops: &OpsClient,
    args: &SubscriptionsArgs,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    match &args.command {
        SubscriptionsCommand::List {
            namespace,
            tenant,
            topic,
        } => {
            let filter = BusSubscriptionFilter {
                namespace: namespace.clone(),
                tenant: tenant.clone(),
                topic: topic.clone(),
            };
            let resp = ops.client().list_bus_subscriptions(&filter).await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&resp)?),
                OutputFormat::Text => {
                    info!(count = resp.count, "Bus subscriptions");
                    for s in &resp.subscriptions {
                        info!(
                            id = %s.id,
                            topic = %s.topic,
                            namespace = %s.namespace,
                            tenant = %s.tenant,
                            starting_offset = %s.starting_offset,
                            ack_mode = %s.ack_mode,
                            "Subscription"
                        );
                    }
                }
            }
        }
        SubscriptionsCommand::Create {
            id,
            topic,
            namespace,
            tenant,
            starting_offset,
            ack_mode,
            dead_letter_topic,
            ack_timeout_ms,
            description,
            labels,
        } => {
            let req = CreateSubscription {
                id: id.clone(),
                topic: topic.clone(),
                namespace: namespace.clone(),
                tenant: tenant.clone(),
                starting_offset: starting_offset.clone(),
                ack_mode: ack_mode.clone(),
                dead_letter_topic: dead_letter_topic.clone(),
                ack_timeout_ms: *ack_timeout_ms,
                description: description.clone(),
                labels: labels.iter().cloned().collect::<HashMap<_, _>>(),
            };
            let sub = ops.client().create_bus_subscription(&req).await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&sub)?),
                OutputFormat::Text => info!(
                    id = %sub.id,
                    topic = %sub.topic,
                    "Subscription created"
                ),
            }
        }
        SubscriptionsCommand::Delete {
            namespace,
            tenant,
            id,
        } => {
            ops.client()
                .delete_bus_subscription(namespace, tenant, id)
                .await?;
            info!(id = %id, "Subscription deleted");
        }
        SubscriptionsCommand::Lag {
            namespace,
            tenant,
            id,
        } => {
            let lag = ops.client().get_bus_lag(namespace, tenant, id).await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&lag)?),
                OutputFormat::Text => {
                    info!(
                        id = %lag.subscription_id,
                        topic = %lag.topic,
                        total_lag = lag.total_lag,
                        "Subscription lag"
                    );
                    for p in &lag.partitions {
                        info!(
                            partition = p.partition,
                            committed = p.committed,
                            high_water = p.high_water_mark,
                            lag = p.lag,
                            "Partition"
                        );
                    }
                }
            }
        }
        SubscriptionsCommand::Ack {
            namespace,
            tenant,
            id,
            partition,
            offset,
        } => {
            ops.client()
                .ack_bus_subscription(
                    namespace,
                    tenant,
                    id,
                    AckOffset {
                        partition: *partition,
                        offset: *offset,
                    },
                )
                .await?;
            info!(
                id = %id,
                partition = *partition,
                offset = *offset,
                "Offset committed"
            );
        }
        SubscriptionsCommand::Consume {
            id,
            topic,
            from,
            limit,
        } => {
            let params = ConsumeBusTopic {
                topic: topic.clone(),
                from: from.clone(),
                reconnect: None,
            };
            // JSONL on stdout regardless of `--format` — a streaming
            // feed is awkward to surface through tracing, and JSONL is
            // the canonical pipe-friendly shape. `info!` would
            // interleave the prefix.
            let _ = format;
            let mut stream = ops.client().consume_bus_subscription(id, &params).await?;
            let mut count = 0usize;
            while let Some(item) = stream.next().await {
                match item? {
                    BusConsumeItem::Message(msg) => {
                        println!("{}", serde_json::to_string(&msg)?);
                        count += 1;
                        if let Some(max) = limit
                            && count >= *max
                        {
                            break;
                        }
                    }
                    BusConsumeItem::Error { message } => {
                        warn!(error = %message, "bus.error");
                    }
                    BusConsumeItem::KeepAlive => {}
                    BusConsumeItem::Reconnected {
                        backoff_ms,
                        attempt,
                    } => {
                        warn!(
                            backoff_ms,
                            attempt,
                            "bus subscription reconnected (best-effort tail; gaps possible)"
                        );
                    }
                    // The non-exhaustive arm: forwards-compat with new
                    // BusConsumeItem variants on the client side.
                    other => {
                        warn!(?other, "unhandled bus consume item — bump the SDK?");
                    }
                }
            }
        }
    }
    Ok(())
}
