package com.acteon.client.models;

import java.util.HashMap;
import java.util.Map;

/**
 * Payload builder for the AWS Auto Scaling update-group action
 * ({@code aws-autoscaling}, action type {@code update_auto_scaling_group}).
 */
public class AsgUpdateGroupPayload {
    private final String groupName;
    private Integer minSize;
    private Integer maxSize;
    private Integer desiredCapacity;
    private Integer defaultCooldown;
    private String healthCheckType;
    private Integer healthCheckGracePeriod;

    public AsgUpdateGroupPayload(String groupName) {
        this.groupName = groupName;
    }

    public AsgUpdateGroupPayload withMinSize(Integer minSize) {
        this.minSize = minSize;
        return this;
    }

    public AsgUpdateGroupPayload withMaxSize(Integer maxSize) {
        this.maxSize = maxSize;
        return this;
    }

    public AsgUpdateGroupPayload withDesiredCapacity(Integer desiredCapacity) {
        this.desiredCapacity = desiredCapacity;
        return this;
    }

    public AsgUpdateGroupPayload withDefaultCooldown(Integer defaultCooldown) {
        this.defaultCooldown = defaultCooldown;
        return this;
    }

    public AsgUpdateGroupPayload withHealthCheckType(String healthCheckType) {
        this.healthCheckType = healthCheckType;
        return this;
    }

    public AsgUpdateGroupPayload withHealthCheckGracePeriod(Integer healthCheckGracePeriod) {
        this.healthCheckGracePeriod = healthCheckGracePeriod;
        return this;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        payload.put("auto_scaling_group_name", groupName);
        if (minSize != null) payload.put("min_size", minSize);
        if (maxSize != null) payload.put("max_size", maxSize);
        if (desiredCapacity != null) payload.put("desired_capacity", desiredCapacity);
        if (defaultCooldown != null) payload.put("default_cooldown", defaultCooldown);
        if (healthCheckType != null) payload.put("health_check_type", healthCheckType);
        if (healthCheckGracePeriod != null) payload.put("health_check_grace_period", healthCheckGracePeriod);
        return payload;
    }
}
