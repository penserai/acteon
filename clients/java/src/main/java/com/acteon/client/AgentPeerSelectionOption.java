package com.acteon.client;

import com.fasterxml.jackson.databind.JsonNode;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

/** Safe registry data for model selection. The description is untrusted text. */
public record AgentPeerSelectionOption(String agentId, String skill,
    String descriptionUntrusted, String cardVersion, String bindingDigest,
    long checkedAtMs) {
    private static boolean token(String value) {
        return value != null && value.matches("[A-Za-z0-9._-]{1,120}");
    }

    static List<AgentPeerSelectionOption> parse(JsonNode value, String requestedSkill) {
        if (value == null || !value.isObject() || value.size() != 1
            || !value.has("peers") || !value.path("peers").isArray()
            || value.path("peers").size() > 128)
            throw new IllegalArgumentException("agent peer discovery response missing or malformed");
        var result = new ArrayList<AgentPeerSelectionOption>();
        var seen = new HashSet<String>();
        Set<String> fields = Set.of("agent_id", "skill", "description_untrusted",
            "card_version", "binding_digest", "checked_at_ms");
        for (var peer : value.path("peers")) {
            var names = new HashSet<String>();
            peer.fieldNames().forEachRemaining(names::add);
            var checked = peer.path("checked_at_ms");
            var description = peer.get("description_untrusted");
            if (!peer.isObject() || !names.equals(fields)
                || !token(peer.path("agent_id").textValue())
                || !token(peer.path("skill").textValue())
                || !requestedSkill.equals(peer.path("skill").textValue())
                || !token(peer.path("card_version").textValue())
                || !peer.path("binding_digest").asText("").matches("[0-9a-f]{64}")
                || !checked.isIntegralNumber() || !checked.canConvertToLong()
                || checked.longValue() < 0
                || !(description == null || description.isNull()
                    || (description.isTextual() && description.textValue()
                        .getBytes(java.nio.charset.StandardCharsets.UTF_8).length <= 2048))
                || !seen.add(peer.path("agent_id").textValue()))
                throw new IllegalArgumentException("agent peer discovery response missing or malformed");
            result.add(new AgentPeerSelectionOption(peer.path("agent_id").textValue(),
                requestedSkill, description == null || description.isNull() ? null : description.textValue(),
                peer.path("card_version").textValue(), peer.path("binding_digest").textValue(),
                checked.longValue()));
        }
        return List.copyOf(result);
    }
}
