package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.Map;

/**
 * A quota policy.
 */
public class QuotaPolicy {
    @JsonProperty("id")
    private String id;

    @JsonProperty("namespace")
    private String namespace;

    @JsonProperty("tenant")
    private String tenant;

    @JsonProperty("max_actions")
    private long maxActions;

    @JsonProperty("window")
    private String window;

    @JsonProperty("overage_behavior")
    private String overageBehavior;

    @JsonProperty("enabled")
    private boolean enabled;

    @JsonProperty("created_at")
    private String createdAt;

    @JsonProperty("updated_at")
    private String updatedAt;

    @JsonProperty("description")
    private String description;

    @JsonProperty("labels")
    private Map<String, String> labels;

    public String getId() { return id; }
    public String getNamespace() { return namespace; }
    public String getTenant() { return tenant; }
    public long getMaxActions() { return maxActions; }
    public String getWindow() { return window; }
    public String getOverageBehavior() { return overageBehavior; }
    public boolean isEnabled() { return enabled; }
    public String getCreatedAt() { return createdAt; }
    public String getUpdatedAt() { return updatedAt; }
    public String getDescription() { return description; }
    public Map<String, String> getLabels() { return labels; }
}
