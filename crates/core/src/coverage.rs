//! Rule coverage types shared between the server, audit backends, and clients.
//!
//! These types power the `GET /v1/rules/coverage` endpoint. The audit backend
//! emits [`CoverageAggregate`] rows (one per unique combination of namespace,
//! tenant, provider, `action_type`, and matched rule) and the server combines
//! those rows with the currently-loaded rule set to build a [`CoverageReport`].

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Query parameters for rule coverage analysis.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CoverageQuery {
    /// Filter by namespace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Filter by tenant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    /// Start of the time range (inclusive). Defaults to 7 days ago.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<DateTime<Utc>>,
    /// End of the time range (inclusive). Defaults to now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<DateTime<Utc>>,
}

/// A single aggregated row emitted by an audit backend for rule coverage.
///
/// Each row represents the count of audit records sharing the same
/// `(namespace, tenant, provider, action_type, matched_rule)` tuple within the
/// queried time range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageAggregate {
    /// Namespace dimension.
    pub namespace: String,
    /// Tenant dimension.
    pub tenant: String,
    /// Provider dimension.
    pub provider: String,
    /// Action type dimension.
    pub action_type: String,
    /// Name of the rule that matched these records, or `None` if no rule matched.
    pub matched_rule: Option<String>,
    /// Number of audit records in this aggregate.
    pub count: u64,
}

/// A unique combination of coverage dimensions.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CoverageKey {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Provider.
    pub provider: String,
    /// Action type.
    pub action_type: String,
}

/// Per-combination coverage statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CoverageEntry {
    /// The dimension combination.
    #[serde(flatten)]
    pub key: CoverageKey,
    /// Total audit records for this combination in the scanned window.
    pub total: u64,
    /// Records that matched a rule.
    pub covered: u64,
    /// Records that matched no rule.
    pub uncovered: u64,
    /// Names of rules that matched records in this combination.
    pub matched_rules: Vec<String>,
}

/// A full rule coverage report.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CoverageReport {
    /// Start of the scanned time window (inclusive).
    pub scanned_from: DateTime<Utc>,
    /// End of the scanned time window (inclusive).
    pub scanned_to: DateTime<Utc>,
    /// Total number of audit records summarized in this report.
    pub total_actions: u64,
    /// Number of unique dimension combinations.
    pub unique_combinations: usize,
    /// Combinations where every action matched a rule.
    pub fully_covered: usize,
    /// Combinations where some actions matched and some did not.
    pub partially_covered: usize,
    /// Combinations where no action matched any rule.
    pub uncovered: usize,
    /// Total number of rules loaded in the gateway.
    pub rules_loaded: usize,
    /// Per-combination coverage entries, sorted (`UNCOVERED` → `PARTIAL` → `COVERED`).
    pub entries: Vec<CoverageEntry>,
    /// Enabled rules that did not match any action within the scanned window.
    ///
    /// This list is window-scoped: a rule listed here may still be live if it
    /// triggers rarely and simply did not fire inside the queried time range.
    pub unmatched_rules: Vec<String>,
}

/// Build a [`CoverageReport`] from a list of aggregate rows and the currently-loaded rules.
///
/// `rules` is a slice of `(rule_name, enabled)` tuples representing the rules
/// loaded in the gateway. This keeps the function free of any dependency on
/// the rules crate.
#[must_use]
pub fn build_report(
    aggregates: &[CoverageAggregate],
    rules: &[(String, bool)],
    scanned_from: DateTime<Utc>,
    scanned_to: DateTime<Utc>,
) -> CoverageReport {
    // Group aggregates by (namespace, tenant, provider, action_type).
    let mut matrix: BTreeMap<CoverageKey, (u64, u64, BTreeSet<String>)> = BTreeMap::new();
    let mut total_actions: u64 = 0;

    for agg in aggregates {
        let key = CoverageKey {
            namespace: agg.namespace.clone(),
            tenant: agg.tenant.clone(),
            provider: agg.provider.clone(),
            action_type: agg.action_type.clone(),
        };

        total_actions = total_actions.saturating_add(agg.count);

        let entry = matrix.entry(key).or_insert_with(|| (0, 0, BTreeSet::new()));
        entry.0 = entry.0.saturating_add(agg.count); // total
        if let Some(ref rule_name) = agg.matched_rule {
            entry.1 = entry.1.saturating_add(agg.count); // covered
            entry.2.insert(rule_name.clone());
        }
    }

    let mut entries: Vec<CoverageEntry> = matrix
        .into_iter()
        .map(|(key, (total, covered, matched_rules))| CoverageEntry {
            key,
            total,
            covered,
            uncovered: total - covered,
            matched_rules: matched_rules.into_iter().collect(),
        })
        .collect();

    // Stable sort: UNCOVERED first, then PARTIAL, then COVERED, ties broken by key.
    entries.sort_by(|a, b| {
        let order_a = status_order(a);
        let order_b = status_order(b);
        order_a.cmp(&order_b).then_with(|| a.key.cmp(&b.key))
    });

    let fully_covered = entries.iter().filter(|e| e.uncovered == 0).count();
    let uncovered_count = entries.iter().filter(|e| e.covered == 0).count();
    let partially_covered = entries.len() - fully_covered - uncovered_count;

    let matched_set: BTreeSet<&str> = entries
        .iter()
        .flat_map(|e| e.matched_rules.iter().map(String::as_str))
        .collect();

    let mut unmatched_rules: Vec<String> = rules
        .iter()
        .filter(|(name, enabled)| *enabled && !matched_set.contains(name.as_str()))
        .map(|(name, _)| name.clone())
        .collect();
    unmatched_rules.sort();

    CoverageReport {
        scanned_from,
        scanned_to,
        total_actions,
        unique_combinations: entries.len(),
        fully_covered,
        partially_covered,
        uncovered: uncovered_count,
        rules_loaded: rules.len(),
        entries,
        unmatched_rules,
    }
}

