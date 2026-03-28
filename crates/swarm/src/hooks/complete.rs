use crate::config::TesseraiConnectionConfig;
use crate::error::SwarmError;
use crate::memory::client::TesseraiClient;

/// Handle the Stop hook: mark the agent session as complete in `TesseraiDB`.
pub async fn handle_session_complete(tesserai_url: &str, agent_id: &str) -> Result<(), SwarmError> {
    let config = TesseraiConnectionConfig {
        endpoint: tesserai_url.into(),
        api_key: None,
        tenant_id: "swarm-default".into(),
    };
    let client = TesseraiClient::new(&config)?;

    let twin_id = format!("swarm-agent-{agent_id}");
    client
        .patch_twin(
            &twin_id,
            &serde_json::json!({
                "properties": {
                    "status": "completed",
                    "finished_at": chrono::Utc::now().to_rfc3339(),
                }
            }),
        )
        .await?;

    Ok(())
}
