package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.Map;

/**
 * Request to create a quota policy.
 */
@JsonInclude(JsonInclude.Include.NON_NULL)
public class CreateQuotaRequest {
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

    @JsonProperty("description")
    private String description;

    @JsonProperty("labels")
    private Map<String, String> labels;

    public CreateQuotaRequest() {}

    public CreateQuotaRequest(String namespace, String tenant, long maxActions, String window, String overageBehavior) {
        this.namespace = namespace;
        this.tenant = tenant;
        this.maxActions = maxActions;
        this.window = window;
        this.overageBehavior = overageBehavior;
    }

    public String getNamespace() { return namespace; }
    public void setNamespace(String namespace) { this.namespace = namespace; }

    public String getTenant() { return tenant; }
    public void setTenant(String tenant) { this.tenant = tenant; }

    public long getMaxActions() { return maxActions; }
    public void setMaxActions(long maxActions) { this.maxActions = maxActions; }

    public String getWindow() { return window; }
    public void setWindow(String window) { this.window = window; }

    public String getOverageBehavior() { return overageBehavior; }
    public void setOverageBehavior(String overageBehavior) { this.overageBehavior = overageBehavior; }

    public String getDescription() { return description; }
    public void setDescription(String description) { this.description = description; }

    public Map<String, String> getLabels() { return labels; }
    public void setLabels(Map<String, String> labels) { this.labels = labels; }
}
