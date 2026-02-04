package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * Response from transitioning an event.
 */
public class TransitionResponse {
    private String fingerprint;

    @JsonProperty("previous_state")
    private String previousState;

    @JsonProperty("new_state")
    private String newState;

    private boolean notify;

    public String getFingerprint() {
        return fingerprint;
    }

    public void setFingerprint(String fingerprint) {
        this.fingerprint = fingerprint;
    }

    public String getPreviousState() {
        return previousState;
    }

    public void setPreviousState(String previousState) {
        this.previousState = previousState;
    }

    public String getNewState() {
        return newState;
    }

    public void setNewState(String newState) {
        this.newState = newState;
    }

    public boolean isNotify() {
        return notify;
    }

    public void setNotify(boolean notify) {
        this.notify = notify;
    }
}
