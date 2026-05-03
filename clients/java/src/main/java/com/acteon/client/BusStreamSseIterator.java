package com.acteon.client;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;
import java.util.Iterator;
import java.util.NoSuchElementException;

/**
 * Iterator that lazily parses bus stream-id SSE frames and surfaces
 * them as typed {@link Bus.BusStreamItem}s.
 *
 * <p>Closes once a terminal {@code End} envelope is observed so the
 * caller's {@code while (iter.hasNext())} loop drops out cleanly.</p>
 */
public final class BusStreamSseIterator
    implements Iterator<Bus.BusStreamItem>, AutoCloseable {

    private static final ObjectMapper MAPPER = new ObjectMapper();

    private final BufferedReader reader;
    private final BusFrameReader frames;
    private Bus.BusStreamItem nextItem;
    private boolean closed;
    private boolean terminated;

    public BusStreamSseIterator(InputStream inputStream) {
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
        if (terminated) {
            close();
            return false;
        }
        nextItem = readNextItem();
        return nextItem != null;
    }

    @Override
    public Bus.BusStreamItem next() {
        if (!hasNext()) {
            throw new NoSuchElementException("No more bus stream items");
        }
        Bus.BusStreamItem item = nextItem;
        nextItem = null;
        if (item instanceof Bus.BusStreamItem.End) {
            terminated = true;
        }
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

    private Bus.BusStreamItem readNextItem() {
        BusFrameReader.Envelope env = frames.readNext();
        if (env == null) return null;
        if (env.keepAlive) return new Bus.BusStreamItem.KeepAlive();
        String name = env.event != null && !env.event.isEmpty() ? env.event : "message";
        switch (name) {
            case "bus.stream.chunk":
                try {
                    JsonNode root = MAPPER.readTree(env.data);
                    return new Bus.BusStreamItem.Chunk(parseChunk(root));
                } catch (IOException e) {
                    close();
                    throw new IllegalStateException("invalid stream chunk payload", e);
                }
            case "bus.stream.end":
                try {
                    JsonNode root = MAPPER.readTree(env.data);
                    return new Bus.BusStreamItem.End(parseEnd(root));
                } catch (IOException e) {
                    close();
                    throw new IllegalStateException("invalid stream end payload", e);
                }
            case "bus.stream.error":
                return new Bus.BusStreamItem.Error(BusSseIterator.extractErrorMessage(env.data));
            default:
                close();
                throw new IllegalStateException(
                    "unexpected SSE event '" + name + "' on bus stream consumer");
        }
    }

    static Bus.StreamChunkEnvelope parseChunk(JsonNode node) {
        return new Bus.StreamChunkEnvelope(
            node.path("stream_id").asText(),
            node.path("chunk_seq").asLong(),
            node.has("body") ? node.get("body") : null,
            BusSseIterator.optString(node, "sender"),
            BusSseIterator.mapField(node, "metadata"),
            BusSseIterator.optString(node, "created_at"));
    }

    static Bus.StreamEndEnvelope parseEnd(JsonNode node) {
        return new Bus.StreamEndEnvelope(
            node.path("stream_id").asText(),
            node.path("chunk_seq").asLong(),
            node.path("status").asText(),
            BusSseIterator.optString(node, "error_message"),
            BusSseIterator.optString(node, "sender"),
            BusSseIterator.mapField(node, "metadata"),
            BusSseIterator.optString(node, "created_at"));
    }
}
