package com.acteon.client.models;

import java.util.List;

/**
 * Response from listing pending approvals.
 */
public class ApprovalListResponse {
    private List<ApprovalStatus> approvals;
    private int count;

    public List<ApprovalStatus> getApprovals() {
        return approvals;
    }

    public void setApprovals(List<ApprovalStatus> approvals) {
        this.approvals = approvals;
    }

    public int getCount() {
        return count;
    }

    public void setCount(int count) {
        this.count = count;
    }
}
