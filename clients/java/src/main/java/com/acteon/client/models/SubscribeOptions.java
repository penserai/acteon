package com.acteon.client.models;

/**
 * Options for the subscribe endpoint.
 */
public class SubscribeOptions {
    private Boolean includeHistory;

    public SubscribeOptions() {}

    public Boolean getIncludeHistory() { return includeHistory; }
    public void setIncludeHistory(Boolean includeHistory) { this.includeHistory = includeHistory; }

}
