use acteon_ops::OpsClient;
use acteon_ops::acteon_client::{CreateQuotaRequest, UpdateQuotaRequest};
use clap::{Args, Subcommand};
use tracing::{info, warn};

use crate::OutputFormat;

#[derive(Args, Debug)]
pub struct QuotasArgs {
    #[command(subcommand)]
    pub command: QuotasCommand,
}

#[derive(Subcommand, Debug)]
pub enum QuotasCommand {
    /// List quota policies.
    List {
        /// Filter by namespace.
        #[arg(long)]
        namespace: Option<String>,
        /// Filter by tenant.
        #[arg(long)]
        tenant: Option<String>,
        /// Filter by provider scope. Pass the literal `generic`
        /// to list only policies without a provider scope, or a
        /// provider name (e.g. `slack`) to list only per-provider
        /// policies for that provider.
        #[arg(long)]
        provider: Option<String>,
    },
    /// Get a quota policy by ID.
    Get {
        /// Quota policy ID.
        id: String,
    },
    /// Create a quota policy.
    Create {
        /// JSON data (string or @file path).
        #[arg(long)]
        data: String,
    },
    /// Update a quota policy.
    Update {
        /// Quota policy ID.
        id: String,
        /// JSON data (string or @file path).
        #[arg(long)]
        data: String,
    },
    /// Delete a quota policy.
    Delete {
        /// Quota policy ID.
        id: String,
        /// Namespace.
        #[arg(long, default_value = "default")]
        namespace: String,
        /// Tenant.
        #[arg(long)]
        tenant: String,
    },
    /// Get quota usage.
    Usage {
        /// Quota policy ID.
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

pub async fn run(ops: &OpsClient, args: &QuotasArgs, format: &OutputFormat) -> anyhow::Result<()> {
    match &args.command {
        QuotasCommand::List {
            namespace,
            tenant,
            provider,
        } => {
            run_list(
                ops,
                namespace.as_ref(),
                tenant.as_ref(),
                provider.as_ref(),
                format,
            )
            .await
        }
        QuotasCommand::Get { id } => run_get(ops, id, format).await,
        QuotasCommand::Create { data } => run_create(ops, data, format).await,
        QuotasCommand::Update { id, data } => run_update(ops, id, data, format).await,
        QuotasCommand::Delete {
            id,
            namespace,
            tenant,
        } => {
            ops.delete_quota(id, namespace, tenant).await?;
            info!(id = %id, "Quota deleted");
            Ok(())
        }
        QuotasCommand::Usage { id } => run_usage(ops, id, format).await,
    }
}

async fn run_list(
    ops: &OpsClient,
    namespace: Option<&String>,
    tenant: Option<&String>,
    provider: Option<&String>,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    let resp = ops
        .list_quotas(
            namespace.map(String::as_str),
            tenant.map(String::as_str),
            provider.map(String::as_str),
        )
        .await?;
    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            info!(count = resp.count, "Quotas");
            for q in &resp.quotas {
                let enabled = if q.enabled { "ON " } else { "OFF" };
                let scope = q.provider.as_deref().unwrap_or("*");
                info!(
                    enabled = %enabled,
                    id = %&q.id[..8.min(q.id.len())],
                    namespace = %q.namespace,
                    tenant = %q.tenant,
                    provider = %scope,
                    max_actions = q.max_actions,
                    window = %q.window,
                    overage_behavior = %q.overage_behavior,
                    "Quota"
                );
            }
        }
    }
    Ok(())
}

async fn run_get(ops: &OpsClient, id: &str, format: &OutputFormat) -> anyhow::Result<()> {
    let resp = ops.get_quota(id).await?;
    match resp {
        Some(q) => match format {
            OutputFormat::Json => {
                info!("{}", serde_json::to_string_pretty(&q)?);
            }
            OutputFormat::Text => {
                info!(id = %q.id, "Quota details");
                info!(namespace = %q.namespace, "  Namespace");
                info!(tenant = %q.tenant, "  Tenant");
                info!(
                    provider = %q.provider.as_deref().unwrap_or("* (generic)"),
                    "  Provider"
                );
                info!(max_actions = q.max_actions, window = %q.window, "  Max");
                info!(overage_behavior = %q.overage_behavior, "  Behavior");
                info!(enabled = q.enabled, "  Enabled");
            }
        },
        None => {
            warn!(id = %id, "Quota not found");
        }
    }
    Ok(())
}

async fn run_create(ops: &OpsClient, data: &str, format: &OutputFormat) -> anyhow::Result<()> {
    let value = parse_json_data(data)?;
    let req: CreateQuotaRequest = serde_json::from_value(value)?;
    let resp = ops.create_quota(&req).await?;
    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            info!(id = %resp.id, "Created quota");
        }
    }
    Ok(())
}

async fn run_update(
    ops: &OpsClient,
    id: &str,
    data: &str,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    let value = parse_json_data(data)?;
    let req: UpdateQuotaRequest = serde_json::from_value(value)?;
    let resp = ops.update_quota(id, &req).await?;
    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            info!(id = %resp.id, "Updated quota");
        }
    }
    Ok(())
}

async fn run_usage(ops: &OpsClient, id: &str, format: &OutputFormat) -> anyhow::Result<()> {
    let resp = ops.get_quota_usage(id).await?;
    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            info!(
                used = resp.used,
                limit = resp.limit,
                remaining = resp.remaining,
                window = %resp.window,
                resets_at = %resp.resets_at,
                overage_behavior = %resp.overage_behavior,
                "Quota usage"
            );
        }
    }
    Ok(())
}
