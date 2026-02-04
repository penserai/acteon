package com.acteon.client.models;

import java.util.List;

/**
 * Response from listing groups.
 */
public class GroupListResponse {
    private List<GroupSummary> groups;
    private int total;

    public List<GroupSummary> getGroups() {
        return groups;
    }

    public void setGroups(List<GroupSummary> groups) {
        this.groups = groups;
    }

    public int getTotal() {
        return total;
    }

    public void setTotal(int total) {
        this.total = total;
    }
}
