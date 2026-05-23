package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonSetter;

import java.util.ArrayList;
import java.util.List;

/**
 * Response body from {@code GET /.well-known/acteon-signing-keys}.
 *
 * <p>The {@code keys} list is empty when signing is disabled on the
 * server. The {@code count} field is always equal to
 * {@code keys.size()} — it's mirrored from the server as a
 * convenience for callers that only want a count. We derive
 * {@code count} from {@code keys.size()} on read so a server that
 * omits the field still produces a sensible value.
 */
public class SigningKeysResponse {
    private List<SigningKeyEntry> keys = new ArrayList<>();
    private Integer count;

    public List<SigningKeyEntry> getKeys() { return keys; }

    @JsonSetter("keys")
    public void setKeys(List<SigningKeyEntry> keys) {
        this.keys = keys != null ? keys : new ArrayList<>();
    }

    public int getCount() {
        return count != null ? count : keys.size();
    }

    public void setCount(int count) { this.count = count; }
}
