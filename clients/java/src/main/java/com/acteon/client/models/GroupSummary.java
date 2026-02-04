package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * Summary of an event group.
 */
public class GroupSummary {
    @JsonProperty("group_id")
    private String groupId;

    @JsonProperty("group_key")
    private String groupKey;

    @JsonProperty("event_count")
    private int eventCount;

    private String state;

    @JsonProperty("notify_at")
    private String notifyAt;

    @JsonProperty("created_at")
    private String createdAt;

    public String getGroupId() {
        return groupId;
    }

    public void setGroupId(String groupId) {
        this.groupId = groupId;
    }

    public String getGroupKey() {
        return groupKey;
    }

    public void setGroupKey(String groupKey) {
        this.groupKey = groupKey;
    }

    public int getEventCount() {
        return eventCount;
    }

    public void setEventCount(int eventCount) {
        this.eventCount = eventCount;
    }

    public String getState() {
        return state;
    }

    public void setState(String state) {
        this.state = state;
    }

    public String getNotifyAt() {
        return notifyAt;
    }

    public void setNotifyAt(String notifyAt) {
        this.notifyAt = notifyAt;
    }

    public String getCreatedAt() {
        return createdAt;
    }

    public void setCreatedAt(String createdAt) {
        this.createdAt = createdAt;
    }
}
