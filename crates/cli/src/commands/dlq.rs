use acteon_ops::OpsClient;
use clap::{Args, Subcommand};

use crate::OutputFormat;

#[derive(Args, Debug)]
pub struct DlqArgs {
    #[command(subcommand)]
    pub command: DlqCommand,
}

#[derive(Subcommand, Debug)]
pub enum DlqCommand {
    /// Show dead letter queue statistics.
    Stats,
    /// Drain all entries from the dead letter queue.
    Drain,
}

pub async fn run(ops: &OpsClient, args: &DlqArgs, format: &OutputFormat) -> anyhow::Result<()> {
    match &args.command {
        DlqCommand::Stats => {
            let resp = ops.dlq_stats().await?;
            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    println!("DLQ enabled: {}", resp.enabled);
                    println!("Entries:     {}", resp.count);
                }
            }
        }
        DlqCommand::Drain => {
            let resp = ops.dlq_drain().await?;
            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    println!("Drained {} entries:", resp.count);
                    for entry in &resp.entries {
                        println!(
                            "  {id} | {provider}/{action_type} | {err} (attempts: {attempts})",
                            id = &entry.action_id[..8.min(entry.action_id.len())],
                            provider = entry.provider,
                            action_type = entry.action_type,
                            err = entry.error,
                            attempts = entry.attempts,
                        );
                    }
                }
            }
        }
    }
    Ok(())
}
