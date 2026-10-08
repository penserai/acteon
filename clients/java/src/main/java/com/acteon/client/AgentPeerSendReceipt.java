package com.acteon.client;

import com.fasterxml.jackson.databind.JsonNode;

/** Durable peer submission; uncertain is distinct from known rejection. */
public record AgentPeerSendReceipt(String submissionId, String state, JsonNode task, String code) {
    static AgentPeerSendReceipt parse(JsonNode value, String namespace, String tenant) {
        if (value == null || !value.isObject() || value.size() != 2
                || !value.path("submission_id").isTextual() || !value.path("status").isObject())
            throw new IllegalArgumentException("agent peer receipt missing or malformed");
        String submission = value.path("submission_id").asText();
        if (!submission.matches("[0-9a-f]{8}-[0-9a-f]{4}-5[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"))
            throw new IllegalArgumentException("agent peer receipt missing or malformed");
        JsonNode status = value.path("status");
        String state = status.path("state").asText("");
        return switch (state) {
            case "uncertain" -> {
                if (status.size() != 1) throw new IllegalArgumentException("agent peer receipt missing or malformed");
                yield new AgentPeerSendReceipt(submission, state, null, null);
            }
            case "accepted" -> {
                if (status.size() != 2) throw new IllegalArgumentException("agent peer receipt missing or malformed");
                yield new AgentPeerSendReceipt(submission, state,
                    AgentServiceReceipt.verifyTask(status.path("task"), namespace, tenant, null), null);
            }
            case "rejected" -> {
                String code = status.path("code").asText("");
                if (status.size() != 2 || code.isEmpty() || code.length() > 1024
                        || !code.equals(code.trim()) || code.chars().anyMatch(c -> Character.isISOControl(c)))
                    throw new IllegalArgumentException("agent peer receipt missing or malformed");
                yield new AgentPeerSendReceipt(submission, state, null, code);
            }
            default -> throw new IllegalArgumentException("agent peer receipt missing or malformed");
        };
    }
}
