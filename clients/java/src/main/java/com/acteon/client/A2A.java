package com.acteon.client;

import java.net.URLEncoder;
import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * A2A protocol constants and factory helpers.
 *
 * <p>Mirrors the Rust, Python, Node, and Go SDKs. Wire payloads are
 * typed as {@code Map<String, Object>} matching the A2A JSON shapes
 * verbatim — the schema evolves and is JSON-native; pinned Java
 * record types would force a translation layer for every field
 * change. Factory methods cover the common construction cases.
 *
 * <p>The HTTP methods themselves live as members of
 * {@link ActeonClient}.
 */
public final class A2A {
    private A2A() {}

    /** A2A protocol version this client speaks. Matches the Rust
     *  client's {@code A2A_PROTOCOL_VERSION} and the server's
     *  negotiated version. */
    public static final String PROTOCOL_VERSION = "1.0";

    /** HTTP header name carrying the negotiated A2A protocol
     *  version. Sent on every authenticated A2A call. */
    public static final String VERSION_HEADER = "A2A-Version";

    // ------------------------------------------------------------------
    // Factory helpers
    // ------------------------------------------------------------------

    /** Build a text {@code Part} payload. The server rejects text
     *  larger than 256 KiB ({@code MAX_PART_TEXT_BYTES}) at
     *  validation time. */
    public static Map<String, Object> makePartText(String text) {
        Map<String, Object> p = new LinkedHashMap<>();
        p.put("text", text);
        return p;
    }

    /** Build a URL-reference {@code Part}. Use this for payloads
     *  that exceed the 256 KiB inline cap — the URL points at an
     *  external store the receiver fetches separately. */
    public static Map<String, Object> makePartUrl(String href) {
        Map<String, Object> p = new LinkedHashMap<>();
        p.put("url", href);
        return p;
    }

    /** Build a structured-data {@code Part}. The server JSON-encodes
     *  {@code value} to measure against {@code MAX_PART_DATA_BYTES}
     *  (256 KiB). {@code mediaType} defaults to
     *  {@code application/json} when null or empty. */
    public static Map<String, Object> makePartData(Object value, String mediaType) {
        if (mediaType == null || mediaType.isEmpty()) {
            mediaType = "application/json";
        }
        Map<String, Object> p = new LinkedHashMap<>();
        p.put("data", value);
        p.put("mediaType", mediaType);
        return p;
    }

    /** Optional fields for {@link #makeMessage}. Both are
     *  null-by-default; the resulting message dict will not carry
     *  the corresponding key when null. */
    public static final class MessageOptions {
        /** Thread the message into an existing Task's history.
         *  Null lets {@code a2aSendMessage} mint a fresh Task. */
        public String taskId;
        /** Optional context id carried across related tasks. */
        public String contextId;

        public MessageOptions taskId(String value) {
            this.taskId = value;
            return this;
        }

        public MessageOptions contextId(String value) {
            this.contextId = value;
            return this;
        }
    }

    /** Build a {@code TaskMessage} payload. {@code role} must be
     *  {@code "user"} or {@code "agent"} — the server validates. */
    public static Map<String, Object> makeMessage(
        String messageId,
        String role,
        List<Map<String, Object>> parts,
        MessageOptions opts
    ) {
        Map<String, Object> msg = new LinkedHashMap<>();
        msg.put("messageId", messageId);
        msg.put("role", role);
        msg.put("parts", parts);
        if (opts != null) {
            if (opts.taskId != null) msg.put("taskId", opts.taskId);
            if (opts.contextId != null) msg.put("contextId", opts.contextId);
        }
        return msg;
    }

    /** Convenience overload for the common no-options case. */
    public static Map<String, Object> makeMessage(
        String messageId,
        String role,
        List<Map<String, Object>> parts
    ) {
        return makeMessage(messageId, role, parts, null);
    }

    /** Optional fields for {@link #makePushConfig}. */
    public static final class PushConfigOptions {
        /** Pre-allocated config id (UUIDv7 by convention). Null
         *  lets the server mint one. */
        public String id;
        /** Optional bearer token sent in
         *  {@code Authorization: Bearer <token>} on every push
         *  POST. */
        public String token;
        /** Optional richer authentication metadata. */
        public Map<String, Object> authentication;

        public PushConfigOptions id(String value) {
            this.id = value;
            return this;
        }

        public PushConfigOptions token(String value) {
            this.token = value;
            return this;
        }

        public PushConfigOptions authentication(Map<String, Object> value) {
            this.authentication = value;
            return this;
        }
    }

    /** Build a {@code PushNotificationConfig} body. {@code url}
     *  must be {@code http://} or {@code https://} — the server
     *  denies other schemes at registration time. */
    public static Map<String, Object> makePushConfig(String url, PushConfigOptions opts) {
        Map<String, Object> body = new LinkedHashMap<>();
        body.put("url", url);
        if (opts != null) {
            if (opts.id != null) body.put("id", opts.id);
            if (opts.token != null) body.put("token", opts.token);
            if (opts.authentication != null) body.put("authentication", opts.authentication);
        }
        return body;
    }

    /** Convenience overload for the no-options case. */
    public static Map<String, Object> makePushConfig(String url) {
        return makePushConfig(url, null);
    }

    // ------------------------------------------------------------------
    // Internal — path-segment encoding shared by ActeonClient
    // ------------------------------------------------------------------

    /** Percent-encode a single path segment opaquely. Mirrors the
     *  Go/Python/Node clients — a tenant id or task id with
     *  reserved characters must not leak into additional path
     *  components. */
    static String segment(String s) {
        // URLEncoder encodes the form-www-urlencoded variant where
        // space becomes '+'. For path segments we need %20 instead.
        return URLEncoder.encode(s, StandardCharsets.UTF_8).replace("+", "%20");
    }

    /** The standard header set every authenticated A2A call sends. */
    static Map<String, String> defaultHeaders() {
        Map<String, String> headers = new HashMap<>();
        headers.put(VERSION_HEADER, PROTOCOL_VERSION);
        return headers;
    }
}
