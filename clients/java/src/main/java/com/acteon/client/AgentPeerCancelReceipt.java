package com.acteon.client;

import com.fasterxml.jackson.databind.JsonNode;

/** Durable at-most-once remote cancellation outcome. */
public record AgentPeerCancelReceipt(
        String submissionId, String cancellationId, String state, JsonNode task, String code) {
    static AgentPeerCancelReceipt parse(JsonNode value, AgentServiceReceipt source, AgentPeerSendReceipt peer) {
        if (peer == null || !"accepted".equals(peer.state()) || peer.task() == null
                || value == null || !value.isObject() || value.size() != 3
                || !peer.submissionId().equals(value.path("submission_id").asText())
                || !value.path("cancellation_id").isTextual() || !value.path("status").isObject())
            throw new IllegalArgumentException("agent peer cancellation receipt missing or malformed");
        String cancellation = value.path("cancellation_id").asText();
        if (!cancellation.matches("[0-9a-f]{8}-[0-9a-f]{4}-5[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"))
            throw new IllegalArgumentException("agent peer cancellation receipt missing or malformed");
        JsonNode status = value.path("status");
        String state = status.path("state").asText("");
        return switch (state) {
            case "unsupported", "uncertain" -> {
                if (status.size() != 1) throw new IllegalArgumentException("agent peer cancellation receipt missing or malformed");
                yield new AgentPeerCancelReceipt(peer.submissionId(), cancellation, state, null, null);
            }
            case "rejected" -> {
                String code = status.path("code").asText("");
                if (status.size() != 2 || code.isEmpty() || code.length() > 1024
                        || !code.equals(code.trim()) || code.chars().anyMatch(Character::isISOControl))
                    throw new IllegalArgumentException("agent peer cancellation receipt missing or malformed");
                yield new AgentPeerCancelReceipt(peer.submissionId(), cancellation, state, null, code);
            }
            case "reconciled" -> {
                if (status.size() != 2) throw new IllegalArgumentException("agent peer cancellation receipt missing or malformed");
                JsonNode task = AgentServiceReceipt.verifyTask(status.path("task"), source.namespace(), source.tenant(), peer.task().path("id").asText());
                String taskState = task.path("status").path("state").asText("");
                if (!java.util.Set.of("completed", "failed", "canceled", "rejected").contains(taskState))
                    throw new IllegalArgumentException("agent peer cancellation receipt missing or malformed");
                yield new AgentPeerCancelReceipt(peer.submissionId(), cancellation, state, task, null);
            }
            default -> throw new IllegalArgumentException("agent peer cancellation receipt missing or malformed");
        };
    }
}
