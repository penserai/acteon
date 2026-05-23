package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * A single dead-letter queue entry.
 */
public class DlqEntry {
    @JsonProperty("action_id")
    private String actionId;

    private String namespace;
    private String tenant;
    private String provider;

    @JsonProperty("action_type")
    private String actionType;

    private String error;
    private int attempts;
    private long timestamp;

    public String getActionId() { return actionId; }
    public void setActionId(String actionId) { this.actionId = actionId; }

    public String getNamespace() { return namespace; }
    public void setNamespace(String namespace) { this.namespace = namespace; }

    public String getTenant() { return tenant; }
    public void setTenant(String tenant) { this.tenant = tenant; }

    public String getProvider() { return provider; }
    public void setProvider(String provider) { this.provider = provider; }

    public String getActionType() { return actionType; }
    public void setActionType(String actionType) { this.actionType = actionType; }

    public String getError() { return error; }
    public void setError(String error) { this.error = error; }

    public int getAttempts() { return attempts; }
    public void setAttempts(int attempts) { this.attempts = attempts; }

    public long getTimestamp() { return timestamp; }
    public void setTimestamp(long timestamp) { this.timestamp = timestamp; }
}
