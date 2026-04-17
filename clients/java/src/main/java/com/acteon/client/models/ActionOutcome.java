package com.acteon.client.models;

import java.time.Duration;

/**
 * Outcome of dispatching an action. Decoded from the Rust serde
 * adjacent-tagged enum shape via {@code ActionOutcomeDeserializer}.
 */
public class ActionOutcome {
    private OutcomeType type;
    private ProviderResponse response;
    private String rule;
    private String originalProvider;
    private String newProvider;
    private Duration retryAfter;
    private ActionError error;
    private String verdict;
    private String matchedRule;
    private String wouldBeProvider;
    private String actionId;
    private String scheduledFor;
    private String tenant;
    private long quotaLimit;
    private long quotaUsed;
    private String overageBehavior;

    public enum OutcomeType {
        EXECUTED, DEDUPLICATED, SUPPRESSED, REROUTED, THROTTLED, FAILED, DRY_RUN, SCHEDULED, QUOTA_EXCEEDED
    }

    // Getters and setters
    public OutcomeType getType() { return type; }
    public void setType(OutcomeType type) { this.type = type; }

    public ProviderResponse getResponse() { return response; }
    public void setResponse(ProviderResponse response) { this.response = response; }

    public String getRule() { return rule; }
    public void setRule(String rule) { this.rule = rule; }

    public String getOriginalProvider() { return originalProvider; }
    public void setOriginalProvider(String originalProvider) { this.originalProvider = originalProvider; }

    public String getNewProvider() { return newProvider; }
    public void setNewProvider(String newProvider) { this.newProvider = newProvider; }

    public Duration getRetryAfter() { return retryAfter; }
    public void setRetryAfter(Duration retryAfter) { this.retryAfter = retryAfter; }

    public ActionError getError() { return error; }
    public void setError(ActionError error) { this.error = error; }

    public String getVerdict() { return verdict; }
    public void setVerdict(String verdict) { this.verdict = verdict; }

    public String getMatchedRule() { return matchedRule; }
    public void setMatchedRule(String matchedRule) { this.matchedRule = matchedRule; }

    public String getWouldBeProvider() { return wouldBeProvider; }
    public void setWouldBeProvider(String wouldBeProvider) { this.wouldBeProvider = wouldBeProvider; }

    public String getActionId() { return actionId; }
    public void setActionId(String actionId) { this.actionId = actionId; }

    public String getScheduledFor() { return scheduledFor; }
    public void setScheduledFor(String scheduledFor) { this.scheduledFor = scheduledFor; }

    public boolean isExecuted() { return type == OutcomeType.EXECUTED; }
    public boolean isDeduplicated() { return type == OutcomeType.DEDUPLICATED; }
    public boolean isSuppressed() { return type == OutcomeType.SUPPRESSED; }
    public boolean isRerouted() { return type == OutcomeType.REROUTED; }
    public boolean isThrottled() { return type == OutcomeType.THROTTLED; }
    public boolean isFailed() { return type == OutcomeType.FAILED; }
    public boolean isDryRun() { return type == OutcomeType.DRY_RUN; }
    public boolean isScheduled() { return type == OutcomeType.SCHEDULED; }
    public boolean isQuotaExceeded() { return type == OutcomeType.QUOTA_EXCEEDED; }

    public String getTenant() { return tenant; }
    public void setTenant(String tenant) { this.tenant = tenant; }

    public long getQuotaLimit() { return quotaLimit; }
    public void setQuotaLimit(long quotaLimit) { this.quotaLimit = quotaLimit; }

    public long getQuotaUsed() { return quotaUsed; }
    public void setQuotaUsed(long quotaUsed) { this.quotaUsed = quotaUsed; }

    public String getOverageBehavior() { return overageBehavior; }
    public void setOverageBehavior(String overageBehavior) { this.overageBehavior = overageBehavior; }
}
