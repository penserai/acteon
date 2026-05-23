//! Import commands for migrating configs from other systems.

pub mod alertmanager;

use clap::{Args, Subcommand};

/// Import configuration from external systems.
#[derive(Args, Debug)]
pub struct ImportArgs {
    #[command(subcommand)]
    pub command: ImportCommand,
}

#[derive(Subcommand, Debug)]
pub enum ImportCommand {
    /// Import routing config from a Prometheus Alertmanager alertmanager.yml file.
    Alertmanager(alertmanager::AlertmanagerImportArgs),
}

pub fn run(args: &ImportArgs) -> anyhow::Result<()> {
    match &args.command {
        ImportCommand::Alertmanager(a) => alertmanager::run(a),
    }
}
