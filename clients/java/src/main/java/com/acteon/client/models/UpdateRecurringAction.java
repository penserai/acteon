package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.Map;

/**
 * Request to update a recurring action.
 */
@JsonInclude(JsonInclude.Include.NON_NULL)
public class UpdateRecurringAction {
    @JsonProperty("namespace")
    private String namespace;

    @JsonProperty("tenant")
    private String tenant;

    @JsonProperty("name")
    private String name;

    @JsonProperty("payload")
    private Map<String, Object> payload;

    @JsonProperty("metadata")
    private Map<String, String> metadata;

    @JsonProperty("cron_expression")
    private String cronExpression;

    @JsonProperty("timezone")
    private String timezone;

    @JsonProperty("end_date")
    private String endDate;

    @JsonProperty("max_executions")
    private Integer maxExecutions;

    @JsonProperty("description")
    private String description;

    @JsonProperty("dedup_key")
    private String dedupKey;

    @JsonProperty("labels")
    private Map<String, String> labels;

    public UpdateRecurringAction() {}

    public UpdateRecurringAction(String namespace, String tenant) {
        this.namespace = namespace;
        this.tenant = tenant;
    }

    public String getNamespace() { return namespace; }
    public void setNamespace(String namespace) { this.namespace = namespace; }

    public String getTenant() { return tenant; }
    public void setTenant(String tenant) { this.tenant = tenant; }

    public String getName() { return name; }
    public void setName(String name) { this.name = name; }

    public Map<String, Object> getPayload() { return payload; }
    public void setPayload(Map<String, Object> payload) { this.payload = payload; }

    public Map<String, String> getMetadata() { return metadata; }
    public void setMetadata(Map<String, String> metadata) { this.metadata = metadata; }

    public String getCronExpression() { return cronExpression; }
    public void setCronExpression(String cronExpression) { this.cronExpression = cronExpression; }

    public String getTimezone() { return timezone; }
    public void setTimezone(String timezone) { this.timezone = timezone; }

    public String getEndDate() { return endDate; }
    public void setEndDate(String endDate) { this.endDate = endDate; }

    public Integer getMaxExecutions() { return maxExecutions; }
    public void setMaxExecutions(Integer maxExecutions) { this.maxExecutions = maxExecutions; }

    public String getDescription() { return description; }
    public void setDescription(String description) { this.description = description; }

    public String getDedupKey() { return dedupKey; }
    public void setDedupKey(String dedupKey) { this.dedupKey = dedupKey; }

    public Map<String, String> getLabels() { return labels; }
    public void setLabels(Map<String, String> labels) { this.labels = labels; }
}
