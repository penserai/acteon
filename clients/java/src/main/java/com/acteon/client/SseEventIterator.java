package com.acteon.client;

import com.acteon.client.models.SseEvent;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.util.Iterator;
import java.util.NoSuchElementException;

/**
 * Iterator that lazily parses Server-Sent Events from an HTTP response stream.
 *
 * <p>Implements {@link AutoCloseable} so the underlying stream is closed when
 * the iterator is no longer needed.</p>
 *
 * <p>The SSE protocol uses blank lines to delimit events. Each event may contain
 * {@code id:}, {@code event:}, and {@code data:} fields. Comment lines
 * (starting with {@code :}) are silently skipped.</p>
 *
 * <p>Example usage:</p>
 * <pre>{@code
 * try (SseEventIterator iter = client.stream(options)) {
 *     while (iter.hasNext()) {
 *         SseEvent event = iter.next();
 *         System.out.println(event.getEvent() + ": " + event.getData());
 *     }
 * }
 * }</pre>
 */
public class SseEventIterator implements Iterator<SseEvent>, AutoCloseable {
    private final BufferedReader reader;
    private SseEvent nextEvent;
    private boolean closed;

    /**
     * Creates a new SSE event iterator from an input stream.
     */
    public SseEventIterator(InputStream inputStream) {
        this.reader = new BufferedReader(new InputStreamReader(inputStream, StandardCharsets.UTF_8));
        this.closed = false;
    }

    @Override
    public boolean hasNext() {
        if (closed) {
            return false;
        }
        if (nextEvent != null) {
            return true;
        }
        nextEvent = readNextEvent();
        return nextEvent != null;
    }

    @Override
    public SseEvent next() {
        if (!hasNext()) {
            throw new NoSuchElementException("No more SSE events");
        }
        SseEvent event = nextEvent;
        nextEvent = null;
        return event;
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

    private SseEvent readNextEvent() {
        try {
            String id = null;
            String event = null;
            StringBuilder data = new StringBuilder();
            boolean hasData = false;

            String line;
            while ((line = reader.readLine()) != null) {
                if (line.isEmpty()) {
                    // Blank line signals end of an event.
                    if (hasData || id != null || event != null) {
                        return new SseEvent(
                            id,
                            event,
                            hasData ? data.toString() : null
                        );
                    }
                    // Otherwise, skip consecutive blank lines.
                    continue;
                }

                // Comment lines start with ':' -- skip them.
                if (line.startsWith(":")) {
                    continue;
                }

                int colonIndex = line.indexOf(':');
                String field;
                String value;
                if (colonIndex >= 0) {
                    field = line.substring(0, colonIndex);
                    // Skip optional single leading space after the colon.
                    value = (colonIndex + 1 < line.length() && line.charAt(colonIndex + 1) == ' ')
                        ? line.substring(colonIndex + 2)
                        : line.substring(colonIndex + 1);
                } else {
                    field = line;
                    value = "";
                }

                switch (field) {
                    case "id":
                        id = value;
                        break;
                    case "event":
                        event = value;
                        break;
                    case "data":
                        if (hasData) {
                            data.append('\n');
                        }
                        data.append(value);
                        hasData = true;
                        break;
                    default:
                        // Ignore unknown fields per the SSE specification.
                        break;
                }
            }

            // Stream ended. Emit any partially buffered event.
            if (hasData || id != null || event != null) {
                return new SseEvent(id, event, hasData ? data.toString() : null);
            }

            return null;
        } catch (IOException e) {
            close();
            return null;
        }
    }
}
