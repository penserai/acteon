package com.acteon.client.models;

import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * Payload builder for the GCP Pub/Sub publish-batch action ({@code gcp-pubsub}, action type {@code publish_batch}).
 */
public class GcpPubSubPublishBatchPayload {
    private final List<Map<String, Object>> messages;
    private String topic;

    public GcpPubSubPublishBatchPayload(List<Map<String, Object>> messages) {
        this.messages = messages;
    }

    public GcpPubSubPublishBatchPayload withTopic(String topic) {
        this.topic = topic;
        return this;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        payload.put("messages", messages);
        if (topic != null) payload.put("topic", topic);
        return payload;
    }
}
