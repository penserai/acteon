package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * Current usage statistics for a quota.
 */
public class QuotaUsage {
    @JsonProperty("tenant")
    private String tenant;

    @JsonProperty("namespace")
    private String namespace;

    @JsonProperty("used")
    private long used;

    @JsonProperty("limit")
    private long limit;

    @JsonProperty("remaining")
    private long remaining;

    @JsonProperty("window")
    private String window;

    @JsonProperty("resets_at")
    private String resetsAt;

    @JsonProperty("overage_behavior")
    private String overageBehavior;

    public String getTenant() { return tenant; }
    public String getNamespace() { return namespace; }
    public long getUsed() { return used; }
    public long getLimit() { return limit; }
    public long getRemaining() { return remaining; }
    public String getWindow() { return window; }
    public String getResetsAt() { return resetsAt; }
    public String getOverageBehavior() { return overageBehavior; }
}
