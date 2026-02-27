use acteon_ops::OpsClient;
use clap::{Args, Subcommand};

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
        /// Namespace.
        #[arg(long, default_value = "default")]
        namespace: String,
        /// Tenant.
        #[arg(long)]
        tenant: String,
        /// Filter by status.
        #[arg(long)]
        status: Option<String>,
    },
    /// Get chain details.
    Get {
        /// Chain ID.
        id: String,
        /// Namespace.
        #[arg(long, default_value = "default")]
        namespace: String,
        /// Tenant.
        #[arg(long)]
        tenant: String,
    },
    /// Cancel a running chain.
    Cancel {
        /// Chain ID.
        id: String,
        /// Namespace.
        #[arg(long, default_value = "default")]
        namespace: String,
        /// Tenant.
        #[arg(long)]
        tenant: String,
        /// Reason for cancellation.
        #[arg(long)]
        reason: Option<String>,
        /// Who cancelled the chain.
        #[arg(long)]
        cancelled_by: Option<String>,
    },
    /// Get the DAG for a chain instance.
    Dag {
        /// Chain ID.
        id: String,
        /// Namespace.
        #[arg(long, default_value = "default")]
        namespace: String,
        /// Tenant.
        #[arg(long)]
        tenant: String,
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
        ChainsCommand::List {
            namespace,
            tenant,
            status,
        } => run_list(ops, namespace, tenant, status.as_ref(), format).await,
        ChainsCommand::Get {
            id,
            namespace,
            tenant,
        } => run_get(ops, id, namespace, tenant, format).await,
        ChainsCommand::Cancel {
            id,
            namespace,
            tenant,
            reason,
            cancelled_by,
        } => {
            run_cancel(
                ops,
                id,
                namespace,
                tenant,
                reason.as_ref(),
                cancelled_by.as_ref(),
                format,
            )
            .await
        }
        ChainsCommand::Dag {
            id,
            namespace,
            tenant,
        } => run_dag(ops, id, namespace, tenant, format).await,
        ChainsCommand::Definitions(def_args) => run_definitions(ops, def_args, format).await,
    }
}

async fn run_list(
    ops: &OpsClient,
    namespace: &str,
    tenant: &str,
    status: Option<&String>,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    let resp = ops
        .list_chains(namespace.to_string(), tenant.to_string(), status.cloned())
        .await?;
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            println!("{} chains:", resp.chains.len());
            for chain in &resp.chains {
                println!(
                    "  {id} | {name} | {status} | step {current}/{total}",
                    id = &chain.chain_id[..8.min(chain.chain_id.len())],
                    name = chain.chain_name,
                    status = chain.status,
                    current = chain.current_step,
                    total = chain.total_steps,
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
    let resp = ops.get_chain(id, namespace, tenant).await?;
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            println!("Chain ID:     {}", resp.chain_id);
            println!("Name:         {}", resp.chain_name);
            println!("Status:       {}", resp.status);
            println!("Progress:     {}/{}", resp.current_step, resp.total_steps);
            println!("Started:      {}", resp.started_at);
            println!("Updated:      {}", resp.updated_at);
            if let Some(ref reason) = resp.cancel_reason {
                println!("Cancel:       {reason}");
            }
            for step in &resp.steps {
                let err = step.error.as_deref().unwrap_or("");
                println!(
                    "  [{status}] {name} -> {provider} {err}",
                    status = step.status,
                    name = step.name,
                    provider = step.provider,
                );
            }
        }
    }
    Ok(())
}

async fn run_cancel(
    ops: &OpsClient,
    id: &str,
    namespace: &str,
    tenant: &str,
    reason: Option<&String>,
    cancelled_by: Option<&String>,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    let resp = ops
        .cancel_chain(
            id,
            namespace,
            tenant,
            reason.map(String::as_str),
            cancelled_by.map(String::as_str),
        )
        .await?;
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            println!(
                "Chain {} cancelled (status: {}).",
                resp.chain_id, resp.status
            );
        }
    }
    Ok(())
}

async fn run_dag(
    ops: &OpsClient,
    id: &str,
    namespace: &str,
    tenant: &str,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    let resp = ops.get_chain_dag(id, namespace, tenant).await?;
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            println!(
                "DAG for chain '{}': {} nodes, {} edges",
                resp.chain_name,
                resp.nodes.len(),
                resp.edges.len()
            );
            for node in &resp.nodes {
                let provider = node.provider.as_deref().unwrap_or("-");
                let status = node.status.as_deref().unwrap_or("-");
                println!(
                    "  [{node_type}] {name} | provider: {provider} | status: {status}",
                    node_type = node.node_type,
                    name = node.name,
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
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    println!("{} definitions:", resp.definitions.len());
                    for def in &resp.definitions {
                        println!(
                            "  {name} | {steps} steps | on_failure: {on_failure}",
                            name = def.name,
                            steps = def.steps_count,
                            on_failure = def.on_failure,
                        );
                    }
                }
            }
        }
        DefinitionsCommand::Get { name } => {
            let resp = ops.get_chain_definition(name).await?;
            // Definition config is raw JSON, so always pretty-print.
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        DefinitionsCommand::Put { name, config } => {
            let config_value = parse_json_data(config)?;
            let resp = ops.put_chain_definition(name, &config_value).await?;
            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    println!("Chain definition '{name}' saved.");
                }
            }
        }
        DefinitionsCommand::Delete { name } => {
            ops.delete_chain_definition(name).await?;
            println!("Chain definition '{name}' deleted.");
        }
        DefinitionsCommand::Dag { name } => {
            let resp = ops.get_chain_definition_dag(name).await?;
            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    println!(
                        "DAG for definition '{}': {} nodes, {} edges",
                        resp.chain_name,
                        resp.nodes.len(),
                        resp.edges.len()
                    );
                    for node in &resp.nodes {
                        let provider = node.provider.as_deref().unwrap_or("-");
                        println!(
                            "  [{node_type}] {name} | provider: {provider}",
                            node_type = node.node_type,
                            name = node.name,
                        );
                    }
                }
            }
        }
    }
    Ok(())
}
