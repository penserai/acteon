package com.acteon.client.models;

import java.util.HashMap;
import java.util.Map;

/**
 * Payload builder for the AWS EC2 detach-volume action ({@code aws-ec2}, action type {@code detach_volume}).
 */
public class Ec2DetachVolumePayload {
    private final String volumeId;
    private String instanceId;
    private String device;
    private Boolean force;

    public Ec2DetachVolumePayload(String volumeId) {
        this.volumeId = volumeId;
    }

    public Ec2DetachVolumePayload withInstanceId(String instanceId) {
        this.instanceId = instanceId;
        return this;
    }

    public Ec2DetachVolumePayload withDevice(String device) {
        this.device = device;
        return this;
    }

    public Ec2DetachVolumePayload withForce(boolean force) {
        this.force = force;
        return this;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        payload.put("volume_id", volumeId);
        if (instanceId != null) payload.put("instance_id", instanceId);
        if (device != null) payload.put("device", device);
        if (force != null) payload.put("force", force);
        return payload;
    }
}
