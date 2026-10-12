package com.acteon.client;

import com.fasterxml.jackson.databind.JsonNode;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

/** One schema-validated page of exact-agent push configurations. */
public record AgentServicePushConfigPage(List<AgentServicePushConfig> configs, String nextPageToken) {
    static AgentServicePushConfigPage parse(JsonNode value, String taskId) {
        if (value == null || !value.isObject())
            throw new IllegalArgumentException("agent service A2A push page is invalid");
        var names = new HashSet<String>(); value.fieldNames().forEachRemaining(names::add);
        if (!names.equals(Set.of("configs", "nextPageToken")) || !value.path("configs").isArray()
            || !value.path("nextPageToken").isTextual() || value.path("nextPageToken").asText().length() > 8192)
            throw new IllegalArgumentException("agent service A2A push page is invalid");
        var configs = new ArrayList<AgentServicePushConfig>();
        for (JsonNode config : value.path("configs")) configs.add(AgentServicePushConfig.parse(config, taskId, null));
        return new AgentServicePushConfigPage(List.copyOf(configs), value.path("nextPageToken").asText());
    }
}
