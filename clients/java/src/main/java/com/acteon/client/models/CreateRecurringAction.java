package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.Map;

/**
 * Request to create a recurring action.
 */
@JsonInclude(JsonInclude.Include.NON_NULL)
public class CreateRecurringAction {
    @JsonProperty("namespace")
    private String namespace;

    @JsonProperty("tenant")
    private String tenant;

    @JsonProperty("provider")
    private String provider;

    @JsonProperty("action_type")
    private String actionType;

    @JsonProperty("payload")
    private Map<String, Object> payload;

    @JsonProperty("cron_expression")
    private String cronExpression;

    @JsonProperty("name")
    private String name;

    @JsonProperty("metadata")
    private Map<String, String> metadata;

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

    public CreateRecurringAction() {}

    public CreateRecurringAction(String namespace, String tenant, String provider, String actionType,
                                 Map<String, Object> payload, String cronExpression) {
        this.namespace = namespace;
        this.tenant = tenant;
        this.provider = provider;
        this.actionType = actionType;
        this.payload = payload;
        this.cronExpression = cronExpression;
    }

    public String getNamespace() { return namespace; }
    public void setNamespace(String namespace) { this.namespace = namespace; }

    public String getTenant() { return tenant; }
    public void setTenant(String tenant) { this.tenant = tenant; }

    public String getProvider() { return provider; }
    public void setProvider(String provider) { this.provider = provider; }

    public String getActionType() { return actionType; }
    public void setActionType(String actionType) { this.actionType = actionType; }

    public Map<String, Object> getPayload() { return payload; }
    public void setPayload(Map<String, Object> payload) { this.payload = payload; }

    public String getCronExpression() { return cronExpression; }
    public void setCronExpression(String cronExpression) { this.cronExpression = cronExpression; }

    public String getName() { return name; }
    public void setName(String name) { this.name = name; }

    public Map<String, String> getMetadata() { return metadata; }
    public void setMetadata(Map<String, String> metadata) { this.metadata = metadata; }

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
