use acteon_ops::OpsClient;
use clap::{Args, Subcommand};
use tracing::info;

use crate::OutputFormat;

#[derive(Args, Debug)]
pub struct PluginsArgs {
    #[command(subcommand)]
    pub command: PluginsCommand,
}

#[derive(Subcommand, Debug)]
pub enum PluginsCommand {
    /// List registered WASM plugins.
    List,
    /// Delete a WASM plugin by name.
    Delete {
        /// Plugin name.
        name: String,
    },
}

pub async fn run(ops: &OpsClient, args: &PluginsArgs, format: &OutputFormat) -> anyhow::Result<()> {
    match &args.command {
        PluginsCommand::List => {
            let resp = ops.list_plugins().await?;
            match format {
                OutputFormat::Json => {
                    info!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    info!(count = resp.count, "Plugins");
                    for p in &resp.plugins {
                        let enabled = if p.enabled { "ON " } else { "OFF" };
                        let desc = p.description.as_deref().unwrap_or("");
                        info!(
                            enabled = %enabled,
                            name = %p.name,
                            status = %p.status,
                            invocation_count = p.invocation_count,
                            description = %desc,
                            "Plugin"
                        );
                    }
                }
            }
        }
        PluginsCommand::Delete { name } => {
            ops.delete_plugin(name).await?;
            info!(name = %name, "Plugin deleted");
        }
    }
    Ok(())
}
