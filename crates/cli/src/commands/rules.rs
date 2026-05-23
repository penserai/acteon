use std::fmt::Write;

use acteon_ops::OpsClient;
use acteon_ops::acteon_client::{CoverageEntry, CoverageQuery, CoverageReport};
use acteon_ops::test_rules::{self, TestRunSummary};
use chrono::{DateTime, Utc};
use clap::{Args, Subcommand, ValueEnum};
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
        /// Sort order for displayed entries.
        #[arg(long, value_enum, default_value_t = CoverageSort::Status)]
        sort_by: CoverageSort,
    },
}

/// Sort order for the coverage table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CoverageSort {
    /// UNCOVERED → PARTIAL → COVERED (server default).
    Status,
    /// Highest total actions first.
    Total,
    /// Highest uncovered count first.
    Miss,
    /// Alphabetical by `namespace:tenant:provider:action_type`.
    Name,
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
        RulesCommand::Coverage { .. } => {
            run_coverage(ops, &args.command, format).await?;
        }
    }
    Ok(())
}

async fn run_coverage(
    ops: &OpsClient,
    command: &RulesCommand,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    let RulesCommand::Coverage {
        namespace,
        tenant,
        since_hours,
        from,
        to,
        only_uncovered,
        min_uncovered,
        sort_by,
    } = command
    else {
        unreachable!("run_coverage called with non-Coverage command");
    };

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
            for line in render_coverage_report(&report, *only_uncovered, *min_uncovered, *sort_by) {
                if line.is_warning {
                    warn!("{}", line.text);
                } else {
                    info!("{}", line.text);
                }
            }
        }
    }

    Ok(())
}

// =========================================================================
// Coverage rendering (pure functions — testable)
// =========================================================================

/// A single line of rendered coverage output.
///
/// `is_warning` drives whether the line is emitted at WARN or INFO level so
/// operators can spot trouble in terminal output, while still letting us
/// snapshot the full rendering in unit tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedLine {
    pub text: String,
    pub is_warning: bool,
}

impl RenderedLine {
    fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_warning: false,
        }
    }

    fn warn(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_warning: true,
        }
    }
}

/// Render a coverage report into an ordered list of output lines.
///
/// Pure function — no I/O, no tracing — so it can be snapshot-tested.
pub fn render_coverage_report(
    report: &CoverageReport,
    only_uncovered: bool,
    min_uncovered: u64,
    sort_by: CoverageSort,
) -> Vec<RenderedLine> {
    let mut out = Vec::new();

    out.push(RenderedLine::info(format!(
        "Coverage analysis: scanned_from={} scanned_to={} total_actions={} rules_loaded={}",
        report.scanned_from.to_rfc3339(),
        report.scanned_to.to_rfc3339(),
        report.total_actions,
        report.rules_loaded,
    )));
    out.push(RenderedLine::info(String::new()));

    out.push(RenderedLine::info(format!(
        "Coverage summary: combinations={} fully_covered={} partially_covered={} uncovered={}",
        report.unique_combinations,
        report.fully_covered,
        report.partially_covered,
        report.uncovered,
    )));
    out.push(RenderedLine::info(String::new()));

    if report.entries.is_empty() {
        out.push(RenderedLine::info(
            "No audit records in the scanned window.",
        ));
        return out;
    }

    let mut filtered: Vec<&CoverageEntry> = report
        .entries
        .iter()
        .filter(|e| {
            if only_uncovered && e.covered > 0 {
                return false;
            }
            e.uncovered >= min_uncovered
        })
        .collect();

    // Apply local re-sort if the user asked for something other than the
    // server default (Status).
    apply_sort(&mut filtered, sort_by);

    if filtered.is_empty() {
        out.push(RenderedLine::info("No entries match the current filters."));
    } else {
        out.extend(render_coverage_table(&filtered));
    }

    out.extend(render_unmatched_rules(report));
    out
}

fn apply_sort(entries: &mut [&CoverageEntry], sort_by: CoverageSort) {
    match sort_by {
        // Server already returns entries sorted by status (UNCOVERED →
        // PARTIAL → COVERED, ties broken by key). Nothing to do.
        CoverageSort::Status => {}
        CoverageSort::Total => {
            entries.sort_by(|a, b| b.total.cmp(&a.total).then_with(|| a.key.cmp(&b.key)));
        }
        CoverageSort::Miss => {
            entries.sort_by(|a, b| {
                b.uncovered
                    .cmp(&a.uncovered)
                    .then_with(|| a.key.cmp(&b.key))
            });
        }
        CoverageSort::Name => {
            entries.sort_by(|a, b| a.key.cmp(&b.key));
        }
    }
}

