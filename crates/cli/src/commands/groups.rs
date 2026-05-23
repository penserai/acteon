use acteon_ops::OpsClient;
use clap::{Args, Subcommand};
use tracing::{info, warn};

use crate::OutputFormat;

#[derive(Args, Debug)]
pub struct GroupsArgs {
    #[command(subcommand)]
    pub command: GroupsCommand,
}

#[derive(Subcommand, Debug)]
pub enum GroupsCommand {
    /// List event groups.
    List,
    /// Get an event group by key.
    Get {
        /// Group key.
        key: String,
    },
    /// Flush an event group (trigger immediate notification).
    Flush {
        /// Group key.
        key: String,
    },
}

pub async fn run(ops: &OpsClient, args: &GroupsArgs, format: &OutputFormat) -> anyhow::Result<()> {
    match &args.command {
        GroupsCommand::List => {
            let resp = ops.list_groups().await?;
            match format {
                OutputFormat::Json => {
                    info!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    info!(count = resp.total, "Groups");
                    for g in &resp.groups {
                        let notify = g.notify_at.as_deref().unwrap_or("-");
                        info!(
                            key = %g.group_key,
                            state = %g.state,
                            event_count = g.event_count,
                            notify_at = %notify,
                            "Group"
                        );
                    }
                }
            }
        }
        GroupsCommand::Get { key } => {
            let resp = ops.get_group(key).await?;
            match resp {
                Some(detail) => match format {
                    OutputFormat::Json => {
                        info!("{}", serde_json::to_string_pretty(&detail)?);
                    }
                    OutputFormat::Text => {
                        info!(group_id = %detail.group.group_id, "Group details");
                        info!(key = %detail.group.group_key, "  Key");
                        info!(state = %detail.group.state, "  State");
                        info!(event_count = detail.group.event_count, "  Events");
                        info!(created_at = %detail.group.created_at, "  Created");
                        if !detail.events.is_empty() {
                            info!("Event fingerprints:");
                            for fp in &detail.events {
                                info!(fingerprint = %fp, "  - Event");
                            }
                        }
                    }
                },
                None => {
                    warn!(key = %key, "Group not found");
                }
            }
        }
        GroupsCommand::Flush { key } => {
            let resp = ops.flush_group(key).await?;
            match format {
                OutputFormat::Json => {
                    info!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    info!(
                        group_id = %resp.group_id,
                        event_count = resp.event_count,
                        notified = resp.notified,
                        "Flushed group"
                    );
                }
            }
        }
    }
    Ok(())
}
