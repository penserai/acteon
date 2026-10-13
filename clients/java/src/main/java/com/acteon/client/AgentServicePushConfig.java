package com.acteon.client;

import com.fasterxml.jackson.databind.JsonNode;
import java.util.HashSet;
import java.util.Set;

/** Schema-validated exact-agent push configuration. */
public record AgentServicePushConfig(
    String id, String taskId, String url, String token,
    AgentServicePushAuthentication authentication, SecretRef tokenRef
) {
    static AgentServicePushConfig parse(JsonNode value, String taskId, String expectedId) {
        if (value == null || !value.isObject())
            throw new IllegalArgumentException("agent service A2A push configuration is invalid");
        var names = new HashSet<String>(); value.fieldNames().forEachRemaining(names::add);
        if (!Set.of("id", "taskId", "url", "token", "tokenRef", "authentication").containsAll(names)
            || !value.path("id").isTextual() || value.path("id").asText().isEmpty()
            || expectedId != null && !expectedId.equals(value.path("id").asText())
            || !taskId.equals(value.path("taskId").asText())
            || !value.path("url").isTextual() || value.path("url").asText().isEmpty()
            || value.has("token") && (!value.path("token").isTextual() || value.path("token").asText().length() > 4096))
            throw new IllegalArgumentException("agent service A2A push configuration is invalid");
        SecretRef tokenRef = value.has("tokenRef") ? parseSecretRef(value.path("tokenRef")) : null;
        if (value.has("token") && tokenRef != null)
            throw new IllegalArgumentException("agent service A2A push token reference is invalid");
        AgentServicePushAuthentication authentication = null;
        if (value.has("authentication")) {
            JsonNode auth = value.path("authentication");
            var authNames = new HashSet<String>(); auth.fieldNames().forEachRemaining(authNames::add);
            if (!auth.isObject() || !Set.of("scheme", "credentials", "credentialsRef").containsAll(authNames)
                || !auth.path("scheme").isTextual()
                || auth.has("credentials") && !auth.path("credentials").isTextual())
                throw new IllegalArgumentException("agent service A2A push authentication is invalid");
            SecretRef credentialsRef = auth.has("credentialsRef") ? parseSecretRef(auth.path("credentialsRef")) : null;
            authentication = new AgentServicePushAuthentication(
                auth.path("scheme").asText(), auth.has("credentials") ? auth.path("credentials").asText() : null, credentialsRef);
        }
        return new AgentServicePushConfig(value.path("id").asText(), taskId, value.path("url").asText(),
            value.has("token") ? value.path("token").asText() : null, authentication, tokenRef);
    }
    private static SecretRef parseSecretRef(JsonNode value) {
        var names = new HashSet<String>(); value.fieldNames().forEachRemaining(names::add);
        if (!value.isObject() || !Set.of("provider", "key", "version").containsAll(names)
            || !value.path("provider").isTextual() || !value.path("key").isTextual()
            || value.has("version") && !value.path("version").isTextual())
            throw new IllegalArgumentException("secret reference is invalid");
        return new SecretRef(value.path("provider").asText(), value.path("key").asText(),
            value.has("version") ? value.path("version").asText() : null);
    }
    @Override public String toString() {
        return "AgentServicePushConfig[id=" + id + ", taskId=" + taskId + ", url=" + url
            + ", token=<redacted>, authentication=" + authentication + "]";
    }
}
