package com.acteon.client.models;

/**
 * Options for a rule coverage analysis.
 */
public class CoverageQuery {
    private String namespace;
    private String tenant;
    private String from;  // RFC 3339 timestamp
    private String to;    // RFC 3339 timestamp

    public String getNamespace() { return namespace; }
    public void setNamespace(String namespace) { this.namespace = namespace; }

    public String getTenant() { return tenant; }
    public void setTenant(String tenant) { this.tenant = tenant; }

    public String getFrom() { return from; }
    public void setFrom(String from) { this.from = from; }

    public String getTo() { return to; }
    public void setTo(String to) { this.to = to; }
}
