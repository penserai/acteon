//! Simulation of the rule coverage API in Acteon.
//!
//! Seeds a memory audit store with a mixed workload across several
//! `(namespace, tenant, provider, action_type)` combinations — some fully
//! covered by rules, some partially, some untouched — then queries the
//! `AnalyticsStore::rule_coverage` method and renders a coverage report
//! using `acteon_core::build_report`.
//!
//! The same aggregation + report builder runs behind
//! `GET /v1/rules/coverage` in the server. This example shows the moving
//! parts without spinning up an HTTP gateway.
//!
//! Run with: `cargo run -p acteon-simulation --example rule_coverage_simulation`

use std::sync::Arc;
use std::time::Instant;

use acteon_audit::InMemoryAnalytics;
use acteon_audit::analytics::AnalyticsStore;
use acteon_audit::record::AuditRecord;
use acteon_audit::store::AuditStore;
use acteon_audit_memory::MemoryAuditStore;
use acteon_core::coverage::{CoverageQuery, CoverageReport, build_report};
use chrono::{DateTime, Duration, Utc};
use tracing::info;

#[allow(clippy::too_many_arguments)]
fn make_record(
    namespace: &str,
    tenant: &str,
    provider: &str,
    action_type: &str,
    matched_rule: Option<&str>,
    dispatched_at: DateTime<Utc>,
) -> AuditRecord {
    AuditRecord {
        id: uuid::Uuid::now_v7().to_string(),
        action_id: uuid::Uuid::now_v7().to_string(),
        chain_id: None,
        namespace: namespace.to_string(),
        tenant: tenant.to_string(),
        provider: provider.to_string(),
        action_type: action_type.to_string(),
        verdict: if matched_rule.is_some() {
            "deny".to_string()
        } else {
            "allow".to_string()
        },
        matched_rule: matched_rule.map(String::from),
        outcome: "executed".to_string(),
        action_payload: None,
        verdict_details: serde_json::json!({}),
        outcome_details: serde_json::json!({}),
        metadata: serde_json::json!({}),
        dispatched_at,
        completed_at: dispatched_at + Duration::milliseconds(50),
        duration_ms: 50,
        expires_at: None,
        caller_id: String::new(),
        auth_method: String::new(),
        record_hash: None,
        previous_hash: None,
        sequence_number: None,
        attachment_metadata: Vec::new(),
        signature: None,
        signer_id: None,
        canonical_hash: None,
    }
}

