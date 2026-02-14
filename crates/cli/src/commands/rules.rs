use acteon_ops::OpsClient;
use clap::{Args, Subcommand};

use crate::OutputFormat;

#[derive(Args, Debug)]
pub struct RulesArgs {
    #[command(subcommand)]
    pub command: RulesCommand,
}

#[derive(Subcommand, Debug)]
pub enum RulesCommand {
    /// List all loaded rules.
    List,
    /// Enable a rule by name.
    Enable {
        /// Rule name.
        name: String,
    },
    /// Disable a rule by name.
    Disable {
        /// Rule name.
        name: String,
    },
}

pub async fn run(ops: &OpsClient, args: &RulesArgs, format: &OutputFormat) -> anyhow::Result<()> {
    match &args.command {
        RulesCommand::List => {
            let rules = ops.client().list_rules().await?;
            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&rules)?);
                }
                OutputFormat::Text => {
                    println!("{} rules loaded:", rules.len());
                    for rule in &rules {
                        let status = if rule.enabled { "ON " } else { "OFF" };
                        let desc = rule.description.as_deref().unwrap_or("");
                        println!(
                            "  [{status}] {name} (priority {priority}) {desc}",
                            name = rule.name,
                            priority = rule.priority,
                        );
                    }
                }
            }
        }
        RulesCommand::Enable { name } => {
            ops.client().set_rule_enabled(name, true).await?;
            println!("Rule '{name}' enabled.");
        }
        RulesCommand::Disable { name } => {
            ops.client().set_rule_enabled(name, false).await?;
            println!("Rule '{name}' disabled.");
        }
    }
    Ok(())
}
