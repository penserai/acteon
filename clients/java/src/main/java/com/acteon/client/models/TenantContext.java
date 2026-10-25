package com.acteon.client.models;

/** Exact server-owned tenant context bound to a credential. */
public record TenantContext(String namespace, String tenant) {
    public TenantContext {
        if (namespace == null || namespace.isBlank() || tenant == null || tenant.isBlank())
            throw new IllegalArgumentException("credential tenant context missing or malformed");
    }
}
