package com.acteon.client.models;

import java.util.Map;

/**
 * Response from approving or rejecting an action.
 */
public class ApprovalActionResponse {
    private String id;
    private String status;
    private Map<String, Object> outcome;

    public String getId() {
        return id;
    }

    public void setId(String id) {
        this.id = id;
    }

    public String getStatus() {
        return status;
    }

    public void setStatus(String status) {
        this.status = status;
    }

    public Map<String, Object> getOutcome() {
        return outcome;
    }

    public void setOutcome(Map<String, Object> outcome) {
        this.outcome = outcome;
    }
}
