package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * Request to update a quota policy.
 */
@JsonInclude(JsonInclude.Include.NON_NULL)
public class UpdateQuotaRequest {

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
