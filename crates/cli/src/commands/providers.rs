use acteon_ops::OpsClient;
use clap::{Args, Subcommand};

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
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    println!("{} providers:", resp.providers.len());
                    for p in &resp.providers {
                        let health = if p.healthy { "OK " } else { "ERR" };
                        let cb = p.circuit_breaker_state.as_deref().unwrap_or("-");
                        println!(
                            "  [{health}] {provider} | reqs: {total} | success: {rate:.1}% | p50: {p50:.1}ms | p99: {p99:.1}ms | cb: {cb}",
                            provider = p.provider,
                            total = p.total_requests,
                            rate = p.success_rate,
                            p50 = p.p50_latency_ms,
                            p99 = p.p99_latency_ms,
                        );
                    }
                }
            }
        }
    }
    Ok(())
}
