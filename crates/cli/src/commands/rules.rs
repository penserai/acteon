use std::fmt::Write;

use acteon_ops::OpsClient;
use acteon_ops::test_rules::{self, TestRunSummary};
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
    /// Run a test suite of rule fixtures against the gateway.
    Test {
        /// Path to YAML fixtures file.
        fixtures: String,
        /// Only run tests whose name contains this substring.
        #[arg(long)]
        filter: Option<String>,
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
        RulesCommand::Test { fixtures, filter } => {
            let yaml = std::fs::read_to_string(fixtures)?;
            let fixture_file = test_rules::parse_fixture(&yaml)?;

            let summary = test_rules::run_test_suite(ops, &fixture_file, filter.as_deref()).await?;

            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&summary)?);
                }
                OutputFormat::Text => {
                    print_test_summary(&summary);
                }
            }

            if summary.failed > 0 {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

fn print_test_summary(summary: &TestRunSummary) {
    println!();
    for result in &summary.results {
        if result.passed {
            println!("  PASS  {}", result.name);
        } else if let Some(ref err) = result.error {
            println!("  FAIL  {} (error: {err})", result.name);
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
            println!("  FAIL  {} ({detail})", result.name);
        }
    }

    println!();
    println!(
        "test result: {} passed, {} failed ({} total) in {}ms",
        summary.passed, summary.failed, summary.total, summary.duration_ms,
    );
}
