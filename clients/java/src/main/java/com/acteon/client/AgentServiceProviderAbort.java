package com.acteon.client;

import com.acteon.client.exceptions.ActeonException;
import com.fasterxml.jackson.databind.JsonNode;
import java.util.UUID;
import java.util.regex.Pattern;

/** Provider-side abort finality, kept separate from Acteon's durable start fence. */
public record AgentServiceProviderAbort(String state, String attemptId, String proofDigest) {
    private static final Pattern PROOF = Pattern.compile("[0-9a-f]{64}");

    static AgentServiceProviderAbort parse(JsonNode value) throws ActeonException {
        if (value == null || value.isMissingNode() || value.isNull()) return null;
        if (!value.isObject() || !value.path("state").isTextual())
            throw new ActeonException("agent service provider abort status malformed");
        var state = value.path("state").asText();
        if (state.equals("restricted_only") && value.size() == 1)
            return new AgentServiceProviderAbort(state, null, null);
        if (state.equals("uncertain") && value.size() == 2 && value.path("attempt_id").isTextual()) {
            var id = value.path("attempt_id").asText();
            try {
                var parsed = UUID.fromString(id);
                if (parsed.version() == 5 && parsed.toString().equals(id))
                    return new AgentServiceProviderAbort(state, id, null);
            } catch (IllegalArgumentException ignored) {
                // Report the same bounded wire error below.
            }
        }
        if (state.equals("reconciled") && value.size() == 2 && value.path("proof_digest").isTextual()) {
            var digest = value.path("proof_digest").asText();
            if (PROOF.matcher(digest).matches())
                return new AgentServiceProviderAbort(state, null, digest);
        }
        throw new ActeonException("agent service provider abort status malformed");
    }
}
