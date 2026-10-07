package com.acteon.client;

import com.fasterxml.jackson.databind.JsonNode;

/** Host-owned acceptance provenance. Keep sourceContext out of model messages. */
public record AgentServiceReceipt(String namespace, String tenant, String agent,
    String taskId, String sourceContext, JsonNode task) {
    public static final String SOURCE_CONTEXT_HEADER = "x-acteon-agent-source-context";
    static String source(String value) {
        if (value == null || value.isEmpty() || value.length() > 8192 || !value.matches("[A-Za-z0-9_-]+"))
            throw new IllegalArgumentException("agent service source context missing or malformed");
        return value;
    }
    static String segment(String value) {
        if (value == null || value.isEmpty() || value.equals(".") || value.equals(".."))
            throw new IllegalArgumentException("invalid agent service path segment");
        return A2A.segment(value);
    }
    static String base(String namespace, String tenant, String agent) {
        return "/a2a/"+segment(namespace)+"/"+segment(tenant)+"/agents/"+segment(agent)+"/v1";
    }
    static JsonNode verifyTask(JsonNode task, String namespace, String tenant, String id) {
        if (task == null || !task.isObject() || !task.path("id").isTextual() || task.path("id").asText().isEmpty()
            || !namespace.equals(task.path("namespace").asText()) || !tenant.equals(task.path("tenant").asText())
            || (id != null && !id.equals(task.path("id").asText())))
            throw new IllegalArgumentException("agent service task identity mismatch");
        return task;
    }
    @Override public String toString() {
        return "AgentServiceReceipt[namespace="+namespace+", tenant="+tenant+", agent="+agent+", taskId="+taskId+"]";
    }
}
