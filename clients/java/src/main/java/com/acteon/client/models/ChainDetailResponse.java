package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;

/**
 * Full detail response for a chain execution.
 */
public class ChainDetailResponse {
    @JsonProperty("chain_id")
    private String chainId;

    @JsonProperty("chain_name")
    private String chainName;

    private String status;

    @JsonProperty("current_step")
    private int currentStep;

    @JsonProperty("total_steps")
    private int totalSteps;

    private List<ChainStepStatus> steps;

    @JsonProperty("started_at")
    private String startedAt;

    @JsonProperty("updated_at")
    private String updatedAt;

    @JsonProperty("expires_at")
    private String expiresAt;

    @JsonProperty("cancel_reason")
    private String cancelReason;

    @JsonProperty("cancelled_by")
    private String cancelledBy;

    @JsonProperty("execution_path")
    private List<String> executionPath;

    @JsonProperty("parent_chain_id")
    private String parentChainId;

    @JsonProperty("child_chain_ids")
    private List<String> childChainIds;

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

    public List<ChainStepStatus> getSteps() { return steps; }
    public void setSteps(List<ChainStepStatus> steps) { this.steps = steps; }

    public String getStartedAt() { return startedAt; }
    public void setStartedAt(String startedAt) { this.startedAt = startedAt; }

    public String getUpdatedAt() { return updatedAt; }
    public void setUpdatedAt(String updatedAt) { this.updatedAt = updatedAt; }

    public String getExpiresAt() { return expiresAt; }
    public void setExpiresAt(String expiresAt) { this.expiresAt = expiresAt; }

    public String getCancelReason() { return cancelReason; }
    public void setCancelReason(String cancelReason) { this.cancelReason = cancelReason; }

    public String getCancelledBy() { return cancelledBy; }
    public void setCancelledBy(String cancelledBy) { this.cancelledBy = cancelledBy; }

    public List<String> getExecutionPath() { return executionPath; }
    public void setExecutionPath(List<String> executionPath) { this.executionPath = executionPath; }

    public String getParentChainId() { return parentChainId; }
    public void setParentChainId(String parentChainId) { this.parentChainId = parentChainId; }

    public List<String> getChildChainIds() { return childChainIds; }
    public void setChildChainIds(List<String> childChainIds) { this.childChainIds = childChainIds; }
}
