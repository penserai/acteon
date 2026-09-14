use std::path::PathBuf;

use acteon_ops::OpsClient;
use clap::{Args, Subcommand};
use tracing::info;

use crate::OutputFormat;

#[derive(Args, Debug)]
pub struct MetricsArgs {
    #[command(subcommand)]
    pub command: MetricsCommand,
}

#[derive(Subcommand, Debug)]
pub enum MetricsCommand {
    /// Export Prometheus alerting rules generated from the server config.
    ExportAlerts {
        /// Write the generated YAML to a file instead of stdout.
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
}

pub async fn run(ops: &OpsClient, args: &MetricsArgs, format: &OutputFormat) -> anyhow::Result<()> {
    match &args.command {
        MetricsCommand::ExportAlerts { output } => {
            let rules = ops.prometheus_alert_rules().await?;
            if let Some(path) = output {
                std::fs::write(path, &rules)?;
                info!(path = %path.display(), bytes = rules.len(), "Prometheus alerting rules exported");
            } else {
                match format {
                    OutputFormat::Text => print!("{rules}"),
                    OutputFormat::Json => println!("{}", serde_json::to_string(&rules)?),
                }
            }
        }
    }
    Ok(())
}
