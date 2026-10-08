package com.acteon.client;

import com.acteon.client.models.PermitReference;
import java.util.List;

/** Opaque governed parent context and the explicit permits delegated to a peer agent. */
public record AgentServiceParent(String executionContext, List<PermitReference> permits) {
    public AgentServiceParent {
        permits = permits == null ? List.of() : List.copyOf(permits);
    }
}
