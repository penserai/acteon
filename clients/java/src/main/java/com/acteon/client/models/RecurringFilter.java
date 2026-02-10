package com.acteon.client.models;

/**
 * Query parameters for listing recurring actions.
 */
public class RecurringFilter {
    private String namespace;
    private String tenant;
    private String status;
    private Integer limit;
    private Integer offset;

    public RecurringFilter() {}

    public String getNamespace() { return namespace; }
    public void setNamespace(String namespace) { this.namespace = namespace; }

    public String getTenant() { return tenant; }
    public void setTenant(String tenant) { this.tenant = tenant; }

    public String getStatus() { return status; }
    public void setStatus(String status) { this.status = status; }

    public Integer getLimit() { return limit; }
    public void setLimit(Integer limit) { this.limit = limit; }

    public Integer getOffset() { return offset; }
    public void setOffset(Integer offset) { this.offset = offset; }
}
