use super::client::{CreateMemoryRequest, MemoryRecord, MemorySearchQuery, TesseraiClient};
use crate::error::SwarmError;

/// Store an episodic memory for an agent action (called from `PostToolUse` hook).
pub async fn record_action(
    client: &TesseraiClient,
    agent_id: &str,
    tool_name: &str,
    action_summary: &str,
    topics: Vec<String>,
    structured_data: Option<serde_json::Value>,
) -> Result<MemoryRecord, SwarmError> {
    client
        .create_memory(&CreateMemoryRequest {
            memory_type: "episodic".into(),
            record_type: "episode".into(),
            agent_id: agent_id.into(),
            content: action_summary.into(),
            summary: None,
            structured_data,
            context: Some(format!("Tool: {tool_name}")),
            topics,
            categories: vec!["agent-action".into()],
            confidence: 1.0,
        })
        .await
}

/// Store a semantic finding from an agent.
pub async fn store_finding(
    client: &TesseraiClient,
    agent_id: &str,
    content: &str,
    topics: Vec<String>,
    confidence: f64,
) -> Result<MemoryRecord, SwarmError> {
    client
        .create_memory(&CreateMemoryRequest {
            memory_type: "semantic".into(),
            record_type: "fact".into(),
            agent_id: agent_id.into(),
            content: content.into(),
            summary: None,
            structured_data: None,
            context: None,
            topics,
            categories: vec!["agent-finding".into()],
            confidence,
        })
        .await
}

/// Search for relevant memories across all agents in this swarm run.
pub async fn search_findings(
    client: &TesseraiClient,
    query: &str,
    limit: u32,
) -> Result<Vec<MemoryRecord>, SwarmError> {
    client
        .search_memories(&MemorySearchQuery {
            query: query.into(),
            agent_id: None,
            memory_type: Some("semantic".into()),
            limit: Some(limit),
            min_confidence: Some(0.5),
        })
        .await
}

/// Search episodic memories for a specific agent.
pub async fn search_agent_actions(
    client: &TesseraiClient,
    agent_id: &str,
    query: &str,
    limit: u32,
) -> Result<Vec<MemoryRecord>, SwarmError> {
    client
        .search_memories(&MemorySearchQuery {
            query: query.into(),
            agent_id: Some(agent_id.into()),
            memory_type: Some("episodic".into()),
            limit: Some(limit),
            min_confidence: None,
        })
        .await
}
