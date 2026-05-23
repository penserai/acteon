package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.Map;

/**
 * A data retention policy.
 */
public class RetentionPolicy {
    @JsonProperty("id")
    private String id;

    @JsonProperty("namespace")
    private String namespace;

    @JsonProperty("tenant")
    private String tenant;

    @JsonProperty("enabled")
    private boolean enabled;

    @JsonProperty("audit_ttl_seconds")
    private Long auditTtlSeconds;

    @JsonProperty("state_ttl_seconds")
    private Long stateTtlSeconds;

    @JsonProperty("event_ttl_seconds")
    private Long eventTtlSeconds;

    @JsonProperty("compliance_hold")
    private boolean complianceHold;

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
    public boolean isEnabled() { return enabled; }
    public Long getAuditTtlSeconds() { return auditTtlSeconds; }
    public Long getStateTtlSeconds() { return stateTtlSeconds; }
    public Long getEventTtlSeconds() { return eventTtlSeconds; }
    public boolean isComplianceHold() { return complianceHold; }
    public String getCreatedAt() { return createdAt; }
    public String getUpdatedAt() { return updatedAt; }
    public String getDescription() { return description; }
    public Map<String, String> getLabels() { return labels; }
}
