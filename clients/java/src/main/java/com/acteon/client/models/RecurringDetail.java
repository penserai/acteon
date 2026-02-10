package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.Map;

/**
 * Detailed information about a recurring action.
 */
public class RecurringDetail {
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

    @JsonProperty("payload")
    private Map<String, Object> payload;

    @JsonProperty("metadata")
    private Map<String, String> metadata;

    @JsonProperty("execution_count")
    private int executionCount;

    @JsonProperty("created_at")
    private String createdAt;

    @JsonProperty("updated_at")
    private String updatedAt;

    @JsonProperty("labels")
    private Map<String, String> labels;

    @JsonProperty("next_execution_at")
    private String nextExecutionAt;

    @JsonProperty("last_executed_at")
    private String lastExecutedAt;

    @JsonProperty("ends_at")
    private String endsAt;

    @JsonProperty("description")
    private String description;

    @JsonProperty("dedup_key")
    private String dedupKey;

    public String getId() { return id; }
    public String getNamespace() { return namespace; }
    public String getTenant() { return tenant; }
    public String getCronExpr() { return cronExpr; }
    public String getTimezone() { return timezone; }
    public boolean isEnabled() { return enabled; }
    public String getProvider() { return provider; }
    public String getActionType() { return actionType; }
    public Map<String, Object> getPayload() { return payload; }
    public Map<String, String> getMetadata() { return metadata; }
    public int getExecutionCount() { return executionCount; }
    public String getCreatedAt() { return createdAt; }
    public String getUpdatedAt() { return updatedAt; }
    public Map<String, String> getLabels() { return labels; }
    public String getNextExecutionAt() { return nextExecutionAt; }
    public String getLastExecutedAt() { return lastExecutedAt; }
    public String getEndsAt() { return endsAt; }
    public String getDescription() { return description; }
    public String getDedupKey() { return dedupKey; }
}
