package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;

/**
 * Response from listing retention policies.
 */
public class ListRetentionResponse {
    @JsonProperty("policies")
    private List<RetentionPolicy> policies;

    @JsonProperty("count")
    private int count;

    public List<RetentionPolicy> getPolicies() { return policies; }
    public int getCount() { return count; }
}
