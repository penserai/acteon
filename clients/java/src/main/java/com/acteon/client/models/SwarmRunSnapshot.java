package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.Map;

/**
 * Snapshot of a long-running swarm goal tracked by the server.
 */
public class SwarmRunSnapshot {
    @JsonProperty("run_id")
    private String runId;

    @JsonProperty("plan_id")
    private String planId;

    @JsonProperty("objective")
    private String objective;

    @JsonProperty("status")
    private String status;

    @JsonProperty("started_at")
    private String startedAt;

    @JsonProperty("finished_at")
    private String finishedAt;

    @JsonProperty("metrics")
    private Map<String, Object> metrics;

    @JsonProperty("error")
    private String error;

    @JsonProperty("namespace")
    private String namespace;

    @JsonProperty("tenant")
    private String tenant;

    public String getRunId() { return runId; }
    public String getPlanId() { return planId; }
    public String getObjective() { return objective; }
    public String getStatus() { return status; }
    public String getStartedAt() { return startedAt; }
    public String getFinishedAt() { return finishedAt; }
    public Map<String, Object> getMetrics() { return metrics; }
    public String getError() { return error; }
    public String getNamespace() { return namespace; }
    public String getTenant() { return tenant; }
}
