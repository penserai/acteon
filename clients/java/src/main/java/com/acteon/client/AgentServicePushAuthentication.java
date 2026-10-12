package com.acteon.client;

/** HTTP authentication included on official A2A webhook deliveries. */
public record AgentServicePushAuthentication(String scheme, String credentials) {
    public AgentServicePushAuthentication {
        if (scheme == null || scheme.isBlank() || scheme.length() > 256
            || credentials != null && credentials.length() > 4096)
            throw new IllegalArgumentException("push authentication is invalid");
    }
    @Override public String toString() {
        return "AgentServicePushAuthentication[scheme=" + scheme + ", credentials=<redacted>]";
    }
}
