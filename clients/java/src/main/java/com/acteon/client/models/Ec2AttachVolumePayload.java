package com.acteon.client.models;

import java.util.HashMap;
import java.util.Map;

/**
 * Payload builder for the AWS EC2 attach-volume action ({@code aws-ec2}, action type {@code attach_volume}).
 */
public class Ec2AttachVolumePayload {
    private final String volumeId;
    private final String instanceId;
    private final String device;

    public Ec2AttachVolumePayload(String volumeId, String instanceId, String device) {
        this.volumeId = volumeId;
        this.instanceId = instanceId;
        this.device = device;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        payload.put("volume_id", volumeId);
        payload.put("instance_id", instanceId);
        payload.put("device", device);
        return payload;
    }
}
