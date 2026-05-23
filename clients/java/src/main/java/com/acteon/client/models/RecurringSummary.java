package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * Summary of a recurring action in list responses.
 */
public class RecurringSummary {
    @JsonProperty("id")
    private String id;

    @JsonProperty("namespace")
    private String namespace;

    @JsonProperty("tenant")
    private String tenant;

    @JsonProperty("cron_expr")
    private String cronExpr;

    @JsonProperty("timezone")
    private String timezone;

    @JsonProperty("enabled")
    private boolean enabled;

    @JsonProperty("provider")
    private String provider;

    @JsonProperty("action_type")
    private String actionType;

    @JsonProperty("execution_count")
    private int executionCount;

    @JsonProperty("created_at")
    private String createdAt;

    @JsonProperty("next_execution_at")
    private String nextExecutionAt;

    @JsonProperty("description")
    private String description;

    public String getId() { return id; }
    public String getNamespace() { return namespace; }
    public String getTenant() { return tenant; }
    public String getCronExpr() { return cronExpr; }
    public String getTimezone() { return timezone; }
    public boolean isEnabled() { return enabled; }
    public String getProvider() { return provider; }
    public String getActionType() { return actionType; }
    public int getExecutionCount() { return executionCount; }
    public String getCreatedAt() { return createdAt; }
    public String getNextExecutionAt() { return nextExecutionAt; }
    public String getDescription() { return description; }
}
