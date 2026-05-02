use std::collections::HashMap;

use acteon_ops::OpsClient;
use acteon_ops::acteon_client::{BusAgentFilter, RegisterBusAgent, SendToBusAgent, UpdateBusAgent};
use clap::{Args, Subcommand};
use tracing::info;

use crate::OutputFormat;
use crate::commands::bus::{parse_json_arg, parse_kv};

#[derive(Args, Debug)]
pub struct AgentsArgs {
    #[command(subcommand)]
    pub command: AgentsCommand,
}

#[derive(Subcommand, Debug)]
pub enum AgentsCommand {
    /// List agents, optionally filtered.
    List {
        #[arg(long)]
        namespace: Option<String>,
        #[arg(long)]
        tenant: Option<String>,
        #[arg(long)]
        capability: Option<String>,
        /// `online`, `idle`, `offline`.
        #[arg(long)]
        status: Option<String>,
    },
    /// Fetch a single agent record.
    Get {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        agent_id: String,
    },
    /// Register an agent. First registration in a `(namespace, tenant)`
    /// auto-creates the shared inbox topic.
    Register {
        #[arg(long)]
        agent_id: String,
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        display_name: Option<String>,
        /// Repeat for multiple capabilities: `--capability planner`.
        #[arg(long = "capability")]
        capabilities: Vec<String>,
        #[arg(long)]
        inbox_topic: Option<String>,
        #[arg(long)]
        heartbeat_ttl_ms: Option<i64>,
        #[arg(long = "label", value_parser = parse_kv)]
        labels: Vec<(String, String)>,
    },
    /// Update mutable fields on an agent. `inbox_topic` is fixed at
    /// registration.
    Update {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        agent_id: String,
        #[arg(long)]
        display_name: Option<String>,
        #[arg(long = "capability")]
        capabilities: Option<Vec<String>>,
        #[arg(long)]
        heartbeat_ttl_ms: Option<i64>,
        #[arg(long = "label", value_parser = parse_kv)]
        labels: Option<Vec<(String, String)>>,
    },
    /// Delete an agent record. The shared inbox topic is preserved.
    Delete {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        agent_id: String,
    },
    /// Record a heartbeat. Agents typically call this once per
    /// `heartbeat_ttl_ms / 3` to stay `Online`.
    Heartbeat {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        agent_id: String,
    },
    /// Send a message to the agent's inbox.
    Send {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        agent_id: String,
        /// JSON payload, or `@path/to/file.json`.
        #[arg(long)]
        payload: String,
        #[arg(long = "header", value_parser = parse_kv)]
        headers: Vec<(String, String)>,
    },
}

#[allow(clippy::too_many_lines)]
pub async fn run(ops: &OpsClient, args: &AgentsArgs, format: &OutputFormat) -> anyhow::Result<()> {
    match &args.command {
        AgentsCommand::List {
            namespace,
            tenant,
            capability,
            status,
        } => {
            let filter = BusAgentFilter {
                namespace: namespace.clone(),
                tenant: tenant.clone(),
                capability: capability.clone(),
                status: status.clone(),
            };
            let resp = ops.client().list_bus_agents(&filter).await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&resp)?),
                OutputFormat::Text => {
                    info!(count = resp.count, "Bus agents");
                    for a in &resp.agents {
                        info!(
                            agent_id = %a.agent_id,
                            namespace = %a.namespace,
                            tenant = %a.tenant,
                            status = %a.status,
                            inbox = %a.inbox_topic,
                            capabilities = ?a.capabilities,
                            "Agent"
                        );
                    }
                }
            }
        }
        AgentsCommand::Get {
            namespace,
            tenant,
            agent_id,
        } => {
            let a = ops
                .client()
                .get_bus_agent(namespace, tenant, agent_id)
                .await?;
            info!("{}", serde_json::to_string_pretty(&a)?);
        }
        AgentsCommand::Register {
            agent_id,
            namespace,
            tenant,
            display_name,
            capabilities,
            inbox_topic,
            heartbeat_ttl_ms,
            labels,
        } => {
            let req = RegisterBusAgent {
                agent_id: agent_id.clone(),
                namespace: namespace.clone(),
                tenant: tenant.clone(),
                display_name: display_name.clone(),
                capabilities: capabilities.clone(),
                inbox_topic: inbox_topic.clone(),
                heartbeat_ttl_ms: *heartbeat_ttl_ms,
                labels: labels.iter().cloned().collect::<HashMap<_, _>>(),
            };
            let a = ops.client().register_bus_agent(&req).await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&a)?),
                OutputFormat::Text => info!(
                    agent_id = %a.agent_id,
                    inbox = %a.inbox_topic,
                    "Agent registered"
                ),
            }
        }
        AgentsCommand::Update {
            namespace,
            tenant,
            agent_id,
            display_name,
            capabilities,
            heartbeat_ttl_ms,
            labels,
        } => {
            let req = UpdateBusAgent {
                display_name: display_name.clone(),
                capabilities: capabilities.clone(),
                heartbeat_ttl_ms: *heartbeat_ttl_ms,
                labels: labels
                    .as_ref()
                    .map(|kvs| kvs.iter().cloned().collect::<HashMap<_, _>>()),
            };
            let a = ops
                .client()
                .update_bus_agent(namespace, tenant, agent_id, &req)
                .await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&a)?),
                OutputFormat::Text => info!(agent_id = %a.agent_id, "Agent updated"),
            }
        }
        AgentsCommand::Delete {
            namespace,
            tenant,
            agent_id,
        } => {
            ops.client()
                .delete_bus_agent(namespace, tenant, agent_id)
                .await?;
            info!(agent_id = %agent_id, "Agent deleted");
        }
        AgentsCommand::Heartbeat {
            namespace,
            tenant,
            agent_id,
        } => {
            let h = ops
                .client()
                .heartbeat_bus_agent(namespace, tenant, agent_id)
                .await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&h)?),
                OutputFormat::Text => info!(
                    agent_id = %h.agent_id,
                    last_heartbeat_at = %h.last_heartbeat_at,
                    status = %h.status,
                    "Heartbeat recorded"
                ),
            }
        }
        AgentsCommand::Send {
            namespace,
            tenant,
            agent_id,
            payload,
            headers,
        } => {
            let payload = parse_json_arg(payload)?;
            let req = SendToBusAgent {
                payload,
                headers: headers.iter().cloned().collect(),
            };
            let receipt = ops
                .client()
                .send_to_bus_agent(namespace, tenant, agent_id, &req)
                .await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&receipt)?),
                OutputFormat::Text => info!(
                    agent_id = %receipt.agent_id,
                    inbox = %receipt.inbox_topic,
                    partition = receipt.partition,
                    offset = receipt.offset,
                    "Message sent"
                ),
            }
        }
    }
    Ok(())
}
