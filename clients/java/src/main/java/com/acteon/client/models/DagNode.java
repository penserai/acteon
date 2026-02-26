package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;

/**
 * A node in the chain DAG.
 */
public class DagNode {
    private String name;

    @JsonProperty("node_type")
    private String nodeType;

    private String provider;

    @JsonProperty("action_type")
    private String actionType;

    @JsonProperty("sub_chain_name")
    private String subChainName;

    private String status;

    @JsonProperty("child_chain_id")
    private String childChainId;

    private DagResponse children;

    @JsonProperty("parallel_children")
    private List<DagNode> parallelChildren;

    @JsonProperty("parallel_join")
    private String parallelJoin;

    public String getName() { return name; }
    public void setName(String name) { this.name = name; }

    public String getNodeType() { return nodeType; }
    public void setNodeType(String nodeType) { this.nodeType = nodeType; }

    public String getProvider() { return provider; }
    public void setProvider(String provider) { this.provider = provider; }

    public String getActionType() { return actionType; }
    public void setActionType(String actionType) { this.actionType = actionType; }

    public String getSubChainName() { return subChainName; }
    public void setSubChainName(String subChainName) { this.subChainName = subChainName; }

    public String getStatus() { return status; }
    public void setStatus(String status) { this.status = status; }

    public String getChildChainId() { return childChainId; }
    public void setChildChainId(String childChainId) { this.childChainId = childChainId; }

    public DagResponse getChildren() { return children; }
    public void setChildren(DagResponse children) { this.children = children; }

    public List<DagNode> getParallelChildren() { return parallelChildren; }
    public void setParallelChildren(List<DagNode> parallelChildren) { this.parallelChildren = parallelChildren; }

    public String getParallelJoin() { return parallelJoin; }
    public void setParallelJoin(String parallelJoin) { this.parallelJoin = parallelJoin; }
}
