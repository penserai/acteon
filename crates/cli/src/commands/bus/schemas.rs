use std::collections::HashMap;

use acteon_ops::OpsClient;
use acteon_ops::acteon_client::{BusSchemaFilter, RegisterBusSchema};
use clap::{Args, Subcommand};
use tracing::info;

use crate::OutputFormat;
use crate::commands::bus::{parse_json_arg, parse_kv};

#[derive(Args, Debug)]
pub struct SchemasArgs {
    #[command(subcommand)]
    pub command: SchemasCommand,
}

#[derive(Subcommand, Debug)]
pub enum SchemasCommand {
    /// List schemas, optionally filtered.
    List {
        #[arg(long)]
        subject: Option<String>,
        /// Show only the latest version per subject.
        #[arg(long)]
        latest_only: bool,
    },
    /// Fetch every version of a subject (oldest-to-newest).
    Versions {
        #[arg(long)]
        subject: String,
    },
    /// Fetch a specific schema version. `--version` accepts `latest`
    /// or any numeric version.
    Get {
        #[arg(long)]
        subject: String,
        #[arg(long, default_value = "latest")]
        version: String,
    },
    /// Register (or bump) a schema subject. The server allocates the
    /// next monotonic version.
    Register {
        #[arg(long)]
        subject: String,
        /// JSON Schema body, or `@path/to/schema.json`.
        #[arg(long)]
        body: String,
        #[arg(long = "label", value_parser = parse_kv)]
        labels: Vec<(String, String)>,
    },
    /// Delete a specific schema version. Fails with 409 if any topic
    /// pins it.
    Delete {
        #[arg(long)]
        subject: String,
        #[arg(long)]
        version: i32,
    },
    /// Bind a topic to a schema subject + version.
    Bind {
        /// Logical topic name (no `namespace.tenant.` prefix).
        #[arg(long)]
        topic: String,
        #[arg(long)]
        subject: String,
        #[arg(long)]
        version: i32,
    },
    /// Drop a topic's schema binding.
    Unbind {
        #[arg(long)]
        topic: String,
    },
}

#[allow(clippy::too_many_lines)]
pub async fn run(ops: &OpsClient, args: &SchemasArgs, format: &OutputFormat) -> anyhow::Result<()> {
    match &args.command {
        SchemasCommand::List {
            subject,
            latest_only,
        } => {
            let filter = BusSchemaFilter {
                subject: subject.clone(),
                latest_only: *latest_only,
            };
            let resp = ops.client().list_bus_schemas(&filter).await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&resp)?),
                OutputFormat::Text => {
                    info!(count = resp.count, "Bus schemas");
                    for s in &resp.schemas {
                        info!(
                            subject = %s.subject,
                            version = s.version,
                            namespace = %s.namespace,
                            tenant = %s.tenant,
                            "Schema"
                        );
                    }
                }
            }
        }
        SchemasCommand::Versions { subject } => {
            let resp = ops.client().get_bus_schema_versions(subject).await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&resp)?),
                OutputFormat::Text => {
                    info!(count = resp.count, subject = %subject, "Schema versions");
                    for s in &resp.schemas {
                        info!(
                            subject = %s.subject,
                            version = s.version,
                            created_at = %s.created_at,
                            "Schema"
                        );
                    }
                }
            }
        }
        SchemasCommand::Get { subject, version } => {
            let s = ops.client().get_bus_schema(subject, version).await?;
            info!("{}", serde_json::to_string_pretty(&s)?);
        }
        SchemasCommand::Register {
            subject,

            body,
            labels,
        } => {
            let body = parse_json_arg(body)?;
            let req = RegisterBusSchema {
                subject: subject.clone(),

                body,
                labels: labels.iter().cloned().collect::<HashMap<_, _>>(),
            };
            let s = ops.client().register_bus_schema(&req).await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&s)?),
                OutputFormat::Text => info!(
                    subject = %s.subject,
                    version = s.version,
                    "Schema registered"
                ),
            }
        }
        SchemasCommand::Delete { subject, version } => {
            ops.client().delete_bus_schema(subject, *version).await?;
            info!(subject = %subject, version = *version, "Schema deleted");
        }
        SchemasCommand::Bind {
            topic,
            subject,
            version,
        } => {
            let resp = ops
                .client()
                .bind_topic_schema(topic, subject, *version)
                .await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&resp)?),
                OutputFormat::Text => info!(
                    topic = %resp.topic,
                    subject = %resp.subject,
                    version = resp.version,
                    "Topic bound to schema"
                ),
            }
        }
        SchemasCommand::Unbind { topic } => {
            ops.client().unbind_topic_schema(topic).await?;
            info!(topic = %topic, "Topic schema binding removed");
        }
    }
    Ok(())
}
