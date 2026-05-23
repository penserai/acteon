package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * Response from creating a recurring action.
 */
public class CreateRecurringResponse {
    @JsonProperty("id")
    private String id;

    @JsonProperty("status")
    private String status;

    @JsonProperty("name")
    private String name;

    @JsonProperty("next_execution_at")
    private String nextExecutionAt;

    public String getId() { return id; }
    public String getStatus() { return status; }
    public String getName() { return name; }
    public String getNextExecutionAt() { return nextExecutionAt; }
}
