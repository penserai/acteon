package com.acteon.client.models;

import org.junit.jupiter.api.Test;

import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.*;

class SigningKeysResponseTest {

    @Test
    void testFromMapWithMultipleKeys() {
        Map<String, Object> key1 = new HashMap<>();
        key1.put("signer_id", "ci-bot");
        key1.put("kid", "k1");
        key1.put("algorithm", "Ed25519");
        key1.put("public_key", "LZkUda4pibD+v4yfHrLyw9Dnt7OLa6PGzSRGOcN1c4o=");
        key1.put("tenants", List.of("acme"));
        key1.put("namespaces", List.of("prod", "staging"));

        Map<String, Object> key2 = new HashMap<>();
        key2.put("signer_id", "ci-bot");
        key2.put("kid", "k2");
        key2.put("algorithm", "Ed25519");
        key2.put("public_key", "BBBB");
        key2.put("tenants", List.of("acme"));
        key2.put("namespaces", List.of("prod", "staging"));

        Map<String, Object> data = new HashMap<>();
        data.put("keys", List.of(key1, key2));
        data.put("count", 2);

        SigningKeysResponse resp = SigningKeysResponse.fromMap(data);
        assertEquals(2, resp.getCount());
        assertEquals(2, resp.getKeys().size());
        assertEquals("ci-bot", resp.getKeys().get(0).getSignerId());
        assertEquals("k1", resp.getKeys().get(0).getKid());
        assertEquals("Ed25519", resp.getKeys().get(0).getAlgorithm());
        assertEquals("k2", resp.getKeys().get(1).getKid());
        assertEquals(List.of("prod", "staging"), resp.getKeys().get(0).getNamespaces());
    }

    @Test
    void testFromMapWithEmptyKeysWhenSigningDisabled() {
        // Server emits {"keys": [], "count": 0} when [signing].enabled
        // is false — the client should round-trip that cleanly rather
        // than requiring callers to special-case a missing "keys" key.
        Map<String, Object> data = new HashMap<>();
        data.put("keys", Collections.emptyList());
        data.put("count", 0);

        SigningKeysResponse resp = SigningKeysResponse.fromMap(data);
        assertEquals(0, resp.getCount());
        assertTrue(resp.getKeys().isEmpty());
    }

    @Test
    void testFromMapDerivesCountWhenMissing() {
        // Defensive: the server always emits count today, but we
        // shouldn't break if a minor server change drops it.
        Map<String, Object> key = new HashMap<>();
        key.put("signer_id", "x");
        key.put("kid", "k0");
        key.put("algorithm", "Ed25519");
        key.put("public_key", "AAAA");
        key.put("tenants", List.of("*"));
        key.put("namespaces", List.of("*"));

        Map<String, Object> data = new HashMap<>();
        data.put("keys", List.of(key));
        // Deliberately omit count.

        SigningKeysResponse resp = SigningKeysResponse.fromMap(data);
        assertEquals(1, resp.getCount());
    }

    @Test
    void testEntryDefaultsMissingScopesToEmptyLists() {
        // Empty list is distinguishable from ["*"] (wildcard), so
        // callers can tell them apart. Don't invent a default.
        Map<String, Object> key = new HashMap<>();
        key.put("signer_id", "minimal");
        key.put("kid", "k0");
        key.put("algorithm", "Ed25519");
        key.put("public_key", "AAAA");

        SigningKeyEntry entry = SigningKeyEntry.fromMap(key);
        assertTrue(entry.getTenants().isEmpty());
        assertTrue(entry.getNamespaces().isEmpty());
    }

    @Test
    void testEntryThrowsOnMissingRequiredField() {
        // The fromMap used to do `(String) data.get("signer_id")`
        // which returned null silently, letting the null flow into
        // caller business logic. Now it throws a clear error.
        Map<String, Object> key = new HashMap<>();
        // signer_id deliberately omitted
        key.put("kid", "k0");
        key.put("algorithm", "Ed25519");
        key.put("public_key", "AAAA");

        IllegalArgumentException ex = assertThrows(
            IllegalArgumentException.class,
            () -> SigningKeyEntry.fromMap(key)
        );
        assertTrue(ex.getMessage().contains("signer_id"),
            "error should name the missing field: " + ex.getMessage());
        assertTrue(ex.getMessage().contains("malformed signing keys response"),
            "error should identify itself as a malformed response: " + ex.getMessage());
    }

    @Test
    void testEntryThrowsOnWrongTypedField() {
        // The old unchecked cast `(String) data.get("kid")` would
        // throw ClassCastException with no useful context when the
        // server sent a number. Now we throw a typed error that
        // names the offending field and its actual type.
        Map<String, Object> key = new HashMap<>();
        key.put("signer_id", "ci-bot");
        key.put("kid", 42); // wrong type
        key.put("algorithm", "Ed25519");
        key.put("public_key", "AAAA");

        IllegalArgumentException ex = assertThrows(
            IllegalArgumentException.class,
            () -> SigningKeyEntry.fromMap(key)
        );
        assertTrue(ex.getMessage().contains("kid"));
        assertTrue(ex.getMessage().contains("should be a string"));
    }

    @Test
    void testResponseThrowsWhenKeysIsNotArray() {
        // Guards the `(List<Map<String,Object>>) data.get("keys")`
        // cast — without the shape check, a server bug that sent
        // an object instead of an array would become a
        // ClassCastException deep in the caller.
        Map<String, Object> data = new HashMap<>();
        data.put("keys", "not an array");
        data.put("count", 0);

        IllegalArgumentException ex = assertThrows(
            IllegalArgumentException.class,
            () -> SigningKeysResponse.fromMap(data)
        );
        assertTrue(ex.getMessage().contains("'keys'"));
    }

    @Test
    void testResponseThrowsWhenCountIsWrongType() {
        // count="2" (string) used to be silently cast via (Number)
        // and throw ClassCastException. Now the error is typed.
        Map<String, Object> data = new HashMap<>();
        data.put("keys", Collections.emptyList());
        data.put("count", "2");

        IllegalArgumentException ex = assertThrows(
            IllegalArgumentException.class,
            () -> SigningKeysResponse.fromMap(data)
        );
        assertTrue(ex.getMessage().contains("'count'"));
    }

    @Test
    void testEntryThrowsOnNonStringScopeElement() {
        // Unchecked cast would let `[1, 2, 3]` sail through and
        // explode when callers iterate the list expecting strings.
        Map<String, Object> key = new HashMap<>();
        key.put("signer_id", "ci-bot");
        key.put("kid", "k1");
        key.put("algorithm", "Ed25519");
        key.put("public_key", "AAAA");
        key.put("tenants", List.of(1, 2, 3));

        IllegalArgumentException ex = assertThrows(
            IllegalArgumentException.class,
            () -> SigningKeyEntry.fromMap(key)
        );
        assertTrue(ex.getMessage().contains("tenants"));
        assertTrue(ex.getMessage().contains("non-string"));
    }
}
