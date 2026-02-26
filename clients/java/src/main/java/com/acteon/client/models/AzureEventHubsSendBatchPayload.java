package com.acteon.client.models;

import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * Payload builder for the Azure Event Hubs send-batch action ({@code azure-eventhubs}, action type {@code send_batch}).
 */
public class AzureEventHubsSendBatchPayload {
    private final List<Map<String, Object>> events;
    private String eventHubName;

    public AzureEventHubsSendBatchPayload(List<Map<String, Object>> events) {
        this.events = events;
    }

    public AzureEventHubsSendBatchPayload withEventHubName(String eventHubName) {
        this.eventHubName = eventHubName;
        return this;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        payload.put("events", events);
        if (eventHubName != null) payload.put("event_hub_name", eventHubName);
        return payload;
    }
}
