package com.acteon.client.models;

/**
 * Query parameters for listing recurring actions.
 */
public class RecurringFilter {

    private String status;
    private Integer limit;
    private Integer offset;

    public RecurringFilter() {}

    public String getStatus() { return status; }
    public void setStatus(String status) { this.status = status; }

    public Integer getLimit() { return limit; }
    public void setLimit(Integer limit) { this.limit = limit; }

    public Integer getOffset() { return offset; }
    public void setOffset(Integer offset) { this.offset = offset; }
}
