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

/// Retrieve prior findings from `TesseraiDB` across all runs in this tenant.
///
/// Returns semantic findings (not episodic actions) as context strings
/// that can be injected into agent prompts. Each finding is truncated
/// to avoid blowing up the prompt.
pub async fn retrieve_prior_context(
    client: &TesseraiClient,
    max_findings: usize,
    max_chars_per_finding: usize,
) -> Vec<String> {
    let findings = client
        .list_twins(Some("SemanticMemory"), None)
        .await
        .unwrap_or_default();

    let mut contexts = Vec::new();

    for twin_summary in findings.into_iter().take(max_findings * 2) {
        let content = client
            .get_twin(&twin_summary.id)
            .await
            .ok()
            .and_then(|full| {
                full.properties
                    .get("content")
                    .and_then(|v| v.as_str())
                    .filter(|c| c.len() > 50)
                    .map(|c| truncate(c, max_chars_per_finding).to_string())
            });

        if let Some(text) = content {
            contexts.push(text);
            if contexts.len() >= max_findings {
                break;
            }
        }
    }

    contexts
}

/// Format prior findings into a prompt section.
///
/// Returns an empty string if no prior findings exist.
pub fn format_prior_context(findings: &[String]) -> String {
    use std::fmt::Write as _;

    if findings.is_empty() {
        return String::new();
    }

    let mut section = String::from(
        "\n\n## Prior Findings (from previous agents)\n\
         The following findings were produced by other agents. \
         Use them to avoid duplicate work and build on prior research.\n\n",
    );

    for (i, finding) in findings.iter().enumerate() {
        let _ = write!(section, "### Finding {}\n{finding}\n\n", i + 1);
    }

    section.push_str(
        "---\n*Use these findings as context. \
         Do not repeat research that has already been done.*\n",
    );

    section
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
