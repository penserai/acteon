package com.acteon.client.models;

import java.util.List;
import java.util.Map;

/**
 * Detailed information about a group.
 */
public class GroupDetail {
    private GroupSummary group;
    private List<String> events;
    private Map<String, String> labels;

    public GroupSummary getGroup() {
        return group;
    }

    public void setGroup(GroupSummary group) {
        this.group = group;
    }

    public List<String> getEvents() {
        return events;
    }

    public void setEvents(List<String> events) {
        this.events = events;
    }

    public Map<String, String> getLabels() {
        return labels;
    }

    public void setLabels(Map<String, String> labels) {
        this.labels = labels;
    }
}
