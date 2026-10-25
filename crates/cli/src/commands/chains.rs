use acteon_ops::OpsClient;
use clap::{Args, Subcommand};
use tracing::info;

use crate::OutputFormat;

#[derive(Args, Debug)]
pub struct ChainsArgs {
    #[command(subcommand)]
    pub command: ChainsCommand,
}

#[derive(Subcommand, Debug)]
pub enum ChainsCommand {
    /// List action chains.
    List {
        /// Filter by status.
        #[arg(long)]
        status: Option<String>,
    },
    /// Get chain details.
    Get {
        /// Chain ID.
        id: String,
    },
    /// Cancel a running chain.
    Cancel {
        /// Chain ID.
        id: String,
        /// Reason for cancellation.
        #[arg(long)]
        reason: Option<String>,
        /// Who cancelled the chain.
        #[arg(long)]
        cancelled_by: Option<String>,
    },
    /// Get per-step execution history with retry attempts.
    History {
        /// Chain ID.
        id: String,
    },
    /// Get the DAG for a chain instance.
    Dag {
        /// Chain ID.
        id: String,
    },
    /// Manage chain definitions.
    Definitions(DefinitionsArgs),
}

#[derive(Args, Debug)]
pub struct DefinitionsArgs {
    #[command(subcommand)]
    pub command: DefinitionsCommand,
}

#[derive(Subcommand, Debug)]
pub enum DefinitionsCommand {
    /// List all chain definitions.
    List,
    /// Get a chain definition by name.
    Get {
        /// Chain definition name.
        name: String,
    },
    /// Create or update a chain definition.
    Put {
        /// Chain definition name.
        name: String,
        /// JSON config (string or @file path).
        #[arg(long)]
        config: String,
    },
    /// Delete a chain definition.
    Delete {
        /// Chain definition name.
        name: String,
    },
    /// Get the DAG for a chain definition.
    Dag {
        /// Chain definition name.
        name: String,
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

pub async fn run(ops: &OpsClient, args: &ChainsArgs, format: &OutputFormat) -> anyhow::Result<()> {
    match &args.command {
        ChainsCommand::List { status } => run_list(ops, status.as_ref(), format).await,
        ChainsCommand::Get { id } => run_get(ops, id, format).await,
        ChainsCommand::Cancel {
            id,

            reason,
            cancelled_by,
        } => run_cancel(ops, id, reason.as_ref(), cancelled_by.as_ref(), format).await,
        ChainsCommand::History { id } => run_history(ops, id, format).await,
        ChainsCommand::Dag { id } => run_dag(ops, id, format).await,
        ChainsCommand::Definitions(def_args) => run_definitions(ops, def_args, format).await,
    }
}

async fn run_list(
    ops: &OpsClient,
    status: Option<&String>,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    let resp = ops.list_chains(status.cloned()).await?;
    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            info!(count = resp.chains.len(), "Chains");
            for chain in &resp.chains {
                info!(
                    id = %&chain.chain_id[..8.min(chain.chain_id.len())],
                    name = %chain.chain_name,
                    status = %chain.status,
                    current_step = chain.current_step,
                    total_steps = chain.total_steps,
                    "Chain"
                );
            }
        }
    }
    Ok(())
}

async fn run_get(ops: &OpsClient, id: &str, format: &OutputFormat) -> anyhow::Result<()> {
    let resp = ops.get_chain(id).await?;
    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            info!(chain_id = %resp.chain_id, "Chain details");
            info!(name = %resp.chain_name, "  Name");
            info!(status = %resp.status, "  Status");
            info!(
                current = resp.current_step,
                total = resp.total_steps,
                "  Progress"
            );
            info!(started_at = %resp.started_at, "  Started");
            info!(updated_at = %resp.updated_at, "  Updated");
            if let Some(ref reason) = resp.cancel_reason {
                info!(reason = %reason, "  Cancel reason");
            }
            for step in &resp.steps {
                let err = step.error.as_deref().unwrap_or("");
                info!(
                    status = %step.status,
                    name = %step.name,
                    provider = %step.provider,
                    error = %err,
                    "  Step"
                );
            }
        }
    }
    Ok(())
}

