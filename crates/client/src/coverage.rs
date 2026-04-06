use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{ActeonClient, AuditQuery, AuditRecord, Error, RuleInfo};

/// Options for a rule coverage analysis.
#[derive(Debug, Clone)]
pub struct CoverageQuery {
    /// Maximum number of audit records to scan.
    pub limit: u32,
    /// Filter by namespace.
    pub namespace: Option<String>,
    /// Filter by tenant.
    pub tenant: Option<String>,
    /// Page size for audit queries.
    pub page_size: u32,
}

impl Default for CoverageQuery {
    fn default() -> Self {
        Self {
            limit: 5000,
            namespace: None,
            tenant: None,
            page_size: 500,
        }
    }
}

impl CoverageQuery {
    /// Create a new coverage query with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum number of audit records to scan.
    #[must_use]
    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = limit;
        self
    }

    /// Filter by namespace.
    #[must_use]
    pub fn namespace(mut self, namespace: impl Into<String>) -> Self {
        self.namespace = Some(namespace.into());
        self
    }

    /// Filter by tenant.
    #[must_use]
    pub fn tenant(mut self, tenant: impl Into<String>) -> Self {
        self.tenant = Some(tenant.into());
        self
    }

    /// Set the page size for audit queries.
    #[must_use]
    pub fn page_size(mut self, page_size: u32) -> Self {
        self.page_size = page_size;
        self
    }
}

/// A unique combination of the four coverage dimensions.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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
pub struct CoverageEntry {
    /// The dimension combination.
    #[serde(flatten)]
    pub key: CoverageKey,
    /// Total number of actions dispatched for this combination.
    pub total: u64,
    /// Number of actions that matched a rule.
    pub covered: u64,
    /// Number of actions that matched no rule.
    pub uncovered: u64,
    /// Names of rules that matched actions in this combination.
    pub matched_rules: Vec<String>,
}

/// Full rule coverage report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageReport {
    /// Total audit records scanned.
    pub records_scanned: u64,
    /// Number of unique dimension combinations found.
    pub unique_combinations: usize,
    /// Combinations where every action matched a rule.
    pub fully_covered: usize,
    /// Combinations where some actions matched and some did not.
    pub partially_covered: usize,
    /// Combinations where no action matched any rule.
    pub uncovered: usize,
    /// Total number of rules loaded in the gateway.
    pub rules_loaded: usize,
    /// Per-combination coverage entries.
    pub entries: Vec<CoverageEntry>,
    /// Enabled rules that never matched any audited action.
    pub unmatched_rules: Vec<String>,
}

impl ActeonClient {
    /// Analyze rule coverage by scanning the audit trail.
    ///
    /// Pages through audit records and builds a coverage matrix showing which
    /// (namespace, tenant, provider, `action_type`) combinations were matched
    /// by a rule and which were not.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::{ActeonClient, CoverageQuery};
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let query = CoverageQuery::new().limit(10000).namespace("prod");
    /// let report = client.rules_coverage(&query).await?;
    ///
    /// println!("Scanned {} records", report.records_scanned);
    /// println!("Uncovered combinations: {}", report.uncovered);
    /// for entry in &report.entries {
    ///     if entry.uncovered > 0 {
    ///         println!(
    ///             "  {}/{}/{}/{}: {} uncovered",
    ///             entry.key.namespace, entry.key.tenant,
    ///             entry.key.provider, entry.key.action_type,
    ///             entry.uncovered,
    ///         );
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn rules_coverage(&self, query: &CoverageQuery) -> Result<CoverageReport, Error> {
        let rules: Vec<RuleInfo> = self.list_rules().await?;

        let mut all_records: Vec<AuditRecord> = Vec::new();
        let mut offset: u32 = 0;
        let effective_page = query.page_size.min(query.limit);

        loop {
            let remaining = query.limit.saturating_sub(offset);
            if remaining == 0 {
                break;
            }
            let this_page = effective_page.min(remaining);

            let audit_query = AuditQuery {
                namespace: query.namespace.clone(),
                tenant: query.tenant.clone(),
                limit: Some(this_page),
                offset: Some(offset),
                ..Default::default()
            };

            let page = self.query_audit(&audit_query).await?;
            #[allow(clippy::cast_possible_truncation)]
            let fetched = page.records.len() as u32;
            all_records.extend(page.records);

            if fetched < this_page {
                break;
            }
            offset += fetched;
        }

        Ok(build_report(&all_records, &rules))
    }
}

fn build_report(records: &[AuditRecord], rules: &[RuleInfo]) -> CoverageReport {
    let mut matrix: BTreeMap<CoverageKey, (u64, u64, BTreeSet<String>)> = BTreeMap::new();

    for record in records {
        let key = CoverageKey {
            namespace: record.namespace.clone(),
            tenant: record.tenant.clone(),
            provider: record.provider.clone(),
            action_type: record.action_type.clone(),
        };

        let entry = matrix.entry(key).or_insert_with(|| (0, 0, BTreeSet::new()));
        entry.0 += 1;
        if let Some(ref rule_name) = record.matched_rule {
            entry.1 += 1;
            entry.2.insert(rule_name.clone());
        }
    }

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
    let uncovered_count = entries.iter().filter(|e| e.covered == 0).count();
    let partially_covered = entries.len() - fully_covered - uncovered_count;

    let matched_set: BTreeSet<&str> = entries
        .iter()
        .flat_map(|e| e.matched_rules.iter().map(String::as_str))
        .collect();

    let unmatched_rules: Vec<String> = rules
        .iter()
        .filter(|r| r.enabled && !matched_set.contains(r.name.as_str()))
        .map(|r| r.name.clone())
        .collect();

    CoverageReport {
        records_scanned: records.len() as u64,
        unique_combinations: entries.len(),
        fully_covered,
        partially_covered,
        uncovered: uncovered_count,
        rules_loaded: rules.len(),
        entries,
        unmatched_rules,
    }
}
