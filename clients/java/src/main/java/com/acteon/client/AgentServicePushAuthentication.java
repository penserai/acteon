package com.acteon.client;

/** HTTP authentication included on official A2A webhook deliveries. */
public record AgentServicePushAuthentication(String scheme, String credentials, SecretRef credentialsRef) {
    public AgentServicePushAuthentication {
        if (scheme == null || scheme.isBlank() || scheme.length() > 256
            || credentials != null && credentials.length() > 4096 || credentials != null && credentialsRef != null)
            throw new IllegalArgumentException("push authentication is invalid");
    }
    public AgentServicePushAuthentication(String scheme, String credentials) { this(scheme, credentials, null); }
    @Override public String toString() {
        return "AgentServicePushAuthentication[scheme=" + scheme + ", credentials=<redacted>]";
    }
}
