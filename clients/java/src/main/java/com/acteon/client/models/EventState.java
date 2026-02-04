package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * Current state of an event.
 */
public class EventState {
    private String fingerprint;
    private String state;

    @JsonProperty("action_type")
    private String actionType;

    @JsonProperty("updated_at")
    private String updatedAt;

    public String getFingerprint() {
        return fingerprint;
    }

    public void setFingerprint(String fingerprint) {
        this.fingerprint = fingerprint;
    }

    public String getState() {
        return state;
    }

    public void setState(String state) {
        this.state = state;
    }

    public String getActionType() {
        return actionType;
    }

    public void setActionType(String actionType) {
        this.actionType = actionType;
    }

    public String getUpdatedAt() {
        return updatedAt;
    }

    public void setUpdatedAt(String updatedAt) {
        this.updatedAt = updatedAt;
    }
}
