package com.acteon.client.models;

import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * Payload builder for the AWS EC2 describe-instances action ({@code aws-ec2}, action type {@code describe_instances}).
 */
public class Ec2DescribeInstancesPayload {
    private List<String> instanceIds;

    public Ec2DescribeInstancesPayload() {
    }

    public Ec2DescribeInstancesPayload withInstanceIds(List<String> instanceIds) {
        this.instanceIds = instanceIds;
        return this;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        if (instanceIds != null) payload.put("instance_ids", instanceIds);
        return payload;
    }
}