fn print_report(report: &CoverageReport) {
    info!("\n{}", "=".repeat(72));
    info!("  Rule Coverage Report");
    info!("{}", "=".repeat(72));
    info!(
        "  Window: {} -> {}",
        report.scanned_from.format("%Y-%m-%d %H:%M"),
        report.scanned_to.format("%Y-%m-%d %H:%M")
    );
    info!(
        "  total_actions={}  combinations={}  rules_loaded={}",
        report.total_actions, report.unique_combinations, report.rules_loaded
    );
    info!(
        "  fully_covered={}  partially_covered={}  uncovered={}",
        report.fully_covered, report.partially_covered, report.uncovered
    );
    info!("");
    info!(
        "  {:<10} {:<8} {:<10} {:<15} {:>6} {:>6} {:>6}  STATUS     RULES",
        "NAMESPACE", "TENANT", "PROVIDER", "ACTION_TYPE", "TOTAL", "COVER", "MISS"
    );
    info!("  {}", "-".repeat(90));

    for entry in &report.entries {
        let status = if entry.covered == 0 {
            "UNCOVERED"
        } else if entry.uncovered > 0 {
            "PARTIAL"
        } else {
            "COVERED"
        };
        let rules = if entry.matched_rules.is_empty() {
            "-".to_string()
        } else {
            entry.matched_rules.join(", ")
        };
        info!(
            "  {:<10} {:<8} {:<10} {:<15} {:>6} {:>6} {:>6}  {:<9}  {}",
            entry.key.namespace,
            entry.key.tenant,
            entry.key.provider,
            entry.key.action_type,
            entry.total,
            entry.covered,
            entry.uncovered,
            status,
            rules
        );
    }

    if !report.unmatched_rules.is_empty() {
        info!("");
        info!(
            "  {} enabled rule(s) with no matches in the scanned window:",
            report.unmatched_rules.len()
        );
        for name in &report.unmatched_rules {
            info!("    - {name}");
        }
        info!("  NOTE: window-scoped — widen --from/--to before deciding a rule is dead.");
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    info!("Acteon Rule Coverage Simulation");
    info!("===============================\n");

    let start = Instant::now();

    // ---------------------------------------------------------------
    // 1. Seed a mixed workload into a memory audit store.
    // ---------------------------------------------------------------
    //
    // Four distinct (provider, action_type) combinations, each with a
    // different coverage profile:
    //
    //  email/send   — fully covered by "block-phishing"
    //  sms/send     — partially covered: half match "rate-limit-sms",
    //                 the other half go through with no rule
    //  webhook/post — completely uncovered (no rules apply here)
    //  slack/notify — fully covered by two different rules
    //
    // Plus a legacy rule "dead-rule" that is loaded in the gateway but
    // never fires inside the scanned window — the kind of rule the
    // report surfaces so operators can investigate.
    //
    // Because dispatched_at is explicit, everything lands inside the
    // default 7-day window used by `rule_coverage`.

    let audit_store = Arc::new(MemoryAuditStore::new());
    let now = Utc::now();

    // email/send: 40 records, all matched by "block-phishing"
    for i in 0..40 {
        let ts = now - Duration::minutes(i * 2);
        let rec = make_record("prod", "acme", "email", "send", Some("block-phishing"), ts);
        audit_store.record(rec).await.unwrap();
    }

    // sms/send: 10 matched, 10 unmatched
    for i in 0..20 {
        let ts = now - Duration::minutes(i * 3);
        let rule = if i % 2 == 0 {
            Some("rate-limit-sms")
        } else {
            None
        };
        let rec = make_record("prod", "acme", "sms", "send", rule, ts);
        audit_store.record(rec).await.unwrap();
    }

    // webhook/post: 15 records, all uncovered
    for i in 0..15 {
        let ts = now - Duration::minutes(i * 4);
        let rec = make_record("prod", "acme", "webhook", "post", None, ts);
        audit_store.record(rec).await.unwrap();
    }

    // slack/notify: 8 records, split between two rules
    for i in 0..8 {
        let ts = now - Duration::minutes(i * 5);
        let rule = if i % 2 == 0 {
            "team-channels-only"
        } else {
            "no-secrets-in-slack"
        };
        let rec = make_record("prod", "acme", "slack", "notify", Some(rule), ts);
        audit_store.record(rec).await.unwrap();
    }

    info!("Populated audit store: 83 records across 4 (provider, action_type) combinations");

    // ---------------------------------------------------------------
    // 2. Query the coverage aggregates through InMemoryAnalytics.
    // ---------------------------------------------------------------
    //
    // Postgres/ClickHouse would emit the same shape via native GROUP BY
    // — the abstraction is the same.

    let analytics = InMemoryAnalytics::new(Arc::clone(&audit_store) as Arc<dyn AuditStore>);
    let query = CoverageQuery {
        namespace: Some("prod".to_string()),
        tenant: Some("acme".to_string()),
        from: None, // default: last 7 days
        to: None,
    };
    let aggregates = analytics.rule_coverage(&query).await.unwrap();

    info!(
        "Backend returned {} aggregate rows (one per unique \
         (ns, tenant, provider, action_type, matched_rule) tuple)",
        aggregates.len()
    );

    // ---------------------------------------------------------------
    // 3. Combine aggregates with the loaded rule set.
    // ---------------------------------------------------------------
    //
    // In the real server this comes from `gateway.rules()`. Here we
    // fabricate the list to show how `build_report` classifies
    // combinations and detects unmatched rules.

    let loaded_rules: Vec<(String, bool)> = vec![
        ("block-phishing".to_string(), true),
        ("rate-limit-sms".to_string(), true),
        ("team-channels-only".to_string(), true),
        ("no-secrets-in-slack".to_string(), true),
        // Enabled but never fires in the scanned window — surfaces as
        // "unmatched" in the final report.
        ("dead-rule".to_string(), true),
        // Disabled rules are ignored by the unmatched-rule detector.
        ("disabled-rule".to_string(), false),
    ];

    let scanned_from = now - Duration::days(7);
    let scanned_to = now;
    let report = build_report(&aggregates, &loaded_rules, scanned_from, scanned_to);

    // ---------------------------------------------------------------
    // 4. Render the report.
    // ---------------------------------------------------------------

    print_report(&report);

    // ---------------------------------------------------------------
    // 5. Sanity assertions — this example doubles as a smoke test.
    // ---------------------------------------------------------------

    assert_eq!(report.total_actions, 83, "expected 83 total actions");
    assert_eq!(report.unique_combinations, 4);
    assert_eq!(
        report.fully_covered, 2,
        "email and slack should be fully covered"
    );
    assert_eq!(
        report.partially_covered, 1,
        "sms should be partially covered"
    );
    assert_eq!(report.uncovered, 1, "webhook should be uncovered");
    assert_eq!(
        report.unmatched_rules,
        vec!["dead-rule".to_string()],
        "dead-rule should be the only unmatched rule"
    );

    info!("\nSimulation completed in {:?}", start.elapsed());
}
