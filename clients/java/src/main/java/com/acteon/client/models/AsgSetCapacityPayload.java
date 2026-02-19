package com.acteon.client.models;

import java.util.HashMap;
import java.util.Map;

/**
 * Payload builder for the AWS Auto Scaling set-desired-capacity action
 * ({@code aws-autoscaling}, action type {@code set_desired_capacity}).
 */
public class AsgSetCapacityPayload {
    private final String groupName;
    private final int desiredCapacity;
    private Boolean honorCooldown;

    public AsgSetCapacityPayload(String groupName, int desiredCapacity) {
        this.groupName = groupName;
        this.desiredCapacity = desiredCapacity;
    }

    public AsgSetCapacityPayload withHonorCooldown(boolean honorCooldown) {
        this.honorCooldown = honorCooldown;
        return this;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        payload.put("auto_scaling_group_name", groupName);
        payload.put("desired_capacity", desiredCapacity);
        if (honorCooldown != null) payload.put("honor_cooldown", honorCooldown);
        return payload;
    }
}
