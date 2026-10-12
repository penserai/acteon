package com.acteon.client;

import com.fasterxml.jackson.databind.JsonNode;
import java.util.HashSet;
import java.util.Set;

/** Schema-validated exact-agent push configuration. */
public record AgentServicePushConfig(
    String id, String taskId, String url, String token,
    AgentServicePushAuthentication authentication
) {
    static AgentServicePushConfig parse(JsonNode value, String taskId, String expectedId) {
        if (value == null || !value.isObject())
            throw new IllegalArgumentException("agent service A2A push configuration is invalid");
        var names = new HashSet<String>(); value.fieldNames().forEachRemaining(names::add);
        if (!Set.of("id", "taskId", "url", "token", "authentication").containsAll(names)
            || !value.path("id").isTextual() || value.path("id").asText().isEmpty()
            || expectedId != null && !expectedId.equals(value.path("id").asText())
            || !taskId.equals(value.path("taskId").asText())
            || !value.path("url").isTextual() || value.path("url").asText().isEmpty()
            || value.has("token") && (!value.path("token").isTextual() || value.path("token").asText().length() > 4096))
            throw new IllegalArgumentException("agent service A2A push configuration is invalid");
        AgentServicePushAuthentication authentication = null;
        if (value.has("authentication")) {
            JsonNode auth = value.path("authentication");
            var authNames = new HashSet<String>(); auth.fieldNames().forEachRemaining(authNames::add);
            if (!auth.isObject() || !Set.of("scheme", "credentials").containsAll(authNames)
                || !auth.path("scheme").isTextual()
                || auth.has("credentials") && !auth.path("credentials").isTextual())
                throw new IllegalArgumentException("agent service A2A push authentication is invalid");
            authentication = new AgentServicePushAuthentication(
                auth.path("scheme").asText(), auth.has("credentials") ? auth.path("credentials").asText() : null);
        }
        return new AgentServicePushConfig(value.path("id").asText(), taskId, value.path("url").asText(),
            value.has("token") ? value.path("token").asText() : null, authentication);
    }
    @Override public String toString() {
        return "AgentServicePushConfig[id=" + id + ", taskId=" + taskId + ", url=" + url
            + ", token=<redacted>, authentication=" + authentication + "]";
    }
}