fn render_coverage_table(entries: &[&CoverageEntry]) -> Vec<RenderedLine> {
    let mut out = Vec::new();

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
    let sep = "-".repeat(header.len());
    out.push(RenderedLine::info(header));
    out.push(RenderedLine::info(sep));

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
            out.push(RenderedLine::warn(line));
        } else {
            out.push(RenderedLine::info(line));
        }
    }

    out
}

fn render_unmatched_rules(report: &CoverageReport) -> Vec<RenderedLine> {
    let mut out = Vec::new();
    if report.unmatched_rules.is_empty() {
        return out;
    }

    out.push(RenderedLine::info(String::new()));
    out.push(RenderedLine::warn(format!(
        "{} enabled rule(s) with no matches in the scanned window",
        report.unmatched_rules.len()
    )));
    out.push(RenderedLine::info(
        "  NOTE: This is window-scoped — a rule listed here may still be live \
         if it triggers rarely and simply did not fire inside the queried time range. \
         Verify against the full audit index before deleting any rule.",
    ));
    for name in &report.unmatched_rules {
        out.push(RenderedLine::warn(format!("  Unmatched rule: {name}")));
    }
    out
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

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_ops::acteon_client::CoverageKey;
    use chrono::TimeZone;

    fn fixed_time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 9, 12, 0, 0).unwrap()
    }

    fn entry(
        ns: &str,
        tenant: &str,
        provider: &str,
        action_type: &str,
        total: u64,
        covered: u64,
        rules: &[&str],
    ) -> CoverageEntry {
        CoverageEntry {
            key: CoverageKey {
                namespace: ns.into(),
                tenant: tenant.into(),
                provider: provider.into(),
                action_type: action_type.into(),
            },
            total,
            covered,
            uncovered: total - covered,
            matched_rules: rules.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    fn sample_report() -> CoverageReport {
        CoverageReport {
            scanned_from: fixed_time() - chrono::Duration::hours(24),
            scanned_to: fixed_time(),
            total_actions: 30,
            unique_combinations: 3,
            fully_covered: 1,
            partially_covered: 1,
            uncovered: 1,
            rules_loaded: 4,
            // Server-side order: UNCOVERED → PARTIAL → COVERED, then by key.
            entries: vec![
                entry("prod", "acme", "webhook", "post", 10, 0, &[]),
                entry("prod", "acme", "sms", "send", 5, 3, &["allow-sms"]),
                entry("prod", "acme", "email", "send", 15, 15, &["allow-email"]),
            ],
            unmatched_rules: vec!["dead-rule".into()],
        }
    }

    fn text_of(lines: &[RenderedLine]) -> String {
        lines
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn render_includes_scan_window_header() {
        let report = sample_report();
        let out = render_coverage_report(&report, false, 0, CoverageSort::Status);
        let text = text_of(&out);

        assert!(
            text.contains("scanned_from=2026-04-08T12:00:00+00:00"),
            "scanned_from header missing: {text}"
        );
        assert!(
            text.contains("scanned_to=2026-04-09T12:00:00+00:00"),
            "scanned_to header missing: {text}"
        );
        assert!(
            text.contains("total_actions=30"),
            "total_actions missing: {text}"
        );
        assert!(
            text.contains("rules_loaded=4"),
            "rules_loaded missing: {text}"
        );
    }

    #[test]
    fn render_classifies_entries_into_status_labels() {
        let report = sample_report();
        let out = render_coverage_report(&report, false, 0, CoverageSort::Status);
        let text = text_of(&out);

        assert!(text.contains("UNCOVERED"), "missing UNCOVERED label");
        assert!(text.contains("PARTIAL"), "missing PARTIAL label");
        assert!(text.contains("COVERED"), "missing COVERED label");
    }

    #[test]
    fn render_marks_uncovered_rows_as_warnings() {
        let report = sample_report();
        let out = render_coverage_report(&report, false, 0, CoverageSort::Status);

        let warning_lines: Vec<&RenderedLine> = out.iter().filter(|l| l.is_warning).collect();
        // At least one warning for the uncovered webhook row and the dead-rule listing.
        assert!(warning_lines.iter().any(|l| l.text.contains("webhook")));
        assert!(
            warning_lines
                .iter()
                .any(|l| l.text.contains("Unmatched rule: dead-rule"))
        );
    }

    #[test]
    fn render_only_uncovered_hides_partial_and_covered() {
        let report = sample_report();
        let out = render_coverage_report(&report, true, 0, CoverageSort::Status);
        let text = text_of(&out);

        assert!(text.contains("webhook"), "uncovered entry should be shown");
        assert!(!text.contains("PARTIAL"), "PARTIAL row should be hidden");
        // COVERED data rows hidden, though the summary header still mentions the count.
        let data_rows: Vec<&RenderedLine> = out
            .iter()
            .filter(|l| l.text.contains("  COVERED  "))
            .collect();
        assert!(data_rows.is_empty(), "COVERED row should be hidden");
    }

    #[test]
    fn render_min_uncovered_filters_low_miss_counts() {
        let report = sample_report();
        // sms has 2 uncovered; webhook has 10. Only webhook should survive.
        let out = render_coverage_report(&report, false, 5, CoverageSort::Status);
        let text = text_of(&out);

        assert!(text.contains("webhook"));
        assert!(!text.contains("  sms  "), "sms row should be filtered out");
    }

    #[test]
    fn render_empty_entries_prints_no_records_message() {
        let mut report = sample_report();
        report.entries.clear();
        report.unmatched_rules.clear();

        let out = render_coverage_report(&report, false, 0, CoverageSort::Status);
        let text = text_of(&out);
        assert!(text.contains("No audit records in the scanned window."));
    }

    #[test]
    fn render_filtered_to_empty_prints_no_match_message() {
        let report = sample_report();
        // min_uncovered=100 filters everything out.
        let out = render_coverage_report(&report, false, 100, CoverageSort::Status);
        let text = text_of(&out);
        assert!(text.contains("No entries match the current filters."));
    }

    #[test]
    fn render_unmatched_rules_warning_is_window_scoped() {
        let report = sample_report();
        let out = render_coverage_report(&report, false, 0, CoverageSort::Status);
        let text = text_of(&out);

        assert!(text.contains("dead-rule"));
        // The NOTE line explicitly calls out that the result is window-scoped.
        assert!(
            text.contains("window-scoped"),
            "unmatched-rule warning should emphasize scope: {text}"
        );
    }

    #[test]
    fn sort_by_total_orders_by_total_descending() {
        let report = sample_report();
        let out = render_coverage_report(&report, false, 0, CoverageSort::Total);

        // Find the data rows (those that contain the provider names).
        let data: Vec<&str> = out
            .iter()
            .filter_map(|l| {
                let t = l.text.as_str();
                if t.contains("email") || t.contains("sms") || t.contains("webhook") {
                    Some(t)
                } else {
                    None
                }
            })
            .collect();

        // Expected order by total desc: email(15) > webhook(10) > sms(5)
        let email_pos = data.iter().position(|s| s.contains("email")).unwrap();
        let webhook_pos = data.iter().position(|s| s.contains("webhook")).unwrap();
        let sms_pos = data.iter().position(|s| s.contains("sms")).unwrap();
        assert!(email_pos < webhook_pos, "email should be before webhook");
        assert!(webhook_pos < sms_pos, "webhook should be before sms");
    }

    #[test]
    fn sort_by_miss_orders_by_uncovered_descending() {
        let report = sample_report();
        let out = render_coverage_report(&report, false, 0, CoverageSort::Miss);

        let data: Vec<&str> = out
            .iter()
            .filter_map(|l| {
                let t = l.text.as_str();
                if t.contains("email") || t.contains("sms") || t.contains("webhook") {
                    Some(t)
                } else {
                    None
                }
            })
            .collect();

        // Miss counts: webhook=10, sms=2, email=0.
        let webhook_pos = data.iter().position(|s| s.contains("webhook")).unwrap();
        let sms_pos = data.iter().position(|s| s.contains("sms")).unwrap();
        let email_pos = data.iter().position(|s| s.contains("email")).unwrap();
        assert!(webhook_pos < sms_pos);
        assert!(sms_pos < email_pos);
    }

    #[test]
    fn sort_by_name_orders_alphabetically_by_key() {
        let report = sample_report();
        let out = render_coverage_report(&report, false, 0, CoverageSort::Name);

        let data: Vec<&str> = out
            .iter()
            .filter_map(|l| {
                let t = l.text.as_str();
                if t.contains("email") || t.contains("sms") || t.contains("webhook") {
                    Some(t)
                } else {
                    None
                }
            })
            .collect();

        // Alphabetical by (provider as the discriminator within the same ns/tenant):
        // email < sms < webhook.
        assert!(data[0].contains("email"));
        assert!(data[1].contains("sms"));
        assert!(data[2].contains("webhook"));
    }

    #[test]
    fn render_table_header_and_separator_match_width() {
        let report = sample_report();
        let out = render_coverage_report(&report, false, 0, CoverageSort::Status);

        let header_line = out
            .iter()
            .find(|l| l.text.contains("NAMESPACE") && l.text.contains("STATUS"))
            .expect("header line");
        let sep_line = out
            .iter()
            .find(|l| !l.text.is_empty() && l.text.chars().all(|c| c == '-'))
            .expect("separator line");

        assert_eq!(header_line.text.len(), sep_line.text.len());
    }
}
