package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * Response from flushing a group.
 */
public class FlushGroupResponse {
    @JsonProperty("group_id")
    private String groupId;

    @JsonProperty("event_count")
    private int eventCount;

    private boolean notified;

    public String getGroupId() {
        return groupId;
    }

    public void setGroupId(String groupId) {
        this.groupId = groupId;
    }

    public int getEventCount() {
        return eventCount;
    }

    public void setEventCount(int eventCount) {
        this.eventCount = eventCount;
    }

    public boolean isNotified() {
        return notified;
    }

    public void setNotified(boolean notified) {
        this.notified = notified;
    }
}
