package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;

/**
 * Retry history for a chain execution.
 */
public class ChainHistoryResponse {
    @JsonProperty("chain_id")
    private String chainId;

    @JsonProperty("chain_name")
    private String chainName;

    private String status;

    private List<StepHistoryEntry> steps;

    public String getChainId() { return chainId; }
    public void setChainId(String chainId) { this.chainId = chainId; }

    public String getChainName() { return chainName; }
    public void setChainName(String chainName) { this.chainName = chainName; }

    public String getStatus() { return status; }
    public void setStatus(String status) { this.status = status; }

    public List<StepHistoryEntry> getSteps() { return steps; }
    public void setSteps(List<StepHistoryEntry> steps) { this.steps = steps; }
}
