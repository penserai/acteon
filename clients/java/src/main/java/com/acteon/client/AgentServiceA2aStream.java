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
 * Official A2A StreamResponse iterator for one exact governed agent.
 * Admission provenance remains outside the yielded protocol result.
 */
public final class AgentServiceA2aStream implements Iterator<JsonNode>, AutoCloseable {
    private static final int MAX_FRAME_BYTES = 2 * 1024 * 1024;
    private static final ObjectMapper MAPPER = JsonMapper.build();

    private final BufferedReader reader;
    private final String requestId;
    private final String sourceContext;
    private JsonNode nextResult;
    private boolean closed;

    AgentServiceA2aStream(InputStream input, String requestId, String sourceContext) {
        this.reader = new BufferedReader(new InputStreamReader(input, StandardCharsets.UTF_8));
        this.requestId = requestId;
        this.sourceContext = sourceContext;
    }

    /** Opaque admission provenance for SendStreamingMessage; null for subscriptions. */
    public String sourceContext() { return sourceContext; }

    /** JSON-RPC request identity required on every frame. */
    public String requestId() { return requestId; }

    @Override public boolean hasNext() {
        if (closed) return false;
        if (nextResult != null) return true;
        nextResult = readNextResult();
        return nextResult != null;
    }

    @Override public JsonNode next() {
        if (!hasNext()) throw new NoSuchElementException("No more A2A stream frames");
        JsonNode result = nextResult;
        nextResult = null;
        return result;
    }

    @Override public void close() {
        if (closed) return;
        closed = true;
        try { reader.close(); } catch (IOException ignored) { /* best effort */ }
    }

    private JsonNode readNextResult() {
        StringBuilder data = new StringBuilder();
        int bytes = 0;
        try {
            while (true) {
                String line = readBoundedLine();
                if (line == null) {
                    if (data.length() == 0) { close(); return null; }
                    return parse(data.toString());
                }
                if (line.isEmpty()) {
                    if (data.length() > 0) return parse(data.toString());
                    continue;
                }
                if (!line.startsWith("data:")) continue;
                String value = line.substring(5);
                if (value.startsWith(" ")) value = value.substring(1);
                if (data.length() > 0) data.append('\n');
                data.append(value);
                bytes += value.getBytes(StandardCharsets.UTF_8).length;
                if (bytes > MAX_FRAME_BYTES) throw new IllegalStateException("A2A SSE frame exceeds 2 MiB");
            }
        } catch (IOException error) {
            close();
            throw new IllegalStateException("A2A SSE stream failed", error);
        }
    }

    private String readBoundedLine() throws IOException {
        StringBuilder line = new StringBuilder();
        while (true) {
            int next = reader.read();
            if (next == -1) return line.length() == 0 ? null : line.toString();
            if (next == '\n') return line.toString();
            if (next == '\r') {
                reader.mark(1);
                int after = reader.read();
                if (after != '\n' && after != -1) reader.reset();
                return line.toString();
            }
            line.append((char) next);
            if (line.length() > MAX_FRAME_BYTES) throw new IllegalStateException("A2A SSE line exceeds 2 MiB");
        }
    }

    private JsonNode parse(String data) {
        try {
            JsonNode rpc = MAPPER.readTree(data);
            if (rpc == null || !rpc.isObject() || !"2.0".equals(rpc.path("jsonrpc").asText())
                    || !requestId.equals(rpc.path("id").asText())) {
                throw new IllegalStateException("A2A SSE frame request identity mismatch");
            }
            if (rpc.path("error").isObject()) {
                throw new IllegalStateException("A2A stream error: " + rpc.path("error"));
            }
            JsonNode result = rpc.path("result");
            if (!result.isObject()) throw new IllegalStateException("A2A SSE frame has no object result");
            return result;
        } catch (IOException error) {
            close();
            throw new IllegalStateException("invalid A2A SSE frame", error);
        } catch (RuntimeException error) {
            close();
            throw error;
        }
    }
}
