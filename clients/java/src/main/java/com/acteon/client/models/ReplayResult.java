package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * Result of replaying a single action from the audit trail.
 */
public class ReplayResult {
    @JsonProperty("original_action_id")
    private String originalActionId;

    @JsonProperty("new_action_id")
    private String newActionId;

    @JsonProperty("success")
    private boolean success;

    @JsonProperty("error")
    private String error;

    public String getOriginalActionId() { return originalActionId; }
    public String getNewActionId() { return newActionId; }
    public boolean isSuccess() { return success; }
    public String getError() { return error; }
}
