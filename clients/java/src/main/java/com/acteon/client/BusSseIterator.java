package com.acteon.client;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.type.MapType;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.Iterator;
import java.util.Map;
import java.util.NoSuchElementException;

/**
 * Iterator that lazily parses bus SSE frames and surfaces them as typed
 * {@link Bus.BusConsumeItem}s.
 *
 * <p>The dispatch {@link SseEventIterator} silently drops SSE comment
 * frames (keep-alives). Bus consumers want them as a liveness signal,
 * so this iterator yields a typed {@code KeepAlive} variant for each
 * one. Closes via {@link AutoCloseable} just like {@code SseEventIterator}.</p>
 */
public final class BusSseIterator
    implements Iterator<Bus.BusConsumeItem>, AutoCloseable {

    private static final ObjectMapper MAPPER = new ObjectMapper();

    private final BufferedReader reader;
    private final BusFrameReader frames;
    private Bus.BusConsumeItem nextItem;
    private boolean closed;

    public BusSseIterator(InputStream inputStream) {
        this.reader =
            new BufferedReader(new InputStreamReader(inputStream, StandardCharsets.UTF_8));
        this.frames = new BusFrameReader(reader);
    }

    @Override
    public boolean hasNext() {
        if (closed) {
            return false;
        }
        if (nextItem != null) {
            return true;
        }
        nextItem = readNextItem();
        return nextItem != null;
    }

    @Override
    public Bus.BusConsumeItem next() {
        if (!hasNext()) {
            throw new NoSuchElementException("No more bus SSE items");
        }
        Bus.BusConsumeItem item = nextItem;
        nextItem = null;
        return item;
    }

    @Override
    public void close() {
        if (!closed) {
            closed = true;
            try {
                reader.close();
            } catch (IOException ignored) {
                // Best-effort close.
            }
        }
    }

    private Bus.BusConsumeItem readNextItem() {
        BusFrameReader.Envelope env = frames.readNext();
        if (env == null) return null;
        if (env.keepAlive) return new Bus.BusConsumeItem.KeepAlive();
        String name = env.event != null && !env.event.isEmpty() ? env.event : "message";
        switch (name) {
            case "bus.message":
            case "message":
                try {
                    JsonNode root = MAPPER.readTree(env.data);
                    return new Bus.BusConsumeItem.Message(parseConsumedMessage(root));
                } catch (IOException e) {
                    close();
                    throw new IllegalStateException("invalid bus.message payload", e);
                }
            case "bus.error":
                return new Bus.BusConsumeItem.Error(extractErrorMessage(env.data));
            default:
                close();
                throw new IllegalStateException(
                    "unexpected SSE event '" + name + "' on bus subscribe stream");
        }
    }

    static Bus.BusConsumedMessage parseConsumedMessage(JsonNode node) {
        return new Bus.BusConsumedMessage(
            node.path("topic").asText(),
            optString(node, "key"),
            node.has("payload") ? node.get("payload") : null,
            mapField(node, "headers"),
            optInt(node, "partition"),
            optLong(node, "offset"),
            optString(node, "timestamp"));
    }

    static String optString(JsonNode node, String field) {
        JsonNode v = node.get(field);
        return v == null || v.isNull() ? null : v.asText();
    }

    static Integer optInt(JsonNode node, String field) {
        JsonNode v = node.get(field);
        return v == null || v.isNull() ? null : v.intValue();
    }

    static Long optLong(JsonNode node, String field) {
        JsonNode v = node.get(field);
        return v == null || v.isNull() ? null : v.longValue();
    }

    static Map<String, String> mapField(JsonNode node, String field) {
        JsonNode v = node.get(field);
        if (v == null || v.isNull()) return new HashMap<>();
        try {
            MapType t = MAPPER.getTypeFactory()
                .constructMapType(HashMap.class, String.class, String.class);
            return MAPPER.convertValue(v, t);
        } catch (Exception e) {
            return new HashMap<>();
        }
    }

    static String extractErrorMessage(String data) {
        try {
            JsonNode root = MAPPER.readTree(data);
            JsonNode err = root.get("error");
            if (err != null && err.isTextual()) {
                return err.asText();
            }
        } catch (IOException ignored) {
            // Fall through and return the raw body.
        }
        return data;
    }
}
