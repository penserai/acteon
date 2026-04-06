use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use acteon_ops::OpsClient;
use acteon_ops::acteon_client::{AuditQuery, AuditRecord, RuleInfo};
use acteon_ops::test_rules::{self, TestRunSummary};
use clap::{Args, Subcommand};
use serde::Serialize;
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
    /// Scans recent audit records and builds a coverage matrix showing which
    /// (namespace, tenant, provider, `action_type`) combinations were matched
    /// by a rule and which were not.
    Coverage {
        /// Maximum number of audit records to scan.
        #[arg(long, default_value = "5000")]
        limit: u32,
        /// Filter by namespace.
        #[arg(long)]
        namespace: Option<String>,
        /// Filter by tenant.
        #[arg(long)]
        tenant: Option<String>,
        /// Page size for audit queries.
        #[arg(long, default_value = "500")]
        page_size: u32,
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
            limit,
            namespace,
            tenant,
            page_size,
        } => {
            run_coverage(
                ops,
                format,
                *limit,
                namespace.as_deref(),
                tenant.as_deref(),
                *page_size,
            )
            .await?;
        }
    }
    Ok(())
}

// =========================================================================
// Coverage analysis
// =========================================================================

/// A unique combination of the four coverage dimensions.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
struct CoverageKey {
    namespace: String,
    tenant: String,
    provider: String,
    action_type: String,
}

/// Per-combination coverage stats.
#[derive(Debug, Clone, Serialize)]
struct CoverageEntry {
    #[serde(flatten)]
    key: CoverageKey,
    total: u64,
    covered: u64,
    uncovered: u64,
    matched_rules: Vec<String>,
}

/// Full coverage report.
#[derive(Debug, Serialize)]
struct CoverageReport {
    records_scanned: u64,
    unique_combinations: usize,
    fully_covered: usize,
    partially_covered: usize,
    uncovered: usize,
    rules_loaded: usize,
    entries: Vec<CoverageEntry>,
}

async fn run_coverage(
    ops: &OpsClient,
    format: &OutputFormat,
    limit: u32,
    namespace: Option<&str>,
    tenant: Option<&str>,
    page_size: u32,
) -> anyhow::Result<()> {
    // Fetch rules for context.
    let rules: Vec<RuleInfo> = ops.list_rules().await?;

    // Page through audit records.
    let mut all_records: Vec<AuditRecord> = Vec::new();
    let mut offset: u32 = 0;
    let effective_page = page_size.min(limit);

    loop {
        let remaining = limit.saturating_sub(offset);
        if remaining == 0 {
            break;
        }
        let this_page = effective_page.min(remaining);

        let query = AuditQuery {
            namespace: namespace.map(String::from),
            tenant: tenant.map(String::from),
            limit: Some(this_page),
            offset: Some(offset),
            ..Default::default()
        };

        let page = ops.query_audit(query).await?;
        #[allow(clippy::cast_possible_truncation)] // page size is bounded by u32 limit
        let fetched = page.records.len() as u32;
        all_records.extend(page.records);

        if fetched < this_page {
            break; // Last page.
        }
        offset += fetched;
    }

    // Build coverage matrix.
    let mut matrix: BTreeMap<CoverageKey, (u64, u64, BTreeSet<String>)> = BTreeMap::new();

    for record in &all_records {
        let key = CoverageKey {
            namespace: record.namespace.clone(),
            tenant: record.tenant.clone(),
            provider: record.provider.clone(),
            action_type: record.action_type.clone(),
        };

        let entry = matrix.entry(key).or_insert_with(|| (0, 0, BTreeSet::new()));
        entry.0 += 1; // total
        if let Some(ref rule_name) = record.matched_rule {
            entry.1 += 1; // covered
            entry.2.insert(rule_name.clone());
        }
    }

    // Build report.
    let entries: Vec<CoverageEntry> = matrix
        .into_iter()
        .map(|(key, (total, covered, matched_rules))| CoverageEntry {
            key,
            total,
            covered,
            uncovered: total - covered,
            matched_rules: matched_rules.into_iter().collect(),
        })
        .collect();

    let fully_covered = entries.iter().filter(|e| e.uncovered == 0).count();
    let uncovered = entries.iter().filter(|e| e.covered == 0).count();
    let partially_covered = entries.len() - fully_covered - uncovered;

    let report = CoverageReport {
        records_scanned: all_records.len() as u64,
        unique_combinations: entries.len(),
        fully_covered,
        partially_covered,
        uncovered,
        rules_loaded: rules.len(),
        entries,
    };

    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&report)?);
        }
        OutputFormat::Text => {
            print_coverage_report(&report, &rules);
        }
    }

    Ok(())
}

fn print_coverage_report(report: &CoverageReport, rules: &[RuleInfo]) {
    info!(
        records_scanned = report.records_scanned,
        rules_loaded = report.rules_loaded,
        "Coverage analysis"
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
        info!("No audit records found. Dispatch some actions first.");
        return;
    }

    print_coverage_table(&report.entries);
    print_unmatched_rules(report, rules);
}

fn print_coverage_table(entries: &[CoverageEntry]) {
    // Compute column widths.
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

    // Sort: uncovered first, then partially, then fully.
    let mut sorted: Vec<&CoverageEntry> = entries.iter().collect();
    sorted.sort_by_key(|e| {
        let order = if e.covered == 0 {
            0
        } else if e.uncovered > 0 {
            1
        } else {
            2
        };
        (order, &e.key)
    });

    for entry in &sorted {
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

fn print_unmatched_rules(report: &CoverageReport, rules: &[RuleInfo]) {
    let matched_set: BTreeSet<&str> = report
        .entries
        .iter()
        .flat_map(|e| e.matched_rules.iter().map(String::as_str))
        .collect();

    let unmatched_rules: Vec<&RuleInfo> = rules
        .iter()
        .filter(|r| r.enabled && !matched_set.contains(r.name.as_str()))
        .collect();

    if !unmatched_rules.is_empty() {
        info!("");
        warn!(
            count = unmatched_rules.len(),
            "Enabled rules with no audit matches (may be dead rules)"
        );
        for rule in &unmatched_rules {
            let desc = rule.description.as_deref().unwrap_or("");
            warn!(name = %rule.name, description = %desc, "  Unmatched rule");
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
