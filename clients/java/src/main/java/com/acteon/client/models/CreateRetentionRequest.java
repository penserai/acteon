package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.Map;

/**
 * Request to create a data retention policy.
 */
@JsonInclude(JsonInclude.Include.NON_NULL)
public class CreateRetentionRequest {
    @JsonProperty("namespace")
    private String namespace;

    @JsonProperty("tenant")
    private String tenant;

    @JsonProperty("audit_ttl_seconds")
    private Long auditTtlSeconds;

    @JsonProperty("state_ttl_seconds")
    private Long stateTtlSeconds;

    @JsonProperty("event_ttl_seconds")
    private Long eventTtlSeconds;

    @JsonProperty("compliance_hold")
    private Boolean complianceHold;

    @JsonProperty("description")
    private String description;

    @JsonProperty("labels")
    private Map<String, String> labels;

    public CreateRetentionRequest() {}

    public CreateRetentionRequest(String namespace, String tenant) {
        this.namespace = namespace;
        this.tenant = tenant;
    }

    public String getNamespace() { return namespace; }
    public void setNamespace(String namespace) { this.namespace = namespace; }

    public String getTenant() { return tenant; }
    public void setTenant(String tenant) { this.tenant = tenant; }

    public Long getAuditTtlSeconds() { return auditTtlSeconds; }
    public void setAuditTtlSeconds(Long auditTtlSeconds) { this.auditTtlSeconds = auditTtlSeconds; }

    public Long getStateTtlSeconds() { return stateTtlSeconds; }
    public void setStateTtlSeconds(Long stateTtlSeconds) { this.stateTtlSeconds = stateTtlSeconds; }

    public Long getEventTtlSeconds() { return eventTtlSeconds; }
    public void setEventTtlSeconds(Long eventTtlSeconds) { this.eventTtlSeconds = eventTtlSeconds; }

    public Boolean getComplianceHold() { return complianceHold; }
    public void setComplianceHold(Boolean complianceHold) { this.complianceHold = complianceHold; }

    public String getDescription() { return description; }
    public void setDescription(String description) { this.description = description; }

    public Map<String, String> getLabels() { return labels; }
    public void setLabels(Map<String, String> labels) { this.labels = labels; }
}
