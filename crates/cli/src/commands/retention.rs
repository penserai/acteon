use acteon_ops::OpsClient;
use acteon_ops::acteon_client::{CreateRetentionRequest, UpdateRetentionRequest};
use clap::{Args, Subcommand};
use tracing::{info, warn};

use crate::OutputFormat;

#[derive(Args, Debug)]
pub struct RetentionArgs {
    #[command(subcommand)]
    pub command: RetentionCommand,
}

#[derive(Subcommand, Debug)]
pub enum RetentionCommand {
    /// List retention policies.
    List {
        /// Filter by namespace.
        #[arg(long)]
        namespace: Option<String>,
        /// Filter by tenant.
        #[arg(long)]
        tenant: Option<String>,
    },
    /// Get a retention policy by ID.
    Get {
        /// Retention policy ID.
        id: String,
    },
    /// Create a retention policy.
    Create {
        /// JSON data (string or @file path).
        #[arg(long)]
        data: String,
    },
    /// Update a retention policy.
    Update {
        /// Retention policy ID.
        id: String,
        /// JSON data (string or @file path).
        #[arg(long)]
        data: String,
    },
    /// Delete a retention policy.
    Delete {
        /// Retention policy ID.
        id: String,
    },
}

fn parse_json_data(input: &str) -> anyhow::Result<serde_json::Value> {
    if let Some(path) = input.strip_prefix('@') {
        let content = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&content)?)
    } else {
        Ok(serde_json::from_str(input)?)
    }
}

pub async fn run(
    ops: &OpsClient,
    args: &RetentionArgs,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    match &args.command {
        RetentionCommand::List { namespace, tenant } => {
            let resp = ops
                .list_retention(namespace.clone(), tenant.clone())
                .await?;
            match format {
                OutputFormat::Json => {
                    info!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    info!(count = resp.count, "Retention policies");
                    for p in &resp.policies {
                        let enabled = if p.enabled { "ON " } else { "OFF" };
                        let hold = if p.compliance_hold { " [HOLD]" } else { "" };
                        info!(
                            enabled = %enabled,
                            id = %&p.id[..8.min(p.id.len())],
                            namespace = %p.namespace,
                            tenant = %p.tenant,
                            compliance_hold = %hold,
                            "Retention policy"
                        );
                    }
                }
            }
        }
        RetentionCommand::Get { id } => {
            let resp = ops.get_retention(id).await?;
            match resp {
                Some(p) => match format {
                    OutputFormat::Json => {
                        info!("{}", serde_json::to_string_pretty(&p)?);
                    }
                    OutputFormat::Text => {
                        info!(id = %p.id, "Retention policy details");
                        info!(namespace = %p.namespace, "  Namespace");
                        info!(tenant = %p.tenant, "  Tenant");
                        info!(enabled = p.enabled, "  Enabled");
                        info!(compliance_hold = p.compliance_hold, "  Compliance Hold");
                        if let Some(ttl) = p.audit_ttl_seconds {
                            info!(audit_ttl_seconds = ttl, "  Audit TTL");
                        }
                        if let Some(ttl) = p.state_ttl_seconds {
                            info!(state_ttl_seconds = ttl, "  State TTL");
                        }
                        if let Some(ttl) = p.event_ttl_seconds {
                            info!(event_ttl_seconds = ttl, "  Event TTL");
                        }
                    }
                },
                None => {
                    warn!(id = %id, "Retention policy not found");
                }
            }
        }
        RetentionCommand::Create { data } => {
            let value = parse_json_data(data)?;
            let req: CreateRetentionRequest = serde_json::from_value(value)?;
            let resp = ops.create_retention(&req).await?;
            match format {
                OutputFormat::Json => {
                    info!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    info!(id = %resp.id, "Created retention policy");
                }
            }
        }
        RetentionCommand::Update { id, data } => {
            let value = parse_json_data(data)?;
            let req: UpdateRetentionRequest = serde_json::from_value(value)?;
            let resp = ops.update_retention(id, &req).await?;
            match format {
                OutputFormat::Json => {
                    info!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    info!(id = %resp.id, "Updated retention policy");
                }
            }
        }
        RetentionCommand::Delete { id } => {
            ops.delete_retention(id).await?;
            info!(id = %id, "Retention policy deleted");
        }
    }
    Ok(())
}
