package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * An edge in the chain DAG.
 */
public class DagEdge {
    private String source;
    private String target;
    private String label;

    @JsonProperty("on_execution_path")
    private boolean onExecutionPath;

    public String getSource() { return source; }
    public void setSource(String source) { this.source = source; }

    public String getTarget() { return target; }
    public void setTarget(String target) { this.target = target; }

    public String getLabel() { return label; }
    public void setLabel(String label) { this.label = label; }

    public boolean isOnExecutionPath() { return onExecutionPath; }
    public void setOnExecutionPath(boolean onExecutionPath) { this.onExecutionPath = onExecutionPath; }
}
