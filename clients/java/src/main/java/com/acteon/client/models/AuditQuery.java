package com.acteon.client.models;

/**
 * Query parameters for audit search.
 */
public class AuditQuery {
    private String namespace;
    private String tenant;
    private String provider;
    private String actionType;
    private String outcome;
    private Integer limit;
    private Integer offset;

    public static Builder builder() {
        return new Builder();
    }

    public String getNamespace() { return namespace; }
    public void setNamespace(String namespace) { this.namespace = namespace; }

    public String getTenant() { return tenant; }
    public void setTenant(String tenant) { this.tenant = tenant; }

    public String getProvider() { return provider; }
    public void setProvider(String provider) { this.provider = provider; }

    public String getActionType() { return actionType; }
    public void setActionType(String actionType) { this.actionType = actionType; }

    public String getOutcome() { return outcome; }
    public void setOutcome(String outcome) { this.outcome = outcome; }

    public Integer getLimit() { return limit; }
    public void setLimit(Integer limit) { this.limit = limit; }

    public Integer getOffset() { return offset; }
    public void setOffset(Integer offset) { this.offset = offset; }

    public static class Builder {
        private String namespace;
        private String tenant;
        private String provider;
        private String actionType;
        private String outcome;
        private Integer limit;
        private Integer offset;

        public Builder namespace(String namespace) { this.namespace = namespace; return this; }
        public Builder tenant(String tenant) { this.tenant = tenant; return this; }
        public Builder provider(String provider) { this.provider = provider; return this; }
        public Builder actionType(String actionType) { this.actionType = actionType; return this; }
        public Builder outcome(String outcome) { this.outcome = outcome; return this; }
        public Builder limit(int limit) { this.limit = limit; return this; }
        public Builder offset(int offset) { this.offset = offset; return this; }

        public AuditQuery build() {
            AuditQuery query = new AuditQuery();
            query.namespace = this.namespace;
            query.tenant = this.tenant;
            query.provider = this.provider;
            query.actionType = this.actionType;
            query.outcome = this.outcome;
            query.limit = this.limit;
            query.offset = this.offset;
            return query;
        }
    }
}
