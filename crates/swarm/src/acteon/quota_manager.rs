use crate::config::SwarmConfig;
use crate::error::SwarmError;

/// Create a per-run quota in Acteon.
///
/// Uses the Acteon REST API directly since `acteon-ops` may not have
/// all the quota fields we need.
pub async fn create_run_quota(
    config: &SwarmConfig,
    run_id: &str,
    estimated_actions: u64,
) -> Result<String, SwarmError> {
    let client = reqwest::Client::new();
    let quota_id = format!("swarm-{run_id}");

    // 50% buffer over estimated actions.
    let max_actions = estimated_actions.saturating_mul(3).saturating_div(2);
    let window = config.defaults.max_duration_minutes.saturating_mul(60);

    let body = serde_json::json!({
        "id": quota_id,
        "namespace": config.acteon.namespace,
        "tenant": format!("swarm-{run_id}"),
        "max_actions": max_actions,
        "window_seconds": window,
        "overage_behavior": "block",
        "enabled": true,
        "description": format!("Auto-generated quota for swarm run {run_id}"),
    });

    let url = format!("{}/v1/quotas", config.acteon.endpoint);
    let mut req = client.post(&url).json(&body);

    if let Some(ref key) = config.acteon.api_key {
        req = req.header("Authorization", format!("Bearer {key}"));
    }

    let resp = req
        .send()
        .await
        .map_err(|e| SwarmError::Acteon(format!("failed to create quota: {e}")))?;

    if resp.status().is_success() {
        tracing::info!(quota_id = %quota_id, max_actions, "created run quota");
        Ok(quota_id)
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Err(SwarmError::Acteon(format!(
            "failed to create quota: HTTP {status}: {body}"
        )))
    }
}

/// Delete a per-run quota from Acteon.
pub async fn delete_run_quota(config: &SwarmConfig, quota_id: &str) -> Result<(), SwarmError> {
    let client = reqwest::Client::new();
    let url = format!("{}/v1/quotas/{quota_id}", config.acteon.endpoint);

    let mut req = client.delete(&url);
    if let Some(ref key) = config.acteon.api_key {
        req = req.header("Authorization", format!("Bearer {key}"));
    }

    let resp = req
        .send()
        .await
        .map_err(|e| SwarmError::Acteon(format!("failed to delete quota: {e}")))?;

    if resp.status().is_success() || resp.status() == reqwest::StatusCode::NOT_FOUND {
        tracing::info!(quota_id = %quota_id, "deleted run quota");
        Ok(())
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Err(SwarmError::Acteon(format!(
            "failed to delete quota: HTTP {status}: {body}"
        )))
    }
}
