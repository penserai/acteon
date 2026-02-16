use serde::{Deserialize, Serialize};

/// A node in the chain DAG visualization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagNode {
    /// Step name.
    pub name: String,
    /// Node type: `"step"` or `"sub_chain"`.
    pub node_type: String,
    /// Provider for regular steps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Action type for regular steps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_type: Option<String>,
    /// Sub-chain name for sub-chain steps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_chain_name: Option<String>,
    /// Runtime status of this step (e.g. `"completed"`, `"failed"`, `"pending"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Running sub-chain execution ID (if this sub-chain step has a child).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub child_chain_id: Option<String>,
    /// Nested DAG for sub-chain nodes (expanded view).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Box<DagResponse>>,
}

/// An edge in the chain DAG visualization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagEdge {
    /// Source step name.
    pub source: String,
    /// Target step name.
    pub target: String,
    /// Optional label describing the branch condition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Whether this edge is part of the actual execution path.
    pub on_execution_path: bool,
}

/// Response for the chain DAG visualization API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagResponse {
    /// Chain configuration name.
    pub chain_name: String,
    /// Chain execution ID (if viewing a running/completed chain).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<String>,
    /// Chain execution status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Nodes in the DAG.
    pub nodes: Vec<DagNode>,
    /// Edges connecting nodes.
    pub edges: Vec<DagEdge>,
    /// Ordered list of step names that were executed.
    pub execution_path: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dag_response_serde_roundtrip() {
        let dag = DagResponse {
            chain_name: "test-chain".into(),
            chain_id: Some("chain-123".into()),
            status: Some("running".into()),
            nodes: vec![
                DagNode {
                    name: "step1".into(),
                    node_type: "step".into(),
                    provider: Some("slack".into()),
                    action_type: Some("send_alert".into()),
                    sub_chain_name: None,
                    status: Some("completed".into()),
                    child_chain_id: None,
                    children: None,
                },
                DagNode {
                    name: "invoke-notify".into(),
                    node_type: "sub_chain".into(),
                    provider: None,
                    action_type: None,
                    sub_chain_name: Some("notify-chain".into()),
                    status: Some("waiting_sub_chain".into()),
                    child_chain_id: Some("child-456".into()),
                    children: Some(Box::new(DagResponse {
                        chain_name: "notify-chain".into(),
                        chain_id: Some("child-456".into()),
                        status: Some("running".into()),
                        nodes: vec![],
                        edges: vec![],
                        execution_path: vec![],
                    })),
                },
            ],
            edges: vec![DagEdge {
                source: "step1".into(),
                target: "invoke-notify".into(),
                label: None,
                on_execution_path: true,
            }],
            execution_path: vec!["step1".into()],
        };

        let json = serde_json::to_string(&dag).unwrap();
        let back: DagResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.chain_name, "test-chain");
        assert_eq!(back.nodes.len(), 2);
        assert_eq!(back.nodes[1].node_type, "sub_chain");
        assert!(back.nodes[1].children.is_some());
    }

    #[test]
    fn dag_response_definition_only() {
        let dag = DagResponse {
            chain_name: "my-chain".into(),
            chain_id: None,
            status: None,
            nodes: vec![DagNode {
                name: "step1".into(),
                node_type: "step".into(),
                provider: Some("email".into()),
                action_type: Some("send".into()),
                sub_chain_name: None,
                status: None,
                child_chain_id: None,
                children: None,
            }],
            edges: vec![],
            execution_path: vec![],
        };

        let json = serde_json::to_string(&dag).unwrap();
        // chain_id and status should be absent in JSON
        assert!(!json.contains("chain_id"));
        assert!(!json.contains("status"));
    }

    #[test]
    fn dag_edge_with_label() {
        let edge = DagEdge {
            source: "check".into(),
            target: "escalate".into(),
            label: Some("severity == high".into()),
            on_execution_path: false,
        };
        let json = serde_json::to_string(&edge).unwrap();
        let back: DagEdge = serde_json::from_str(&json).unwrap();
        assert_eq!(back.label.as_deref(), Some("severity == high"));
        assert!(!back.on_execution_path);
    }
}
