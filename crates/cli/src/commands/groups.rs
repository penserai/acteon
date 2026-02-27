use acteon_ops::OpsClient;
use clap::{Args, Subcommand};

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
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    println!("{} groups:", resp.total);
                    for g in &resp.groups {
                        let notify = g.notify_at.as_deref().unwrap_or("-");
                        println!(
                            "  {key} | {state} | {count} events | notify: {notify}",
                            key = g.group_key,
                            state = g.state,
                            count = g.event_count,
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
                        println!("{}", serde_json::to_string_pretty(&detail)?);
                    }
                    OutputFormat::Text => {
                        println!("Group ID:  {}", detail.group.group_id);
                        println!("Key:       {}", detail.group.group_key);
                        println!("State:     {}", detail.group.state);
                        println!("Events:    {}", detail.group.event_count);
                        println!("Created:   {}", detail.group.created_at);
                        if !detail.events.is_empty() {
                            println!("Event fingerprints:");
                            for fp in &detail.events {
                                println!("  - {fp}");
                            }
                        }
                    }
                },
                None => {
                    println!("Group not found: {key}");
                }
            }
        }
        GroupsCommand::Flush { key } => {
            let resp = ops.flush_group(key).await?;
            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    println!(
                        "Flushed group '{}': {} events, notified: {}",
                        resp.group_id, resp.event_count, resp.notified
                    );
                }
            }
        }
    }
    Ok(())
}
