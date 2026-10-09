package com.acteon.client;

import com.fasterxml.jackson.databind.JsonNode;

/** Durable handoff for one exact remote authorization challenge. */
public record AgentPeerAuthorizationReceipt(
        String submissionId, String authorizationId, String state,
        JsonNode task, String progressCursor, String code) {
    static AgentPeerAuthorizationReceipt parse(
            JsonNode value, AgentServiceReceipt source, AgentPeerSendReceipt peer, String challengeId) {
        if (peer == null || !"accepted".equals(peer.state()) || peer.task() == null
                || value == null || !value.isObject() || value.size() != 3
                || !peer.submissionId().equals(value.path("submission_id").asText())
                || !value.path("authorization_id").isTextual()
                || !value.path("status").isObject())
            throw new IllegalArgumentException("agent peer authorization receipt missing or malformed");
        String authorization = value.path("authorization_id").asText();
        if (!authorization.matches("[0-9a-f]{8}-[0-9a-f]{4}-5[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"))
            throw new IllegalArgumentException("agent peer authorization receipt missing or malformed");
        JsonNode status = value.path("status");
        String state = status.path("state").asText("");
        return switch (state) {
            case "uncertain" -> {
                if (status.size() != 1) throw new IllegalArgumentException("agent peer authorization receipt missing or malformed");
                yield new AgentPeerAuthorizationReceipt(peer.submissionId(), authorization, state, null, null, null);
            }
            case "rejected" -> {
                String code = status.path("code").asText("");
                if (status.size() != 2 || code.isEmpty() || code.length() > 1024
                        || !code.equals(code.trim()) || code.chars().anyMatch(Character::isISOControl))
                    throw new IllegalArgumentException("agent peer authorization receipt missing or malformed");
                yield new AgentPeerAuthorizationReceipt(peer.submissionId(), authorization, state, null, null, code);
            }
            case "resolved" -> {
                String cursor = status.path("progress_cursor").asText("");
                if (status.size() != 3 || cursor.length() > 512 || !cursor.matches("\"[A-Za-z0-9_.:-]+\""))
                    throw new IllegalArgumentException("agent peer authorization receipt missing or malformed");
                JsonNode task = AgentServiceReceipt.verifyTask(
                    status.path("task"), source.namespace(), source.tenant(), peer.task().path("id").asText());
                String taskState = task.path("status").path("state").asText("");
                if (!(taskState.equals("working") || taskState.equals("completed")
                        || taskState.equals("input_required") || taskState.equals("auth_required"))
                        || (taskState.equals("auth_required")
                            && challengeId.equals(task.path("pendingApprovalId").asText())))
                    throw new IllegalArgumentException("agent peer authorization receipt missing or malformed");
                yield new AgentPeerAuthorizationReceipt(peer.submissionId(), authorization, state, task, cursor, null);
            }
            default -> throw new IllegalArgumentException("agent peer authorization receipt missing or malformed");
        };
    }
}
