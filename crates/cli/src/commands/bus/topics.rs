use std::collections::HashMap;

use acteon_ops::OpsClient;
use acteon_ops::acteon_client::{BusTopicFilter, CreateBusTopic, PublishBusMessage};
use clap::{Args, Subcommand};
use tracing::info;

use crate::OutputFormat;
use crate::commands::bus::{parse_json_arg, parse_kv};

#[derive(Args, Debug)]
pub struct TopicsArgs {
    #[command(subcommand)]
    pub command: TopicsCommand,
}

#[derive(Subcommand, Debug)]
pub enum TopicsCommand {
    /// List bus topics, optionally filtered by namespace/tenant.
    List {
        #[arg(long)]
        namespace: Option<String>,
        #[arg(long)]
        tenant: Option<String>,
    },
    /// Create a bus topic. Persists in Acteon state and creates the
    /// backing Kafka topic.
    Create {
        /// Logical topic name (Kafka name is `namespace.tenant.name`).
        #[arg(long)]
        name: String,
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        partitions: Option<i32>,
        #[arg(long)]
        replication_factor: Option<i16>,
        #[arg(long)]
        retention_ms: Option<i64>,
        #[arg(long)]
        description: Option<String>,
        /// Repeat for multiple labels: `--label key=value`.
        #[arg(long = "label", value_parser = parse_kv)]
        labels: Vec<(String, String)>,
    },
    /// Delete a bus topic by Kafka name (`namespace.tenant.name`).
    Delete {
        /// Kafka topic name, e.g. `agents.demo.events`.
        kafka_name: String,
    },
    /// Publish a single message to a topic.
    Publish {
        /// Either the full `namespace.tenant.name` form...
        #[arg(long)]
        topic: Option<String>,
        /// ...or the three parts spelled out separately.
        #[arg(long)]
        namespace: Option<String>,
        #[arg(long)]
        tenant: Option<String>,
        #[arg(long)]
        name: Option<String>,
        /// Partition key (Kafka routes by this).
        #[arg(long)]
        key: Option<String>,
        /// JSON payload, or `@path/to/file.json`.
        #[arg(long)]
        payload: String,
        /// Repeat for multiple headers: `--header key=value`.
        #[arg(long = "header", value_parser = parse_kv)]
        headers: Vec<(String, String)>,
    },
}

#[allow(clippy::too_many_lines)]
pub async fn run(ops: &OpsClient, args: &TopicsArgs, format: &OutputFormat) -> anyhow::Result<()> {
    match &args.command {
        TopicsCommand::List { namespace, tenant } => {
            let filter = BusTopicFilter {
                namespace: namespace.clone(),
                tenant: tenant.clone(),
            };
            let resp = ops.client().list_bus_topics(&filter).await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&resp)?),
                OutputFormat::Text => {
                    info!(count = resp.count, "Bus topics");
                    for t in &resp.topics {
                        info!(
                            name = %t.name,
                            namespace = %t.namespace,
                            tenant = %t.tenant,
                            kafka = %t.kafka_name,
                            partitions = t.partitions,
                            replication_factor = t.replication_factor,
                            "Topic"
                        );
                    }
                }
            }
        }
        TopicsCommand::Create {
            name,
            namespace,
            tenant,
            partitions,
            replication_factor,
            retention_ms,
            description,
            labels,
        } => {
            let req = CreateBusTopic {
                name: name.clone(),
                namespace: namespace.clone(),
                tenant: tenant.clone(),
                partitions: *partitions,
                replication_factor: *replication_factor,
                retention_ms: *retention_ms,
                description: description.clone(),
                labels: labels.iter().cloned().collect::<HashMap<_, _>>(),
            };
            let topic = ops.client().create_bus_topic(&req).await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&topic)?),
                OutputFormat::Text => info!(
                    name = %topic.name,
                    kafka = %topic.kafka_name,
                    partitions = topic.partitions,
                    "Topic created"
                ),
            }
        }
        TopicsCommand::Delete { kafka_name } => {
            ops.client().delete_bus_topic(kafka_name).await?;
            info!(kafka = %kafka_name, "Topic deleted");
        }
        TopicsCommand::Publish {
            topic,
            namespace,
            tenant,
            name,
            key,
            payload,
            headers,
        } => {
            let payload = parse_json_arg(payload)?;
            let msg = PublishBusMessage {
                topic: topic.clone(),
                namespace: namespace.clone(),
                tenant: tenant.clone(),
                name: name.clone(),
                key: key.clone(),
                payload,
                headers: headers.iter().cloned().collect(),
            };
            let receipt = ops.client().publish_message(&msg).await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&receipt)?),
                OutputFormat::Text => info!(
                    topic = %receipt.topic,
                    partition = receipt.partition,
                    offset = receipt.offset,
                    produced_at = %receipt.produced_at,
                    "Message published"
                ),
            }
        }
    }
    Ok(())
}
