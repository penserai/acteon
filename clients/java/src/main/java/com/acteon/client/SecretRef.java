package com.acteon.client;

/** Durable pointer to host-owned secret material. */
public record SecretRef(String provider, String key, String version) {
    public SecretRef {
        if (!valid(provider) || !valid(key) || version != null && !valid(version))
            throw new IllegalArgumentException("secret reference is invalid");
    }
    public SecretRef(String provider, String key) { this(provider, key, null); }
    private static boolean valid(String value) {
        return value != null && value.matches("[A-Za-z0-9._/-]{1,256}");
    }
}
