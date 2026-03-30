use std::fmt::Write;

use acteon_ops::OpsClient;
use acteon_ops::test_rules::{self, TestRunSummary};
use clap::{Args, Subcommand};
use tracing::{info, warn};

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
    /// Run a test suite of rule fixtures against the gateway.
    Test {
        /// Path to YAML fixtures file.
        fixtures: String,
        /// Only run tests whose name contains this substring.
        #[arg(long)]
        filter: Option<String>,
    },
    /// Reload rules from the YAML directory.
    Reload,
}

pub async fn run(ops: &OpsClient, args: &RulesArgs, format: &OutputFormat) -> anyhow::Result<()> {
    match &args.command {
        RulesCommand::List => {
            let rules = ops.client().list_rules().await?;
            match format {
                OutputFormat::Json => {
                    info!("{}", serde_json::to_string_pretty(&rules)?);
                }
                OutputFormat::Text => {
                    info!(count = rules.len(), "Rules loaded");
                    for rule in &rules {
                        let status = if rule.enabled { "ON " } else { "OFF" };
                        let desc = rule.description.as_deref().unwrap_or("");
                        info!(
                            status = %status,
                            name = %rule.name,
                            priority = rule.priority,
                            description = %desc,
                            "Rule"
                        );
                    }
                }
            }
        }
        RulesCommand::Enable { name } => {
            ops.client().set_rule_enabled(name, true).await?;
            info!(name = %name, "Rule enabled");
        }
        RulesCommand::Disable { name } => {
            ops.client().set_rule_enabled(name, false).await?;
            info!(name = %name, "Rule disabled");
        }
        RulesCommand::Test { fixtures, filter } => {
            let yaml = std::fs::read_to_string(fixtures)?;
            let fixture_file = test_rules::parse_fixture(&yaml)?;

            let summary = test_rules::run_test_suite(ops, &fixture_file, filter.as_deref()).await?;

            match format {
                OutputFormat::Json => {
                    info!("{}", serde_json::to_string_pretty(&summary)?);
                }
                OutputFormat::Text => {
                    print_test_summary(&summary);
                }
            }

            if summary.failed > 0 {
                std::process::exit(1);
            }
        }
        RulesCommand::Reload => {
            let result = ops.reload_rules().await?;
            match format {
                OutputFormat::Json => {
                    info!("{}", serde_json::to_string_pretty(&result)?);
                }
                OutputFormat::Text => {
                    info!(loaded = result.loaded, "Reloaded rules");
                    if !result.errors.is_empty() {
                        warn!("Rule reload errors:");
                        for err in &result.errors {
                            warn!(error = %err, "  Rule error");
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn print_test_summary(summary: &TestRunSummary) {
    info!("");
    for result in &summary.results {
        if result.passed {
            info!(name = %result.name, "  PASS");
        } else if let Some(ref err) = result.error {
            warn!(name = %result.name, error = %err, "  FAIL");
        } else {
            let mut detail = format!(
                "expected verdict '{}', got '{}'",
                result.expected_verdict, result.actual_verdict
            );
            if let Some(ref expected_rule) = result.expected_rule {
                let actual = result.actual_rule.as_deref().unwrap_or("<none>");
                if expected_rule != actual {
                    let _ = write!(detail, "; expected rule '{expected_rule}', got '{actual}'");
                }
            }
            warn!(name = %result.name, detail = %detail, "  FAIL");
        }
    }

    info!("");
    info!(
        passed = summary.passed,
        failed = summary.failed,
        total = summary.total,
        duration_ms = summary.duration_ms,
        "Test result"
    );
}
