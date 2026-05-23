package com.acteon.client.models;

/**
 * Options for the subscribe endpoint.
 */
public class SubscribeOptions {
    private Boolean includeHistory;
    private String namespace;
    private String tenant;

    public SubscribeOptions() {}

    public SubscribeOptions(String namespace, String tenant) {
        this.namespace = namespace;
        this.tenant = tenant;
    }

    public Boolean getIncludeHistory() { return includeHistory; }
    public void setIncludeHistory(Boolean includeHistory) { this.includeHistory = includeHistory; }

    public String getNamespace() { return namespace; }
    public void setNamespace(String namespace) { this.namespace = namespace; }

    public String getTenant() { return tenant; }
    public void setTenant(String tenant) { this.tenant = tenant; }
}
