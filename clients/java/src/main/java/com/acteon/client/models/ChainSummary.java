package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * Summary of a chain execution for list responses.
 */
public class ChainSummary {
    @JsonProperty("chain_id")
    private String chainId;

    @JsonProperty("chain_name")
    private String chainName;

    private String status;

    @JsonProperty("current_step")
    private int currentStep;

    @JsonProperty("total_steps")
    private int totalSteps;

    @JsonProperty("started_at")
    private String startedAt;

    @JsonProperty("updated_at")
    private String updatedAt;

    @JsonProperty("parent_chain_id")
    private String parentChainId;

    public String getChainId() { return chainId; }
    public void setChainId(String chainId) { this.chainId = chainId; }

    public String getChainName() { return chainName; }
    public void setChainName(String chainName) { this.chainName = chainName; }

    public String getStatus() { return status; }
    public void setStatus(String status) { this.status = status; }

    public int getCurrentStep() { return currentStep; }
    public void setCurrentStep(int currentStep) { this.currentStep = currentStep; }

    public int getTotalSteps() { return totalSteps; }
    public void setTotalSteps(int totalSteps) { this.totalSteps = totalSteps; }

    public String getStartedAt() { return startedAt; }
    public void setStartedAt(String startedAt) { this.startedAt = startedAt; }

    public String getUpdatedAt() { return updatedAt; }
    public void setUpdatedAt(String updatedAt) { this.updatedAt = updatedAt; }

    public String getParentChainId() { return parentChainId; }
    public void setParentChainId(String parentChainId) { this.parentChainId = parentChainId; }
}
