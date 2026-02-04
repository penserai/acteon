package com.acteon.client.models;

/**
 * Query parameters for listing events.
 */
public class EventQuery {
    private final String namespace;
    private final String tenant;
    private String status;
    private Integer limit;

    public EventQuery(String namespace, String tenant) {
        this.namespace = namespace;
        this.tenant = tenant;
    }

    public String getNamespace() {
        return namespace;
    }

    public String getTenant() {
        return tenant;
    }

    public String getStatus() {
        return status;
    }

    public EventQuery setStatus(String status) {
        this.status = status;
        return this;
    }

    public Integer getLimit() {
        return limit;
    }

    public EventQuery setLimit(Integer limit) {
        this.limit = limit;
        return this;
    }
}
