package com.acteon.client.models;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;

/**
 * Response body from {@code GET /.well-known/acteon-signing-keys}.
 *
 * <p>The {@code keys} list is empty when signing is disabled on the
 * server. The {@code count} field is always equal to
 * {@code keys.size()} — it's mirrored from the server as a
 * convenience for callers that only want a count.
 */
public class SigningKeysResponse {
    private List<SigningKeyEntry> keys;
    private int count;

    public List<SigningKeyEntry> getKeys() { return keys; }
    public void setKeys(List<SigningKeyEntry> keys) { this.keys = keys; }

    public int getCount() { return count; }
    public void setCount(int count) { this.count = count; }

    @SuppressWarnings("unchecked")
    public static SigningKeysResponse fromMap(Map<String, Object> data) {
        SigningKeysResponse resp = new SigningKeysResponse();
        List<Map<String, Object>> rawKeys = data.containsKey("keys") && data.get("keys") != null
            ? (List<Map<String, Object>>) data.get("keys")
            : new ArrayList<>();
        List<SigningKeyEntry> keys = new ArrayList<>(rawKeys.size());
        for (Map<String, Object> k : rawKeys) {
            keys.add(SigningKeyEntry.fromMap(k));
        }
        resp.keys = keys;
        // Defensive: derive from keys.size() when the server omits
        // count (it always emits it today, but we shouldn't break on
        // a minor server change).
        resp.count = data.containsKey("count") && data.get("count") != null
            ? ((Number) data.get("count")).intValue()
            : keys.size();
        return resp;
    }
}
