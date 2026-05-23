package com.acteon.client.models;

import com.acteon.client.JsonMapper;
import com.fasterxml.jackson.databind.ObjectMapper;
import org.junit.jupiter.api.Test;

import java.util.List;

import static org.junit.jupiter.api.Assertions.*;

class SigningKeysResponseTest {
    private static final ObjectMapper MAPPER = JsonMapper.build();

    @Test
    void deserializesMultipleKeys() throws Exception {
        String json = """
            {
              "keys": [
                {
                  "signer_id": "ci-bot",
                  "kid": "k1",
                  "algorithm": "Ed25519",
                  "public_key": "LZkUda4pibD+v4yfHrLyw9Dnt7OLa6PGzSRGOcN1c4o=",
                  "tenants": ["acme"],
                  "namespaces": ["prod", "staging"]
                },
                {
                  "signer_id": "ci-bot",
                  "kid": "k2",
                  "algorithm": "Ed25519",
                  "public_key": "BBBB",
                  "tenants": ["acme"],
                  "namespaces": ["prod", "staging"]
                }
              ],
              "count": 2
            }
            """;

        SigningKeysResponse resp = MAPPER.readValue(json, SigningKeysResponse.class);

        assertEquals(2, resp.getCount());
        assertEquals(2, resp.getKeys().size());
        assertEquals("ci-bot", resp.getKeys().get(0).getSignerId());
        assertEquals("k1", resp.getKeys().get(0).getKid());
        assertEquals("Ed25519", resp.getKeys().get(0).getAlgorithm());
        assertEquals("k2", resp.getKeys().get(1).getKid());
        assertEquals(List.of("prod", "staging"), resp.getKeys().get(0).getNamespaces());
    }

    @Test
    void deserializesEmptyKeysWhenSigningDisabled() throws Exception {
        // Server emits {"keys": [], "count": 0} when [signing].enabled
        // is false — the client should round-trip that cleanly rather
        // than requiring callers to special-case a missing "keys" key.
        String json = """
            {"keys": [], "count": 0}
            """;

        SigningKeysResponse resp = MAPPER.readValue(json, SigningKeysResponse.class);

        assertEquals(0, resp.getCount());
        assertTrue(resp.getKeys().isEmpty());
    }

    @Test
    void derivesCountFromKeysWhenAbsent() throws Exception {
        // Defensive: the server always emits count today, but we
        // shouldn't break if a minor server change drops it.
        String json = """
            {
              "keys": [
                {
                  "signer_id": "x",
                  "kid": "k0",
                  "algorithm": "Ed25519",
                  "public_key": "AAAA",
                  "tenants": ["*"],
                  "namespaces": ["*"]
                }
              ]
            }
            """;

        SigningKeysResponse resp = MAPPER.readValue(json, SigningKeysResponse.class);

        assertEquals(1, resp.getCount());
    }

    @Test
    void entryDefaultsMissingScopesToEmptyLists() throws Exception {
        // Empty list is distinguishable from ["*"] (wildcard), so
        // callers can tell them apart.
        String json = """
            {
              "signer_id": "minimal",
              "kid": "k0",
              "algorithm": "Ed25519",
              "public_key": "AAAA"
            }
            """;

        SigningKeyEntry entry = MAPPER.readValue(json, SigningKeyEntry.class);

        assertTrue(entry.getTenants().isEmpty());
        assertTrue(entry.getNamespaces().isEmpty());
    }

    @Test
    void treatsNullKeysAsEmpty() throws Exception {
        // {"keys": null} should round-trip to an empty list rather
        // than NPE at getKeys().size() in caller code.
        String json = """
            {"keys": null, "count": 0}
            """;

        SigningKeysResponse resp = MAPPER.readValue(json, SigningKeysResponse.class);

        assertNotNull(resp.getKeys());
        assertTrue(resp.getKeys().isEmpty());
    }
}