async fn run_cancel(
    ops: &OpsClient,
    id: &str,
    reason: Option<&String>,
    cancelled_by: Option<&String>,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    let resp = ops
        .cancel_chain(
            id,
            reason.map(String::as_str),
            cancelled_by.map(String::as_str),
        )
        .await?;
    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            info!(
                chain_id = %resp.chain_id,
                status = %resp.status,
                "Chain cancelled"
            );
        }
    }
    Ok(())
}

async fn run_history(ops: &OpsClient, id: &str, format: &OutputFormat) -> anyhow::Result<()> {
    let resp = ops.get_chain_history(id).await?;
    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            info!(
                chain_id = %resp.chain_id,
                chain_name = %resp.chain_name,
                status = %resp.status,
                "Chain execution history"
            );
            for step in &resp.steps {
                let max = step
                    .max_retries
                    .map_or_else(|| "-".to_string(), |m| m.to_string());
                info!(
                    step_index = step.step_index,
                    name = %step.name,
                    attempt = step.current_attempt,
                    max_retries = %max,
                    total_attempts = step.attempts.len(),
                    "  Step"
                );
                for attempt in &step.attempts {
                    let err = attempt.error.as_deref().unwrap_or("");
                    info!(
                        attempt = attempt.attempt,
                        success = attempt.success,
                        duration_ms = attempt.duration_ms,
                        error = %err,
                        "    Attempt"
                    );
                }
            }
        }
    }
    Ok(())
}

async fn run_dag(ops: &OpsClient, id: &str, format: &OutputFormat) -> anyhow::Result<()> {
    let resp = ops.get_chain_dag(id).await?;
    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            info!(
                chain_name = %resp.chain_name,
                nodes = resp.nodes.len(),
                edges = resp.edges.len(),
                "Chain DAG"
            );
            for node in &resp.nodes {
                let provider = node.provider.as_deref().unwrap_or("-");
                let status = node.status.as_deref().unwrap_or("-");
                info!(
                    node_type = %node.node_type,
                    name = %node.name,
                    provider = %provider,
                    status = %status,
                    "  DAG node"
                );
            }
        }
    }
    Ok(())
}

async fn run_definitions(
    ops: &OpsClient,
    args: &DefinitionsArgs,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    match &args.command {
        DefinitionsCommand::List => {
            let resp = ops.list_chain_definitions().await?;
            match format {
                OutputFormat::Json => {
                    info!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    info!(count = resp.definitions.len(), "Chain definitions");
                    for def in &resp.definitions {
                        info!(
                            name = %def.name,
                            steps = def.steps_count,
                            on_failure = %def.on_failure,
                            "Definition"
                        );
                    }
                }
            }
        }
        DefinitionsCommand::Get { name } => {
            let resp = ops.get_chain_definition(name).await?;
            // Definition config is raw JSON, so always pretty-print.
            info!("{}", serde_json::to_string_pretty(&resp)?);
        }
        DefinitionsCommand::Put { name, config } => {
            let config_value = parse_json_data(config)?;
            let resp = ops.put_chain_definition(name, &config_value).await?;
            match format {
                OutputFormat::Json => {
                    info!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    info!(name = %name, "Chain definition saved");
                }
            }
        }
        DefinitionsCommand::Delete { name } => {
            ops.delete_chain_definition(name).await?;
            info!(name = %name, "Chain definition deleted");
        }
        DefinitionsCommand::Dag { name } => {
            let resp = ops.get_chain_definition_dag(name).await?;
            match format {
                OutputFormat::Json => {
                    info!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    info!(
                        chain_name = %resp.chain_name,
                        nodes = resp.nodes.len(),
                        edges = resp.edges.len(),
                        "Definition DAG"
                    );
                    for node in &resp.nodes {
                        let provider = node.provider.as_deref().unwrap_or("-");
                        info!(
                            node_type = %node.node_type,
                            name = %node.name,
                            provider = %provider,
                            "  DAG node"
                        );
                    }
                }
            }
        }
    }
    Ok(())
}
