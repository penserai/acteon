use acteon_ops::OpsClient;
use clap::{Args, Subcommand};
use tracing::info;

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
                    info!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    info!(enabled = resp.enabled, entries = resp.count, "DLQ stats");
                }
            }
        }
        DlqCommand::Drain => {
            let resp = ops.dlq_drain().await?;
            match format {
                OutputFormat::Json => {
                    info!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    info!(count = resp.count, "Drained DLQ entries");
                    for entry in &resp.entries {
                        info!(
                            id = %&entry.action_id[..8.min(entry.action_id.len())],
                            provider = %entry.provider,
                            action_type = %entry.action_type,
                            error = %entry.error,
                            attempts = entry.attempts,
                            "DLQ entry"
                        );
                    }
                }
            }
        }
    }
    Ok(())
}
