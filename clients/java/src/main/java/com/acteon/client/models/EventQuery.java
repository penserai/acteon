package com.acteon.client.models;

/**
 * Query parameters for listing events.
 */
public class EventQuery {

    private String status;
    private Integer limit;

    public EventQuery() {

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
