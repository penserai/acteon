package com.acteon.client.models;

import java.util.HashMap;
import java.util.Map;

/**
 * Payload builder for the Azure Event Hubs send action ({@code azure-eventhubs}, action type {@code send_event}).
 */
public class AzureEventHubsSendPayload {
    private final Object body;
    private String eventHubName;
    private String partitionId;
    private Map<String, String> properties;

    public AzureEventHubsSendPayload(Object body) {
        this.body = body;
    }

    public AzureEventHubsSendPayload withEventHubName(String eventHubName) {
        this.eventHubName = eventHubName;
        return this;
    }

    public AzureEventHubsSendPayload withPartitionId(String partitionId) {
        this.partitionId = partitionId;
        return this;
    }

    public AzureEventHubsSendPayload withProperties(Map<String, String> properties) {
        this.properties = properties;
        return this;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        payload.put("body", body);
        if (eventHubName != null) payload.put("event_hub_name", eventHubName);
        if (partitionId != null) payload.put("partition_id", partitionId);
        if (properties != null) payload.put("properties", properties);
        return payload;
    }
}
