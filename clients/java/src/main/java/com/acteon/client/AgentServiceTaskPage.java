package com.acteon.client;

import com.fasterxml.jackson.databind.JsonNode;
import java.util.ArrayList;
import java.util.List;
import java.util.Set;

/** Schema-validated official A2A task page for one exact governed agent. */
public record AgentServiceTaskPage(
    List<JsonNode> tasks,
    String nextPageToken,
    int pageSize,
    int totalSize
) {
    static AgentServiceTaskPage parse(JsonNode value) {
        if (value == null || !value.isObject())
            throw new IllegalArgumentException("agent service A2A task page is malformed");
        var names = new java.util.HashSet<String>();
        value.fieldNames().forEachRemaining(names::add);
        if (!names.equals(Set.of("tasks", "nextPageToken", "pageSize", "totalSize"))
            || !value.path("tasks").isArray() || !value.path("nextPageToken").isTextual()
            || value.path("nextPageToken").asText().length() > 8192
            || !value.path("pageSize").isIntegralNumber() || !value.path("pageSize").canConvertToInt()
            || !value.path("totalSize").isIntegralNumber() || !value.path("totalSize").canConvertToInt())
            throw new IllegalArgumentException("agent service A2A task page is malformed");
        int pageSize = value.path("pageSize").asInt();
        int totalSize = value.path("totalSize").asInt();
        if (pageSize < 1 || pageSize > 100 || value.path("tasks").size() > pageSize
            || totalSize < value.path("tasks").size())
            throw new IllegalArgumentException("agent service A2A task page is malformed");
        var tasks = new ArrayList<JsonNode>();
        for (JsonNode task : value.path("tasks")) {
            String state = task.path("status").path("state").asText("");
            boolean validState = switch (state) {
                case "TASK_STATE_SUBMITTED", "TASK_STATE_WORKING", "TASK_STATE_COMPLETED",
                    "TASK_STATE_FAILED", "TASK_STATE_CANCELED", "TASK_STATE_INPUT_REQUIRED",
                    "TASK_STATE_AUTH_REQUIRED", "TASK_STATE_REJECTED" -> true;
                default -> false;
            };
            if (!task.isObject() || !task.path("id").isTextual() || task.path("id").asText().isEmpty()
                || !validState
                || task.has("namespace") || task.has("tenant")
                || task.path("metadata").has("acteon_governed_execution"))
                throw new IllegalArgumentException("agent service A2A task projection is invalid");
            tasks.add(task.deepCopy());
        }
        return new AgentServiceTaskPage(
            List.copyOf(tasks), value.path("nextPageToken").asText(), pageSize, totalSize);
    }
}
