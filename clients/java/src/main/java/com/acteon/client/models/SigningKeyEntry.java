package com.acteon.client.models;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;

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
    private List<String> tenants;
    private List<String> namespaces;

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

    @SuppressWarnings("unchecked")
    public static SigningKeyEntry fromMap(Map<String, Object> data) {
        SigningKeyEntry entry = new SigningKeyEntry();
        entry.signerId = (String) data.get("signer_id");
        entry.kid = (String) data.get("kid");
        entry.algorithm = (String) data.get("algorithm");
        entry.publicKey = (String) data.get("public_key");
        entry.tenants = data.containsKey("tenants") && data.get("tenants") != null
            ? new ArrayList<>((List<String>) data.get("tenants"))
            : new ArrayList<>();
        entry.namespaces = data.containsKey("namespaces") && data.get("namespaces") != null
            ? new ArrayList<>((List<String>) data.get("namespaces"))
            : new ArrayList<>();
        return entry;
    }
}
