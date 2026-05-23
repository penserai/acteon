package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;

/**
 * Full rule coverage report.
 */
public class CoverageReport {
    @JsonProperty("scanned_from")
    private String scannedFrom;
    @JsonProperty("scanned_to")
    private String scannedTo;
    @JsonProperty("total_actions")
    private long totalActions;
    @JsonProperty("unique_combinations")
    private int uniqueCombinations;
    @JsonProperty("fully_covered")
    private int fullyCovered;
    @JsonProperty("partially_covered")
    private int partiallyCovered;
    private int uncovered;
    @JsonProperty("rules_loaded")
    private int rulesLoaded;
    private List<CoverageEntry> entries;
    @JsonProperty("unmatched_rules")
    private List<String> unmatchedRules;

    public String getScannedFrom() { return scannedFrom; }
    public void setScannedFrom(String scannedFrom) { this.scannedFrom = scannedFrom; }

    public String getScannedTo() { return scannedTo; }
    public void setScannedTo(String scannedTo) { this.scannedTo = scannedTo; }

    public long getTotalActions() { return totalActions; }
    public void setTotalActions(long totalActions) { this.totalActions = totalActions; }

    public int getUniqueCombinations() { return uniqueCombinations; }
    public void setUniqueCombinations(int uniqueCombinations) { this.uniqueCombinations = uniqueCombinations; }

    public int getFullyCovered() { return fullyCovered; }
    public void setFullyCovered(int fullyCovered) { this.fullyCovered = fullyCovered; }

    public int getPartiallyCovered() { return partiallyCovered; }
    public void setPartiallyCovered(int partiallyCovered) { this.partiallyCovered = partiallyCovered; }

    public int getUncovered() { return uncovered; }
    public void setUncovered(int uncovered) { this.uncovered = uncovered; }

    public int getRulesLoaded() { return rulesLoaded; }
    public void setRulesLoaded(int rulesLoaded) { this.rulesLoaded = rulesLoaded; }

    public List<CoverageEntry> getEntries() { return entries; }
    public void setEntries(List<CoverageEntry> entries) { this.entries = entries; }

    public List<String> getUnmatchedRules() { return unmatchedRules; }
    public void setUnmatchedRules(List<String> unmatchedRules) { this.unmatchedRules = unmatchedRules; }
}
