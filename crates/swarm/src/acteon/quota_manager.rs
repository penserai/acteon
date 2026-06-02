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

/// Cancel a swarm run by installing a zero-budget, fail-closed quota on its
/// tenant (`swarm-{run_id}`).
///
/// Once the gateway's quota cache refreshes (bounded by its TTL, ~60s), the
/// `PreToolUse` gate denies every gated action — command execution, file
/// writes, web access, sub-agent spawns — for the run, so in-flight agents
/// wind down and no new gated work is admitted. Read-only tool use and the
/// orchestrator process itself are not force-killed; this is a graceful,
/// cross-process cancel via the existing safety gate, not a SIGKILL.
///
/// The quota create endpoint rejects `max_actions == 0`, so this creates the
/// policy at `1` and then clamps it to `0` via update — the update is the
/// authoritative block. It is idempotent: a repeat cancel sees the create
/// conflict (HTTP 409) and simply re-applies the clamp.
///
/// # Errors
///
/// Returns an error if Acteon is unreachable or rejects the create/update, so
/// the CLI surfaces a non-zero exit rather than silently appearing to succeed.
pub async fn block_run(config: &SwarmConfig, run_id: &str) -> Result<(), SwarmError> {
    let client = reqwest::Client::new();
    let quota_id = format!("swarm-{run_id}");
    let tenant = format!("swarm-{run_id}");
    let api_key = config.acteon.api_key.as_deref();

    // 1) Ensure a quota exists for the run tenant. `create_quota` rejects
    //    max_actions == 0, so seed at 1; an existing policy (409) is fine.
    let create_body = serde_json::json!({
        "id": quota_id,
        "namespace": config.acteon.namespace,
        "tenant": tenant,
        "max_actions": 1,
        "window_seconds": 60,
        "overage_behavior": "block",
        "enabled": true,
        "description": format!("Cancellation block for swarm run {run_id}"),
    });
    let create_url = format!("{}/v1/quotas", config.acteon.endpoint);
    let mut req = client.post(&create_url).json(&create_body);
    if let Some(key) = api_key {
        req = req.header("Authorization", format!("Bearer {key}"));
    }
    let resp = req
        .send()
        .await
        .map_err(|e| SwarmError::Acteon(format!("cancel: failed to reach Acteon: {e}")))?;
    let status = resp.status();
    if !status.is_success() && status != reqwest::StatusCode::CONFLICT {
        let body = resp.text().await.unwrap_or_default();
        return Err(SwarmError::Acteon(format!(
            "cancel: failed to create blocking quota: HTTP {status}: {body}"
        )));
    }

    // 2) Clamp the budget to zero — the authoritative block.
    let update_url = format!("{}/v1/quotas/{quota_id}", config.acteon.endpoint);
    let mut req = client
        .put(&update_url)
        .json(&serde_json::json!({ "max_actions": 0 }));
    if let Some(key) = api_key {
        req = req.header("Authorization", format!("Bearer {key}"));
    }
    let resp = req
        .send()
        .await
        .map_err(|e| SwarmError::Acteon(format!("cancel: failed to reach Acteon: {e}")))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(SwarmError::Acteon(format!(
            "cancel: failed to apply blocking quota: HTTP {status}: {body}"
        )));
    }

    tracing::info!(quota_id = %quota_id, "installed cancellation block quota");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SwarmConfig;

    /// `block_run` must surface an error (never silently succeed) when the
    /// Acteon gateway is unreachable — port 1 is reserved and refuses
    /// connections deterministically.
    #[tokio::test]
    async fn block_run_errors_when_acteon_unreachable() {
        let mut config = SwarmConfig::default();
        config.acteon.endpoint = "http://127.0.0.1:1".to_string();
        let result = block_run(&config, "test-run").await;
        assert!(
            matches!(result, Err(SwarmError::Acteon(_))),
            "expected an Acteon error, got {result:?}"
        );
    }
}