fn status_order(entry: &CoverageEntry) -> u8 {
    if entry.covered == 0 {
        0 // UNCOVERED
    } else if entry.uncovered > 0 {
        1 // PARTIAL
    } else {
        2 // COVERED
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agg(
        ns: &str,
        t: &str,
        p: &str,
        at: &str,
        rule: Option<&str>,
        count: u64,
    ) -> CoverageAggregate {
        CoverageAggregate {
            namespace: ns.into(),
            tenant: t.into(),
            provider: p.into(),
            action_type: at.into(),
            matched_rule: rule.map(String::from),
            count,
        }
    }

    #[test]
    fn build_report_classifies_combinations() {
        let aggregates = vec![
            // Fully covered: all 5 hits matched rule "allow-email"
            agg("prod", "acme", "email", "send", Some("allow-email"), 5),
            // Partial: 3 matched "allow-sms", 2 unmatched
            agg("prod", "acme", "sms", "send", Some("allow-sms"), 3),
            agg("prod", "acme", "sms", "send", None, 2),
            // Uncovered: 10 actions, none matched
            agg("prod", "acme", "webhook", "post", None, 10),
        ];

        let rules = vec![
            ("allow-email".to_string(), true),
            ("allow-sms".to_string(), true),
            ("dead-rule".to_string(), true),
            ("disabled-rule".to_string(), false),
        ];

        let from = Utc::now() - chrono::Duration::days(1);
        let to = Utc::now();

        let report = build_report(&aggregates, &rules, from, to);

        assert_eq!(report.total_actions, 20);
        assert_eq!(report.unique_combinations, 3);
        assert_eq!(report.fully_covered, 1);
        assert_eq!(report.partially_covered, 1);
        assert_eq!(report.uncovered, 1);
        assert_eq!(report.rules_loaded, 4);

        // UNCOVERED first.
        assert_eq!(report.entries[0].key.provider, "webhook");
        assert_eq!(report.entries[0].covered, 0);
        // PARTIAL next.
        assert_eq!(report.entries[1].key.provider, "sms");
        assert_eq!(report.entries[1].covered, 3);
        assert_eq!(report.entries[1].uncovered, 2);
        // COVERED last.
        assert_eq!(report.entries[2].key.provider, "email");
        assert_eq!(report.entries[2].uncovered, 0);

        // Dead rule detected; disabled rule NOT listed; matched rules NOT listed.
        assert_eq!(report.unmatched_rules, vec!["dead-rule".to_string()]);
    }

    #[test]
    fn build_report_handles_empty_aggregates() {
        let rules = vec![("r1".to_string(), true)];
        let from = Utc::now() - chrono::Duration::days(1);
        let to = Utc::now();
        let report = build_report(&[], &rules, from, to);

        assert_eq!(report.total_actions, 0);
        assert_eq!(report.unique_combinations, 0);
        assert!(report.entries.is_empty());
        // Unmatched includes all enabled rules when there are no aggregates.
        assert_eq!(report.unmatched_rules, vec!["r1".to_string()]);
    }

    #[test]
    fn build_report_merges_same_key_rows() {
        // Two rows sharing the same dimensions (one matched, one not) should
        // merge into a single entry.
        let aggregates = vec![
            agg("prod", "acme", "email", "send", Some("rule-a"), 3),
            agg("prod", "acme", "email", "send", Some("rule-b"), 2),
            agg("prod", "acme", "email", "send", None, 1),
        ];
        let rules = vec![("rule-a".to_string(), true), ("rule-b".to_string(), true)];
        let from = Utc::now();
        let to = Utc::now();

        let report = build_report(&aggregates, &rules, from, to);

        assert_eq!(report.unique_combinations, 1);
        assert_eq!(report.entries[0].total, 6);
        assert_eq!(report.entries[0].covered, 5);
        assert_eq!(report.entries[0].uncovered, 1);
        assert_eq!(report.entries[0].matched_rules, vec!["rule-a", "rule-b"]);
    }
}
