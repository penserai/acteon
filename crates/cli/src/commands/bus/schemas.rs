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
        namespace: Option<String>,
        #[arg(long)]
        tenant: Option<String>,
        #[arg(long)]
        subject: Option<String>,
        /// Show only the latest version per subject.
        #[arg(long)]
        latest_only: bool,
    },
    /// Fetch every version of a subject (oldest-to-newest).
    Versions {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        subject: String,
    },
    /// Fetch a specific schema version. `--version` accepts `latest`
    /// or any numeric version.
    Get {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
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
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
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
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        subject: String,
        #[arg(long)]
        version: i32,
    },
    /// Bind a topic to a schema subject + version.
    Bind {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
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
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        topic: String,
    },
}

#[allow(clippy::too_many_lines)]
pub async fn run(ops: &OpsClient, args: &SchemasArgs, format: &OutputFormat) -> anyhow::Result<()> {
    match &args.command {
        SchemasCommand::List {
            namespace,
            tenant,
            subject,
            latest_only,
        } => {
            let filter = BusSchemaFilter {
                namespace: namespace.clone(),
                tenant: tenant.clone(),
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
        SchemasCommand::Versions {
            namespace,
            tenant,
            subject,
        } => {
            let resp = ops
                .client()
                .get_bus_schema_versions(namespace, tenant, subject)
                .await?;
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
        SchemasCommand::Get {
            namespace,
            tenant,
            subject,
            version,
        } => {
            let s = ops
                .client()
                .get_bus_schema(namespace, tenant, subject, version)
                .await?;
            info!("{}", serde_json::to_string_pretty(&s)?);
        }
        SchemasCommand::Register {
            subject,
            namespace,
            tenant,
            body,
            labels,
        } => {
            let body = parse_json_arg(body)?;
            let req = RegisterBusSchema {
                subject: subject.clone(),
                namespace: namespace.clone(),
                tenant: tenant.clone(),
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
        SchemasCommand::Delete {
            namespace,
            tenant,
            subject,
            version,
        } => {
            ops.client()
                .delete_bus_schema(namespace, tenant, subject, *version)
                .await?;
            info!(subject = %subject, version = *version, "Schema deleted");
        }
        SchemasCommand::Bind {
            namespace,
            tenant,
            topic,
            subject,
            version,
        } => {
            let resp = ops
                .client()
                .bind_topic_schema(namespace, tenant, topic, subject, *version)
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
        SchemasCommand::Unbind {
            namespace,
            tenant,
            topic,
        } => {
            ops.client()
                .unbind_topic_schema(namespace, tenant, topic)
                .await?;
            info!(topic = %topic, "Topic schema binding removed");
        }
    }
    Ok(())
}
