package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;

/**
 * Response from listing swarm runs.
 */
public class ListSwarmRunsResponse {
    @JsonProperty("runs")
    private List<SwarmRunSnapshot> runs;

    @JsonProperty("total")
    private int total;

    public List<SwarmRunSnapshot> getRuns() { return runs; }
    public int getTotal() { return total; }
}
