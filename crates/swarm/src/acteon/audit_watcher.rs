use crate::config::SwarmConfig;
use crate::error::SwarmError;

/// Summary of audit events for a swarm run.
#[derive(Debug, Clone, Default)]
pub struct AuditSummary {
    pub total_dispatched: u64,
    pub executed: u64,
    pub suppressed: u64,
    pub throttled: u64,
    pub deduplicated: u64,
    pub pending_approval: u64,
    pub quota_exceeded: u64,
    pub rerouted: u64,
}

/// Fetch an audit summary for a swarm run from Acteon.
pub async fn fetch_audit_summary(
    config: &SwarmConfig,
    run_id: &str,
) -> Result<AuditSummary, SwarmError> {
    let client = reqwest::Client::new();
    let tenant = format!("swarm-{run_id}");
    let url = format!(
        "{}/v1/audit?namespace={}&tenant={}&limit=500",
        config.acteon.endpoint, config.acteon.namespace, tenant,
    );

    let mut req = client.get(&url);
    if let Some(ref key) = config.acteon.api_key {
        req = req.header("Authorization", format!("Bearer {key}"));
    }

    let resp = req
        .send()
        .await
        .map_err(|e| SwarmError::Acteon(format!("failed to fetch audit: {e}")))?;

    if !resp.status().is_success() {
        return Ok(AuditSummary::default());
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| SwarmError::Acteon(format!("failed to parse audit response: {e}")))?;

    let records = body
        .get("records")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();

    let mut summary = AuditSummary {
        total_dispatched: records.len() as u64,
        ..AuditSummary::default()
    };

    for record in &records {
        let outcome = record
            .get("outcome")
            .and_then(|o| o.as_str())
            .unwrap_or("unknown");

        match outcome {
            "Executed" => summary.executed += 1,
            "Suppressed" => summary.suppressed += 1,
            "Throttled" => summary.throttled += 1,
            "Deduplicated" => summary.deduplicated += 1,
            "PendingApproval" => summary.pending_approval += 1,
            "QuotaExceeded" => summary.quota_exceeded += 1,
            "Rerouted" => summary.rerouted += 1,
            _ => {}
        }
    }

    Ok(summary)
}
