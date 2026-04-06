package com.acteon.client.models;

/**
 * Options for a rule coverage analysis.
 */
public class CoverageQuery {
    private int limit = 5000;
    private String namespace;
    private String tenant;
    private int pageSize = 500;

    public int getLimit() { return limit; }
    public void setLimit(int limit) { this.limit = limit; }

    public String getNamespace() { return namespace; }
    public void setNamespace(String namespace) { this.namespace = namespace; }

    public String getTenant() { return tenant; }
    public void setTenant(String tenant) { this.tenant = tenant; }

    public int getPageSize() { return pageSize; }
    public void setPageSize(int pageSize) { this.pageSize = pageSize; }
}
