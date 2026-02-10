package com.acteon.client.models;

import java.util.List;

/**
 * Response from draining the dead-letter queue.
 */
public class DlqDrainResponse {
    private List<DlqEntry> entries;
    private int count;

    public List<DlqEntry> getEntries() { return entries; }
    public void setEntries(List<DlqEntry> entries) { this.entries = entries; }

    public int getCount() { return count; }
    public void setCount(int count) { this.count = count; }
}
