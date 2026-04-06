package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;

/**
 * Per-combination coverage statistics.
 */
public class CoverageEntry {
    private CoverageKey key;
    private long total;
    private long covered;
    private long uncovered;
    @JsonProperty("matched_rules")
    private List<String> matchedRules;

    public CoverageKey getKey() { return key; }
    public void setKey(CoverageKey key) { this.key = key; }

    public long getTotal() { return total; }
    public void setTotal(long total) { this.total = total; }

    public long getCovered() { return covered; }
    public void setCovered(long covered) { this.covered = covered; }

    public long getUncovered() { return uncovered; }
    public void setUncovered(long uncovered) { this.uncovered = uncovered; }

    public List<String> getMatchedRules() { return matchedRules; }
    public void setMatchedRules(List<String> matchedRules) { this.matchedRules = matchedRules; }
}
