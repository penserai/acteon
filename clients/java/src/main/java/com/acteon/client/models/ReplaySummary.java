package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;
import java.util.List;

/**
 * Summary of a bulk replay operation.
 */
public class ReplaySummary {
    @JsonProperty("replayed")
    private int replayed;

    @JsonProperty("failed")
    private int failed;

    @JsonProperty("skipped")
    private int skipped;

    @JsonProperty("results")
    private List<ReplayResult> results;

    public int getReplayed() { return replayed; }
    public int getFailed() { return failed; }
    public int getSkipped() { return skipped; }
    public List<ReplayResult> getResults() { return results; }
}
