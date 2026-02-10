package com.acteon.client.models;

/**
 * Response for DLQ stats endpoint.
 */
public class DlqStatsResponse {
    private boolean enabled;
    private int count;

    public boolean isEnabled() { return enabled; }
    public void setEnabled(boolean enabled) { this.enabled = enabled; }

    public int getCount() { return count; }
    public void setCount(int count) { this.count = count; }
}
