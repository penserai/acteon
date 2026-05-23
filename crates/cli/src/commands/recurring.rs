use acteon_ops::OpsClient;
use acteon_ops::acteon_client::{CreateRecurringAction, RecurringFilter, UpdateRecurringAction};
use clap::{Args, Subcommand};
use tracing::{info, warn};

use crate::OutputFormat;

#[derive(Args, Debug)]
pub struct RecurringArgs {
    #[command(subcommand)]
    pub command: RecurringCommand,
}

#[derive(Subcommand, Debug)]
pub enum RecurringCommand {
    /// List recurring actions.
    List {
        /// Namespace.
        #[arg(long, default_value = "default")]
        namespace: String,
        /// Tenant.
        #[arg(long)]
        tenant: String,
        /// Filter by status (active/paused).
        #[arg(long)]
        status: Option<String>,
    },
    /// Get a recurring action by ID.
    Get {
        /// Recurring action ID.
        id: String,
        /// Namespace.
        #[arg(long, default_value = "default")]
        namespace: String,
        /// Tenant.
        #[arg(long)]
        tenant: String,
    },
    /// Create a recurring action.
    Create {
        /// JSON data (string or @file path).
        #[arg(long)]
        data: String,
    },
    /// Update a recurring action.
    Update {
        /// Recurring action ID.
        id: String,
        /// JSON data (string or @file path).
        #[arg(long)]
        data: String,
    },
    /// Delete a recurring action.
    Delete {
        /// Recurring action ID.
        id: String,
        /// Namespace.
        #[arg(long, default_value = "default")]
        namespace: String,
        /// Tenant.
        #[arg(long)]
        tenant: String,
    },
    /// Pause a recurring action.
    Pause {
        /// Recurring action ID.
        id: String,
        /// Namespace.
        #[arg(long, default_value = "default")]
        namespace: String,
        /// Tenant.
        #[arg(long)]
        tenant: String,
    },
    /// Resume a recurring action.
    Resume {
        /// Recurring action ID.
        id: String,
        /// Namespace.
        #[arg(long, default_value = "default")]
        namespace: String,
        /// Tenant.
        #[arg(long)]
        tenant: String,
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
    args: &RecurringArgs,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    match &args.command {
        RecurringCommand::List {
            namespace,
            tenant,
            status,
        } => run_list(ops, namespace, tenant, status.as_ref(), format).await,
        RecurringCommand::Get {
            id,
            namespace,
            tenant,
        } => run_get(ops, id, namespace, tenant, format).await,
        RecurringCommand::Create { data } => run_create(ops, data, format).await,
        RecurringCommand::Update { id, data } => run_update(ops, id, data, format).await,
        RecurringCommand::Delete {
            id,
            namespace,
            tenant,
        } => {
            ops.delete_recurring(id, namespace, tenant).await?;
            info!(id = %id, "Recurring action deleted");
            Ok(())
        }
        RecurringCommand::Pause {
            id,
            namespace,
            tenant,
        } => run_pause(ops, id, namespace, tenant, format).await,
        RecurringCommand::Resume {
            id,
            namespace,
            tenant,
        } => run_resume(ops, id, namespace, tenant, format).await,
    }
}

async fn run_list(
    ops: &OpsClient,
    namespace: &str,
    tenant: &str,
    status: Option<&String>,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    let filter = RecurringFilter {
        namespace: namespace.to_string(),
        tenant: tenant.to_string(),
        status: status.cloned(),
        ..Default::default()
    };
    let resp = ops.list_recurring(&filter).await?;
    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            info!(count = resp.count, "Recurring actions");
            for r in &resp.recurring_actions {
                let enabled = if r.enabled { "ON " } else { "OFF" };
                info!(
                    enabled = %enabled,
                    id = %&r.id[..8.min(r.id.len())],
                    cron = %r.cron_expr,
                    provider = %r.provider,
                    action_type = %r.action_type,
                    "Recurring action"
                );
            }
        }
    }
    Ok(())
}

async fn run_get(
    ops: &OpsClient,
    id: &str,
    namespace: &str,
    tenant: &str,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    let resp = ops.get_recurring(id, namespace, tenant).await?;
    match resp {
        Some(detail) => match format {
            OutputFormat::Json => {
                info!("{}", serde_json::to_string_pretty(&detail)?);
            }
            OutputFormat::Text => {
                info!(id = %detail.id, "Recurring action details");
                info!(provider = %detail.provider, "  Provider");
                info!(action_type = %detail.action_type, "  Action Type");
                info!(cron = %detail.cron_expr, "  Cron");
                info!(timezone = %detail.timezone, "  Timezone");
                info!(enabled = detail.enabled, "  Enabled");
                info!(execution_count = detail.execution_count, "  Executions");
                if let Some(ref next) = detail.next_execution_at {
                    info!(next_run = %next, "  Next Run");
                }
            }
        },
        None => {
            warn!(id = %id, "Recurring action not found");
        }
    }
    Ok(())
}

async fn run_create(ops: &OpsClient, data: &str, format: &OutputFormat) -> anyhow::Result<()> {
    let value = parse_json_data(data)?;
    let req: CreateRecurringAction = serde_json::from_value(value)?;
    let resp = ops.create_recurring(&req).await?;
    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            info!(
                id = %resp.id,
                status = %resp.status,
                "Created recurring action"
            );
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
    let req: UpdateRecurringAction = serde_json::from_value(value)?;
    let resp = ops.update_recurring(id, &req).await?;
    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            info!(id = %resp.id, "Updated recurring action");
        }
    }
    Ok(())
}

async fn run_pause(
    ops: &OpsClient,
    id: &str,
    namespace: &str,
    tenant: &str,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    let resp = ops.pause_recurring(id, namespace, tenant).await?;
    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            info!(id = %resp.id, "Recurring action paused");
        }
    }
    Ok(())
}

async fn run_resume(
    ops: &OpsClient,
    id: &str,
    namespace: &str,
    tenant: &str,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    let resp = ops.resume_recurring(id, namespace, tenant).await?;
    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            info!(id = %resp.id, "Recurring action resumed");
        }
    }
    Ok(())
}
