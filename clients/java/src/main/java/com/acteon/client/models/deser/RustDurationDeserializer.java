package com.acteon.client.models.deser;

import com.fasterxml.jackson.core.JsonParser;
import com.fasterxml.jackson.databind.DeserializationContext;
import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.deser.std.StdDeserializer;

import java.io.IOException;
import java.time.Duration;

/**
 * Decodes a Rust {@code std::time::Duration} as serialized by serde:
 * {@code {"secs": <u64>, "nanos": <u32>}}.
 *
 * <p>Used inside {@code ActionOutcome.Throttled.retryAfter} and any
 * other field whose source side stores a Rust {@code Duration} and
 * forwards it to JSON via serde's default representation. The wire
 * shape is non-negotiable on the server side, so the client absorbs
 * the structural decode here rather than asking every model that
 * holds a {@link Duration} to spell it out.
 */
public class RustDurationDeserializer extends StdDeserializer<Duration> {
    public RustDurationDeserializer() {
        super(Duration.class);
    }

    @Override
    public Duration deserialize(JsonParser p, DeserializationContext ctxt) throws IOException {
        JsonNode node = p.getCodec().readTree(p);
        if (node.isNumber()) {
            // Defensive: tolerate raw seconds if a server ever emits one.
            return Duration.ofSeconds(node.asLong());
        }
        long secs = node.has("secs") ? node.get("secs").asLong() : 0L;
        long nanos = node.has("nanos") ? node.get("nanos").asLong() : 0L;
        return Duration.ofSeconds(secs).plusNanos(nanos);
    }
}
