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

    /**
     * Build a {@code SigningKeysResponse} from a decoded Jackson
     * map. Throws {@link IllegalArgumentException} on malformed
     * shapes — missing required fields, wrong types, etc — so
     * callers see a clear error instead of a
     * {@link ClassCastException} from deep inside the parser.
     */
    @SuppressWarnings("unchecked")
    public static SigningKeysResponse fromMap(Map<String, Object> data) {
        SigningKeysResponse resp = new SigningKeysResponse();

        Object rawKeysObj = data.get("keys");
        List<Map<String, Object>> rawKeys;
        if (rawKeysObj == null) {
            rawKeys = new ArrayList<>();
        } else if (rawKeysObj instanceof List<?>) {
            rawKeys = (List<Map<String, Object>>) rawKeysObj;
        } else {
            throw new IllegalArgumentException(
                "malformed signing keys response: 'keys' should be an array, got "
                    + rawKeysObj.getClass().getSimpleName()
            );
        }

        List<SigningKeyEntry> keys = new ArrayList<>(rawKeys.size());
        for (Map<String, Object> k : rawKeys) {
            keys.add(SigningKeyEntry.fromMap(k));
        }
        resp.keys = keys;

        // Defensive: derive from keys.size() when the server omits
        // count (it always emits it today, but we shouldn't break on
        // a minor server change). Wrong type on count (e.g. "2" as a
        // string) is a malformed response.
        Object rawCount = data.get("count");
        if (rawCount == null) {
            resp.count = keys.size();
        } else if (rawCount instanceof Number) {
            resp.count = ((Number) rawCount).intValue();
        } else {
            throw new IllegalArgumentException(
                "malformed signing keys response: 'count' should be a number, got "
                    + rawCount.getClass().getSimpleName()
            );
        }
        return resp;
    }
}
