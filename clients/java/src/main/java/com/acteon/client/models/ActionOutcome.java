package com.acteon.client.models;

import java.time.Duration;

/**
 * Outcome of dispatching an action. Decoded from the Rust serde
 * adjacent-tagged enum shape via {@code ActionOutcomeDeserializer}.
 */
public class ActionOutcome {
    private OutcomeType type;
    private ProviderWorkPending pending;
    public ProviderWorkPending getPending() { return pending; }
    public void setPending(ProviderWorkPending value) { pending = value; }
    public boolean isProviderPending() { return type == OutcomeType.PROVIDER_PENDING; }
    private String groupId;
    private long groupSize;
    private String notifyAt;
    private String fingerprint;
    private String previousState;
    private String newState;
    private boolean notify;
    private String approvalId;
    private String expiresAt;
    private String approveUrl;
    private String rejectUrl;
    private boolean notificationSent;
    private String chainId;
    private String chainName;
    private long totalSteps;
    private String firstStep;
    private String provider;
    private java.util.List<String> fallbackChain;
    private String recurringId;
    private String cronExpr;
    private String nextExecutionAt;
    private String silenceId;
    private String interval;
    private String reason;

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
        PROVIDER_PENDING, GROUPED, STATE_CHANGED, PENDING_APPROVAL, CHAIN_STARTED, CIRCUIT_OPEN, RECURRING_CREATED, SILENCED, MUTED, EXECUTED, DEDUPLICATED, SUPPRESSED, REROUTED, THROTTLED, FAILED, DRY_RUN, SCHEDULED, QUOTA_EXCEEDED
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
    public String getGroupId() { return groupId; }
    public void setGroupId(String value) { this.groupId = value; }
    public long getGroupSize() { return groupSize; }
    public void setGroupSize(long value) { this.groupSize = value; }
    public String getNotifyAt() { return notifyAt; }
    public void setNotifyAt(String value) { this.notifyAt = value; }
    public String getFingerprint() { return fingerprint; }
    public void setFingerprint(String value) { this.fingerprint = value; }
    public String getPreviousState() { return previousState; }
    public void setPreviousState(String value) { this.previousState = value; }
    public String getNewState() { return newState; }
    public void setNewState(String value) { this.newState = value; }
    public boolean getNotify() { return notify; }
    public void setNotify(boolean value) { this.notify = value; }
    public String getApprovalId() { return approvalId; }
    public void setApprovalId(String value) { this.approvalId = value; }
    public String getExpiresAt() { return expiresAt; }
    public void setExpiresAt(String value) { this.expiresAt = value; }
    public String getApproveUrl() { return approveUrl; }
    public void setApproveUrl(String value) { this.approveUrl = value; }
    public String getRejectUrl() { return rejectUrl; }
    public void setRejectUrl(String value) { this.rejectUrl = value; }
    public boolean getNotificationSent() { return notificationSent; }
    public void setNotificationSent(boolean value) { this.notificationSent = value; }
    public String getChainId() { return chainId; }
    public void setChainId(String value) { this.chainId = value; }
    public String getChainName() { return chainName; }
    public void setChainName(String value) { this.chainName = value; }
    public long getTotalSteps() { return totalSteps; }
    public void setTotalSteps(long value) { this.totalSteps = value; }
    public String getFirstStep() { return firstStep; }
    public void setFirstStep(String value) { this.firstStep = value; }
    public String getProvider() { return provider; }
    public void setProvider(String value) { this.provider = value; }
    public java.util.List<String> getFallbackChain() { return fallbackChain; }
    public void setFallbackChain(java.util.List<String> value) { this.fallbackChain = value; }
    public String getRecurringId() { return recurringId; }
    public void setRecurringId(String value) { this.recurringId = value; }
    public String getCronExpr() { return cronExpr; }
    public void setCronExpr(String value) { this.cronExpr = value; }
    public String getNextExecutionAt() { return nextExecutionAt; }
    public void setNextExecutionAt(String value) { this.nextExecutionAt = value; }
    public String getSilenceId() { return silenceId; }
    public void setSilenceId(String value) { this.silenceId = value; }
    public String getInterval() { return interval; }
    public void setInterval(String value) { this.interval = value; }
    public String getReason() { return reason; }
    public void setReason(String value) { this.reason = value; }
    public boolean isGrouped() { return type == OutcomeType.GROUPED; }
    public boolean isStateChanged() { return type == OutcomeType.STATE_CHANGED; }
    public boolean isPendingApproval() { return type == OutcomeType.PENDING_APPROVAL; }
    public boolean isChainStarted() { return type == OutcomeType.CHAIN_STARTED; }
    public boolean isCircuitOpen() { return type == OutcomeType.CIRCUIT_OPEN; }
    public boolean isRecurringCreated() { return type == OutcomeType.RECURRING_CREATED; }
    public boolean isSilenced() { return type == OutcomeType.SILENCED; }
    public boolean isMuted() { return type == OutcomeType.MUTED; }
}
