package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * Result of verifying the integrity of an audit hash chain.
 */
public class HashChainVerification {
    private boolean valid;
    @JsonProperty("records_checked")
    private long recordsChecked;
    @JsonProperty("first_broken_at")
    private String firstBrokenAt;
    @JsonProperty("first_record_id")
    private String firstRecordId;
    @JsonProperty("last_record_id")
    private String lastRecordId;

    public boolean isValid() { return valid; }
    public void setValid(boolean valid) { this.valid = valid; }

    public long getRecordsChecked() { return recordsChecked; }
    public void setRecordsChecked(long recordsChecked) { this.recordsChecked = recordsChecked; }

    public String getFirstBrokenAt() { return firstBrokenAt; }
    public void setFirstBrokenAt(String firstBrokenAt) { this.firstBrokenAt = firstBrokenAt; }

    public String getFirstRecordId() { return firstRecordId; }
    public void setFirstRecordId(String firstRecordId) { this.firstRecordId = firstRecordId; }

    public String getLastRecordId() { return lastRecordId; }
    public void setLastRecordId(String lastRecordId) { this.lastRecordId = lastRecordId; }
}
