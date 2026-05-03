package com.acteon.client;

import java.io.BufferedReader;
import java.io.IOException;

/**
 * Line-protocol parser for bus SSE streams. Yields one envelope per
 * {@link #readNext()} call: either a complete frame ({@code event} +
 * {@code data}) or a keep-alive comment.
 *
 * <p>Distinct from {@link SseEventIterator}'s parser because the bus
 * consumers want comment frames surfaced as a typed
 * {@code KeepAlive} variant — the dispatch event stream silently
 * drops them.</p>
 */
final class BusFrameReader {
    private final BufferedReader reader;

    BusFrameReader(BufferedReader reader) {
        this.reader = reader;
    }

    /** Returns the next envelope, or {@code null} when the stream ends. */
    Envelope readNext() {
        try {
            String event = null;
            String id = null;
            StringBuilder data = new StringBuilder();
            boolean hasData = false;
            String line;
            while ((line = reader.readLine()) != null) {
                if (line.startsWith(":")) {
                    return Envelope.keepAlive();
                }
                if (line.isEmpty()) {
                    if (hasData || event != null || id != null) {
                        return Envelope.frame(event, id, hasData ? data.toString() : "");
                    }
                    continue;
                }
                int colon = line.indexOf(':');
                String field;
                String value;
                if (colon >= 0) {
                    field = line.substring(0, colon);
                    value =
                        (colon + 1 < line.length() && line.charAt(colon + 1) == ' ')
                            ? line.substring(colon + 2)
                            : line.substring(colon + 1);
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
                        // Unknown fields are ignored per the SSE spec.
                        break;
                }
            }
            // End of stream — emit any partially buffered frame.
            if (hasData || event != null || id != null) {
                return Envelope.frame(event, id, hasData ? data.toString() : "");
            }
            return null;
        } catch (IOException e) {
            return null;
        }
    }

    /** One line-protocol envelope: a frame or a keep-alive comment. */
    static final class Envelope {
        final boolean keepAlive;
        final String event;
        final String id;
        final String data;

        private Envelope(boolean keepAlive, String event, String id, String data) {
            this.keepAlive = keepAlive;
            this.event = event;
            this.id = id;
            this.data = data;
        }

        static Envelope keepAlive() {
            return new Envelope(true, null, null, null);
        }

        static Envelope frame(String event, String id, String data) {
            return new Envelope(false, event, id, data);
        }
    }
}
