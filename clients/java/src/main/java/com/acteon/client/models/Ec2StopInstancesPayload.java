package com.acteon.client.models;

import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * Payload builder for the AWS EC2 stop-instances action ({@code aws-ec2}, action type {@code stop_instances}).
 */
public class Ec2StopInstancesPayload {
    private final List<String> instanceIds;
    private Boolean hibernate;
    private Boolean force;

    public Ec2StopInstancesPayload(List<String> instanceIds) {
        this.instanceIds = instanceIds;
    }

    public Ec2StopInstancesPayload withHibernate(boolean hibernate) {
        this.hibernate = hibernate;
        return this;
    }

    public Ec2StopInstancesPayload withForce(boolean force) {
        this.force = force;
        return this;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        payload.put("instance_ids", instanceIds);
        if (hibernate != null) payload.put("hibernate", hibernate);
        if (force != null) payload.put("force", force);
        return payload;
    }
}
