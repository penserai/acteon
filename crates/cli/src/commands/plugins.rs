use acteon_ops::OpsClient;
use clap::{Args, Subcommand};

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
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    println!("{} plugins:", resp.count);
                    for p in &resp.plugins {
                        let enabled = if p.enabled { "ON " } else { "OFF" };
                        let desc = p.description.as_deref().unwrap_or("");
                        println!(
                            "  [{enabled}] {name} | {status} | invocations: {count} {desc}",
                            name = p.name,
                            status = p.status,
                            count = p.invocation_count,
                        );
                    }
                }
            }
        }
        PluginsCommand::Delete { name } => {
            ops.delete_plugin(name).await?;
            println!("Plugin '{name}' deleted.");
        }
    }
    Ok(())
}
