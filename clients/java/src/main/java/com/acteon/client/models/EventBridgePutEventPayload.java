package com.acteon.client.models;

import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * Payload builder for the AWS EventBridge provider ({@code aws-eventbridge}, action type {@code put_event}).
 */
public class EventBridgePutEventPayload {
    private final String source;
    private final String detailType;
    private final Object detail;
    private String eventBusName;
    private List<String> resources;

    public EventBridgePutEventPayload(String source, String detailType, Object detail) {
        this.source = source;
        this.detailType = detailType;
        this.detail = detail;
    }

    public EventBridgePutEventPayload withEventBusName(String eventBusName) {
        this.eventBusName = eventBusName;
        return this;
    }

    public EventBridgePutEventPayload withResources(List<String> resources) {
        this.resources = resources;
        return this;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        payload.put("source", source);
        payload.put("detail_type", detailType);
        payload.put("detail", detail);
        if (eventBusName != null) payload.put("event_bus_name", eventBusName);
        if (resources != null) payload.put("resources", resources);
        return payload;
    }
}
