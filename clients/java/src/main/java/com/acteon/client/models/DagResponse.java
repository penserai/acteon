package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;

/**
 * DAG representation of a chain (config or instance).
 */
public class DagResponse {
    @JsonProperty("chain_name")
    private String chainName;

    @JsonProperty("chain_id")
    private String chainId;

    private String status;

    private List<DagNode> nodes;

    private List<DagEdge> edges;

    @JsonProperty("execution_path")
    private List<String> executionPath;

    public String getChainName() { return chainName; }
    public void setChainName(String chainName) { this.chainName = chainName; }

    public String getChainId() { return chainId; }
    public void setChainId(String chainId) { this.chainId = chainId; }

    public String getStatus() { return status; }
    public void setStatus(String status) { this.status = status; }

    public List<DagNode> getNodes() { return nodes; }
    public void setNodes(List<DagNode> nodes) { this.nodes = nodes; }

    public List<DagEdge> getEdges() { return edges; }
    public void setEdges(List<DagEdge> edges) { this.edges = edges; }

    public List<String> getExecutionPath() { return executionPath; }
    public void setExecutionPath(List<String> executionPath) { this.executionPath = executionPath; }
}
