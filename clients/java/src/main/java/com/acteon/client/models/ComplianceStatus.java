package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * Current compliance configuration status.
 */
public class ComplianceStatus {
    private String mode;
    @JsonProperty("sync_audit_writes")
    private boolean syncAuditWrites;
    @JsonProperty("immutable_audit")
    private boolean immutableAudit;
    @JsonProperty("hash_chain")
    private boolean hashChain;

    public String getMode() { return mode; }
    public void setMode(String mode) { this.mode = mode; }

    public boolean isSyncAuditWrites() { return syncAuditWrites; }
    public void setSyncAuditWrites(boolean syncAuditWrites) { this.syncAuditWrites = syncAuditWrites; }

    public boolean isImmutableAudit() { return immutableAudit; }
    public void setImmutableAudit(boolean immutableAudit) { this.immutableAudit = immutableAudit; }

    public boolean isHashChain() { return hashChain; }
    public void setHashChain(boolean hashChain) { this.hashChain = hashChain; }
}
