package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * An audit record.
 */
public class AuditRecord {
    private String id;
    @JsonProperty("action_id")
    private String actionId;
    private String namespace;
    private String tenant;
    private String provider;
    @JsonProperty("action_type")
    private String actionType;
    private String verdict;
    private String outcome;
    @JsonProperty("matched_rule")
    private String matchedRule;
    @JsonProperty("duration_ms")
    private long durationMs;
    @JsonProperty("dispatched_at")
    private String dispatchedAt;
    @JsonProperty("record_hash")
    private String recordHash;
    @JsonProperty("previous_hash")
    private String previousHash;
    @JsonProperty("sequence_number")
    private Long sequenceNumber;

    public String getId() { return id; }
    public void setId(String id) { this.id = id; }

    public String getActionId() { return actionId; }
    public void setActionId(String actionId) { this.actionId = actionId; }

    public String getNamespace() { return namespace; }
    public void setNamespace(String namespace) { this.namespace = namespace; }

    public String getTenant() { return tenant; }
    public void setTenant(String tenant) { this.tenant = tenant; }

    public String getProvider() { return provider; }
    public void setProvider(String provider) { this.provider = provider; }

    public String getActionType() { return actionType; }
    public void setActionType(String actionType) { this.actionType = actionType; }

    public String getVerdict() { return verdict; }
    public void setVerdict(String verdict) { this.verdict = verdict; }

    public String getOutcome() { return outcome; }
    public void setOutcome(String outcome) { this.outcome = outcome; }

    public String getMatchedRule() { return matchedRule; }
    public void setMatchedRule(String matchedRule) { this.matchedRule = matchedRule; }

    public long getDurationMs() { return durationMs; }
    public void setDurationMs(long durationMs) { this.durationMs = durationMs; }

    public String getDispatchedAt() { return dispatchedAt; }
    public void setDispatchedAt(String dispatchedAt) { this.dispatchedAt = dispatchedAt; }

    public String getRecordHash() { return recordHash; }
    public void setRecordHash(String recordHash) { this.recordHash = recordHash; }

    public String getPreviousHash() { return previousHash; }
    public void setPreviousHash(String previousHash) { this.previousHash = previousHash; }

    public Long getSequenceNumber() { return sequenceNumber; }
    public void setSequenceNumber(Long sequenceNumber) { this.sequenceNumber = sequenceNumber; }
}
