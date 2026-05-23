use acteon_ops::OpsClient;
use clap::{Args, Subcommand};
use tracing::info;

use crate::OutputFormat;

#[derive(Args, Debug)]
pub struct ProvidersArgs {
    #[command(subcommand)]
    pub command: ProvidersCommand,
}

#[derive(Subcommand, Debug)]
pub enum ProvidersCommand {
    /// Show provider health status and metrics.
    Health,
}

pub async fn run(
    ops: &OpsClient,
    args: &ProvidersArgs,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    match &args.command {
        ProvidersCommand::Health => {
            let resp = ops.list_provider_health().await?;
            match format {
                OutputFormat::Json => {
                    info!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    info!(count = resp.providers.len(), "Providers");
                    for p in &resp.providers {
                        let health = if p.healthy { "OK " } else { "ERR" };
                        let cb = p.circuit_breaker_state.as_deref().unwrap_or("-");
                        info!(
                            health = %health,
                            provider = %p.provider,
                            total_requests = p.total_requests,
                            success_rate = p.success_rate,
                            p50_latency_ms = p.p50_latency_ms,
                            p99_latency_ms = p.p99_latency_ms,
                            circuit_breaker = %cb,
                            "Provider"
                        );
                    }
                }
            }
        }
    }
    Ok(())
}
