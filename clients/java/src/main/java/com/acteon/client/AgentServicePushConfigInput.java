package com.acteon.client;

/** Bounded input for one exact-agent push configuration. */
public record AgentServicePushConfigInput(
    String id, String url, String token, AgentServicePushAuthentication authentication, SecretRef tokenRef
) {
    public AgentServicePushConfigInput {
        if (url == null || url.isEmpty() || token != null && token.length() > 4096 || token != null && tokenRef != null)
            throw new IllegalArgumentException("push configuration is invalid");
    }
    public AgentServicePushConfigInput(String id, String url, String token, AgentServicePushAuthentication authentication) {
        this(id, url, token, authentication, null);
    }
    @Override public String toString() {
        return "AgentServicePushConfigInput[id=" + id + ", url=" + url
            + ", token=<redacted>, authentication=" + authentication + "]";
    }
}
