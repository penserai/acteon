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

    /**
     * Build a {@code SigningKeyEntry} from a generic {@code Map}
     * decoded by Jackson. Throws {@link IllegalArgumentException} on
     * missing or wrong-typed required fields so callers see a clear
     * "malformed response" error instead of a raw
     * {@link ClassCastException} bubbling from deep inside the
     * parser.
     *
     * <p>Kept as a static factory (rather than annotated Jackson
     * deserialization) so this class matches the {@code fromMap}
     * convention used by every other model in this SDK (see
     * {@link ProviderHealthStatus}, {@link ActionOutcome}, etc).
     * A codebase-wide migration to annotated deserialization is a
     * separate concern.
     */
    public static SigningKeyEntry fromMap(Map<String, Object> data) {
        SigningKeyEntry entry = new SigningKeyEntry();
        entry.signerId = requireString(data, "signer_id");
        entry.kid = requireString(data, "kid");
        entry.algorithm = requireString(data, "algorithm");
        entry.publicKey = requireString(data, "public_key");
        entry.tenants = optionalStringList(data, "tenants");
        entry.namespaces = optionalStringList(data, "namespaces");
        return entry;
    }

    /**
     * Pull a required string field out of a raw JSON map. Missing,
     * null, and wrong-type values all surface as a
     * {@link IllegalArgumentException} with the offending field name
     * included so an operator can tell from the message exactly
     * which part of the response was malformed.
     */
    static String requireString(Map<String, Object> data, String field) {
        Object value = data.get(field);
        if (value == null) {
            throw new IllegalArgumentException(
                "malformed signing keys response: missing required string field '" + field + "'"
            );
        }
        if (!(value instanceof String)) {
            throw new IllegalArgumentException(
                "malformed signing keys response: field '" + field
                    + "' should be a string, got " + value.getClass().getSimpleName()
            );
        }
        return (String) value;
    }

    /**
     * Read an optional scope list. Missing or null becomes an empty
     * list (distinguishable from the wildcard {@code ["*"]} shape
     * the server emits when no scope is configured). Wrong shape
     * (e.g. an object, a number, or a list of non-strings) is a
     * malformed response, not silently coerced.
     */
    @SuppressWarnings("unchecked")
    static List<String> optionalStringList(Map<String, Object> data, String field) {
        Object value = data.get(field);
        if (value == null) {
            return new ArrayList<>();
        }
        if (!(value instanceof List<?>)) {
            throw new IllegalArgumentException(
                "malformed signing keys response: field '" + field
                    + "' should be a list, got " + value.getClass().getSimpleName()
            );
        }
        List<?> raw = (List<?>) value;
        for (Object item : raw) {
            if (item != null && !(item instanceof String)) {
                throw new IllegalArgumentException(
                    "malformed signing keys response: field '" + field
                        + "' contains non-string element of type " + item.getClass().getSimpleName()
                );
            }
        }
        return new ArrayList<>((List<String>) raw);
    }
}
