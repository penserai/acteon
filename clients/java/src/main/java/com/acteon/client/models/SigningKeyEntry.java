package com.acteon.client.models;

import java.util.ArrayList;
import java.util.List;

/**
 * One verifying key in the server's active signing keyring.
 *
 * <p>Mirrors the shape returned by
 * {@code GET /.well-known/acteon-signing-keys}. Each entry identifies
 * a {@code (signer_id, kid)} pair the server will accept signatures
 * from, along with the algorithm (always {@code Ed25519} today) and
 * the tenant/namespace scope the key is authorized for.
 */
public class SigningKeyEntry {
    private String signerId;
    private String kid;
    private String algorithm;
    private String publicKey;
    /**
     * Default to an empty list so a server response that omits
     * {@code "tenants"} returns a non-null collection — consistent
     * with the pre-Jackson-migration {@code fromMap} behavior and
     * distinguishable from the wildcard {@code ["*"]} shape the
     * server emits when scopes are configured.
     */
    private List<String> tenants = new ArrayList<>();
    private List<String> namespaces = new ArrayList<>();

    public String getSignerId() { return signerId; }
    public void setSignerId(String signerId) { this.signerId = signerId; }

    public String getKid() { return kid; }
    public void setKid(String kid) { this.kid = kid; }

    public String getAlgorithm() { return algorithm; }
    public void setAlgorithm(String algorithm) { this.algorithm = algorithm; }

    /** Raw 32-byte Ed25519 public key, base64-encoded. */
    public String getPublicKey() { return publicKey; }
    public void setPublicKey(String publicKey) { this.publicKey = publicKey; }

    /** Tenant scopes this key is authorized for. {@code ["*"]} means all. */
    public List<String> getTenants() { return tenants; }
    public void setTenants(List<String> tenants) { this.tenants = tenants; }

    /** Namespace scopes this key is authorized for. {@code ["*"]} means all. */
    public List<String> getNamespaces() { return namespaces; }
    public void setNamespaces(List<String> namespaces) { this.namespaces = namespaces; }
}
