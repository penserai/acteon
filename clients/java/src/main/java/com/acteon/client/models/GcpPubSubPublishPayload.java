package com.acteon.client.models;

import java.util.HashMap;
import java.util.Map;

/**
 * Payload builder for the GCP Pub/Sub publish action ({@code gcp-pubsub}, action type {@code publish}).
 */
public class GcpPubSubPublishPayload {
    private final String data;
    private String dataBase64;
    private Map<String, String> attributes;
    private String orderingKey;
    private String topic;

    public GcpPubSubPublishPayload(String data) {
        this.data = data;
    }

    public GcpPubSubPublishPayload withDataBase64(String dataBase64) {
        this.dataBase64 = dataBase64;
        return this;
    }

    public GcpPubSubPublishPayload withAttributes(Map<String, String> attributes) {
        this.attributes = attributes;
        return this;
    }

    public GcpPubSubPublishPayload withOrderingKey(String orderingKey) {
        this.orderingKey = orderingKey;
        return this;
    }

    public GcpPubSubPublishPayload withTopic(String topic) {
        this.topic = topic;
        return this;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        payload.put("data", data);
        if (dataBase64 != null) payload.put("data_base64", dataBase64);
        if (attributes != null) payload.put("attributes", attributes);
        if (orderingKey != null) payload.put("ordering_key", orderingKey);
        if (topic != null) payload.put("topic", topic);
        return payload;
    }
}
