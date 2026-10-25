package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

/** Current credential and optional stable principal binding. */
public record CredentialIdentity(
    @JsonProperty("credential_id") String credentialId,
    @JsonProperty("auth_method") String authMethod,
    String role,
    PrincipalIdentity principal,
    @JsonProperty("authority_id") String authorityId,
    TenantContext scope
) {
    public CredentialIdentity {
        if (scope == null) throw new IllegalArgumentException("credential tenant context required");
    }
}
