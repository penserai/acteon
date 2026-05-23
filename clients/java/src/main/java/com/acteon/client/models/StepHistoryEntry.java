package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;

/**
 * Retry history for a single chain step.
 */
public class StepHistoryEntry {
    private String name;

    @JsonProperty("step_index")
    private int stepIndex;

    @JsonProperty("current_attempt")
    private int currentAttempt;

    @JsonProperty("max_retries")
    private int maxRetries;

    private List<StepAttemptResponse> attempts;

    public String getName() { return name; }
    public void setName(String name) { this.name = name; }

    public int getStepIndex() { return stepIndex; }
    public void setStepIndex(int stepIndex) { this.stepIndex = stepIndex; }

    public int getCurrentAttempt() { return currentAttempt; }
    public void setCurrentAttempt(int currentAttempt) { this.currentAttempt = currentAttempt; }

    public int getMaxRetries() { return maxRetries; }
    public void setMaxRetries(int maxRetries) { this.maxRetries = maxRetries; }

    public List<StepAttemptResponse> getAttempts() { return attempts; }
    public void setAttempts(List<StepAttemptResponse> attempts) { this.attempts = attempts; }
}
