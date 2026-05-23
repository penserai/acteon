package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * Request to update a quota policy.
 */
@JsonInclude(JsonInclude.Include.NON_NULL)
public class UpdateQuotaRequest {
    @JsonProperty("namespace")
    private String namespace;

    @JsonProperty("tenant")
    private String tenant;

    @JsonProperty("max_actions")
    private Long maxActions;

    @JsonProperty("window")
    private String window;

    @JsonProperty("overage_behavior")
    private String overageBehavior;

    @JsonProperty("description")
    private String description;

    @JsonProperty("enabled")
    private Boolean enabled;

    public UpdateQuotaRequest() {}

    public UpdateQuotaRequest(String namespace, String tenant) {
        this.namespace = namespace;
        this.tenant = tenant;
    }

    public String getNamespace() { return namespace; }
    public void setNamespace(String namespace) { this.namespace = namespace; }

    public String getTenant() { return tenant; }
    public void setTenant(String tenant) { this.tenant = tenant; }

    public Long getMaxActions() { return maxActions; }
    public void setMaxActions(Long maxActions) { this.maxActions = maxActions; }

    public String getWindow() { return window; }
    public void setWindow(String window) { this.window = window; }

    public String getOverageBehavior() { return overageBehavior; }
    public void setOverageBehavior(String overageBehavior) { this.overageBehavior = overageBehavior; }

    public String getDescription() { return description; }
    public void setDescription(String description) { this.description = description; }

    public Boolean getEnabled() { return enabled; }
    public void setEnabled(Boolean enabled) { this.enabled = enabled; }
}
