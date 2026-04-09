use std::fmt::Write;

use acteon_ops::OpsClient;
use acteon_ops::acteon_client::{CoverageEntry, CoverageQuery, CoverageReport};
use acteon_ops::test_rules::{self, TestRunSummary};
use chrono::{DateTime, Utc};
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
    /// Analyze rule coverage from the audit trail.
    ///
    /// The server aggregates audit records by `(namespace, tenant, provider,
    /// action_type, matched_rule)` within the requested time window and
    /// cross-references the result with the currently-loaded rule set.
    /// No raw audit records are transferred over the wire.
    Coverage {
        /// Filter by namespace.
        #[arg(long)]
        namespace: Option<String>,
        /// Filter by tenant.
        #[arg(long)]
        tenant: Option<String>,
        /// Scan the last N hours of audit history (default: 24).
        ///
        /// Mutually exclusive with `--from`/`--to`.
        #[arg(long, default_value = "24", conflicts_with_all = ["from", "to"])]
        since_hours: u64,
        /// Start of the time range (RFC 3339).
        #[arg(long)]
        from: Option<DateTime<Utc>>,
        /// End of the time range (RFC 3339). Defaults to now.
        #[arg(long)]
        to: Option<DateTime<Utc>>,
        /// Only display UNCOVERED entries.
        #[arg(long)]
        only_uncovered: bool,
        /// Hide entries with fewer than N uncovered actions.
        #[arg(long, default_value = "0")]
        min_uncovered: u64,
    },
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
        RulesCommand::Coverage {
            namespace,
            tenant,
            since_hours,
            from,
            to,
            only_uncovered,
            min_uncovered,
        } => {
            // Build time range: explicit from/to takes precedence; otherwise use --since-hours.
            let (effective_from, effective_to) = if from.is_some() || to.is_some() {
                (*from, *to)
            } else {
                let now = Utc::now();
                let since = now - chrono::Duration::hours((*since_hours).cast_signed());
                (Some(since), Some(now))
            };

            let query = CoverageQuery {
                namespace: namespace.clone(),
                tenant: tenant.clone(),
                from: effective_from,
                to: effective_to,
            };

            let report = ops.rules_coverage(&query).await?;

            match format {
                OutputFormat::Json => {
                    info!("{}", serde_json::to_string_pretty(&report)?);
                }
                OutputFormat::Text => {
                    print_coverage_report(&report, *only_uncovered, *min_uncovered);
                }
            }
        }
    }
    Ok(())
}

// =========================================================================
// Coverage display
// =========================================================================

fn print_coverage_report(report: &CoverageReport, only_uncovered: bool, min_uncovered: u64) {
    info!(
        scanned_from = %report.scanned_from.to_rfc3339(),
        scanned_to = %report.scanned_to.to_rfc3339(),
        total_actions = report.total_actions,
        rules_loaded = report.rules_loaded,
        "Coverage analysis (scan window)"
    );
    info!("");

    info!(
        combinations = report.unique_combinations,
        fully_covered = report.fully_covered,
        partially_covered = report.partially_covered,
        uncovered = report.uncovered,
        "Coverage summary"
    );
    info!("");

    if report.entries.is_empty() {
        info!("No audit records in the scanned window.");
        return;
    }

    let filtered: Vec<&CoverageEntry> = report
        .entries
        .iter()
        .filter(|e| {
            if only_uncovered && e.covered > 0 {
                return false;
            }
            e.uncovered >= min_uncovered
        })
        .collect();

    if filtered.is_empty() {
        info!("No entries match the current filters.");
    } else {
        print_coverage_table(&filtered);
    }

    print_unmatched_rules(report);
}

fn print_coverage_table(entries: &[&CoverageEntry]) {
    let ns_w = entries
        .iter()
        .map(|e| e.key.namespace.len())
        .max()
        .unwrap_or(9)
        .max(9);
    let tenant_w = entries
        .iter()
        .map(|e| e.key.tenant.len())
        .max()
        .unwrap_or(6)
        .max(6);
    let prov_w = entries
        .iter()
        .map(|e| e.key.provider.len())
        .max()
        .unwrap_or(8)
        .max(8);
    let type_w = entries
        .iter()
        .map(|e| e.key.action_type.len())
        .max()
        .unwrap_or(11)
        .max(11);

    let mut header = String::new();
    let _ = write!(
        header,
        "{:<ns_w$}  {:<tenant_w$}  {:<prov_w$}  {:<type_w$}  {:>5}  {:>5}  {:>5}  STATUS     RULES",
        "NAMESPACE", "TENANT", "PROVIDER", "ACTION_TYPE", "TOTAL", "COVER", "MISS",
    );
    info!("{header}");
    info!("{}", "-".repeat(header.len()));

    for entry in entries {
        let status = if entry.covered == 0 {
            "UNCOVERED"
        } else if entry.uncovered > 0 {
            "PARTIAL"
        } else {
            "COVERED"
        };

        let rules_str = if entry.matched_rules.is_empty() {
            "-".to_string()
        } else {
            entry.matched_rules.join(", ")
        };

        let line = format!(
            "{:<ns_w$}  {:<tenant_w$}  {:<prov_w$}  {:<type_w$}  {:>5}  {:>5}  {:>5}  {:<9}  {}",
            entry.key.namespace,
            entry.key.tenant,
            entry.key.provider,
            entry.key.action_type,
            entry.total,
            entry.covered,
            entry.uncovered,
            status,
            rules_str,
        );

        if entry.covered == 0 {
            warn!("{line}");
        } else {
            info!("{line}");
        }
    }
}

fn print_unmatched_rules(report: &CoverageReport) {
    if !report.unmatched_rules.is_empty() {
        info!("");
        warn!(
            count = report.unmatched_rules.len(),
            "Enabled rules with no matches in the scanned window"
        );
        info!(
            "  NOTE: This is window-scoped — a rule listed here may still be live \
             if it triggers rarely and simply did not fire inside the queried time range. \
             Verify against the full audit index before deleting any rule."
        );
        for name in &report.unmatched_rules {
            warn!(name = %name, "  Unmatched rule");
        }
    }
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
