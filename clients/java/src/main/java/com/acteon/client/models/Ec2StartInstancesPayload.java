package com.acteon.client.models;

import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * Payload builder for the AWS EC2 start-instances action ({@code aws-ec2}, action type {@code start_instances}).
 */
public class Ec2StartInstancesPayload {
    private final List<String> instanceIds;

    public Ec2StartInstancesPayload(List<String> instanceIds) {
        this.instanceIds = instanceIds;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        payload.put("instance_ids", instanceIds);
        return payload;
    }
}
