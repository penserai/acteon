use serde_json::json;
use uuid::Uuid;

use super::client::{CreateTwinRequest, TesseraiClient, TwinResponse};
use crate::error::SwarmError;

/// Store an episodic memory for an agent action (recorded after each subtask completes).
pub async fn record_action(
    client: &TesseraiClient,
    run_id: &str,
    agent_id: &str,
    tool_name: &str,
    action_summary: &str,
    topics: Vec<String>,
    structured_data: Option<serde_json::Value>,
) -> Result<TwinResponse, SwarmError> {
    let memory_id = Uuid::new_v4().to_string();
    client
        .create_twin(&CreateTwinRequest {
            id: format!("memory-{memory_id}"),
            twin_type: "EpisodicMemory".into(),
            name: Some(format!("[{tool_name}] {}", truncate(action_summary, 80))),
            description: Some(action_summary.into()),
            properties: json!({
                "memory_type": "episodic",
                "run_id": run_id,
                "agent_id": agent_id,
                "tool_name": tool_name,
                "content": action_summary,
                "topics": topics,
                "structured_data": structured_data,
                "confidence": 1.0,
                "created_at": chrono::Utc::now().to_rfc3339(),
            }),
        })
        .await
}

/// Store a semantic finding from an agent (key discoveries, analysis results).
pub async fn store_finding(
    client: &TesseraiClient,
    run_id: &str,
    agent_id: &str,
    content: &str,
    topics: Vec<String>,
    confidence: f64,
) -> Result<TwinResponse, SwarmError> {
    let memory_id = Uuid::new_v4().to_string();
    client
        .create_twin(&CreateTwinRequest {
            id: format!("finding-{memory_id}"),
            twin_type: "SemanticMemory".into(),
            name: Some(truncate(content, 100).to_string()),
            description: Some(content.into()),
            properties: json!({
                "memory_type": "semantic",
                "run_id": run_id,
                "agent_id": agent_id,
                "content": content,
                "topics": topics,
                "confidence": confidence,
                "created_at": chrono::Utc::now().to_rfc3339(),
            }),
        })
        .await
}

/// List all memories for a run (uses twin list with type filter).
pub async fn list_run_memories(
    client: &TesseraiClient,
    run_id: &str,
) -> Result<Vec<TwinResponse>, SwarmError> {
    // List episodic memories
    let episodic = client
        .list_twins(Some("EpisodicMemory"), Some(run_id))
        .await
        .unwrap_or_default();

    // List semantic findings
    let semantic = client
        .list_twins(Some("SemanticMemory"), Some(run_id))
        .await
        .unwrap_or_default();

    let mut all = episodic;
    all.extend(semantic);
    Ok(all)
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    // Find a valid char boundary at or before `max`.
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
