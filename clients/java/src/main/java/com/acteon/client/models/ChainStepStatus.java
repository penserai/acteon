package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.Map;

/**
 * Detailed status of a single chain step.
 */
public class ChainStepStatus {
    private String name;
    private String provider;
    private String status;

    @JsonProperty("response_body")
    private Object responseBody;

    private String error;

    @JsonProperty("completed_at")
    private String completedAt;

    public String getName() { return name; }
    public void setName(String name) { this.name = name; }

    public String getProvider() { return provider; }
    public void setProvider(String provider) { this.provider = provider; }

    public String getStatus() { return status; }
    public void setStatus(String status) { this.status = status; }

    public Object getResponseBody() { return responseBody; }
    public void setResponseBody(Object responseBody) { this.responseBody = responseBody; }

    public String getError() { return error; }
    public void setError(String error) { this.error = error; }

    public String getCompletedAt() { return completedAt; }
    public void setCompletedAt(String completedAt) { this.completedAt = completedAt; }
}
