package com.acteon.client.models;

import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * Payload builder for the AWS Auto Scaling describe-groups action
 * ({@code aws-autoscaling}, action type {@code describe_auto_scaling_groups}).
 */
public class AsgDescribeGroupsPayload {
    private List<String> groupNames;

    public AsgDescribeGroupsPayload() {
    }

    public AsgDescribeGroupsPayload withGroupNames(List<String> groupNames) {
        this.groupNames = groupNames;
        return this;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        if (groupNames != null) payload.put("auto_scaling_group_names", groupNames);
        return payload;
    }
}
