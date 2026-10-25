package com.acteon.client;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpServer;
import org.junit.jupiter.api.Test;

import com.acteon.client.exceptions.ApiException;
import java.io.IOException;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.*;

/**
 * A2A Java SDK — factory + URL/header smoke tests.
 *
 * Live HTTP tests would need a running Acteon instance with A2A
 * enabled; these tests exercise the wire surface of the new
 * {@link A2A} class + the {@link ActeonClient} methods via an
 * in-process {@link HttpServer}. The contract under test:
 * factories produce the dict shapes the server expects, URLs are
 * spec-correct, and the {@code A2A-Version} header lands on every
 * authenticated call.
 */
class A2ATest {
    private static final ObjectMapper MAPPER = JsonMapper.build();

    // ----------------------------------------------------------------
    // Factory helpers
    // ----------------------------------------------------------------

    @Test
    void makePartText() {
        assertEquals("hi", A2A.makePartText("hi").get("text"));
    }

    @Test
    void makePartUrl() {
        assertEquals("https://x/y", A2A.makePartUrl("https://x/y").get("url"));
    }

    @Test
    void makePartDataDefaultsToJson() {
        Map<String, Object> p = A2A.makePartData(Map.of("k", 1), null);
        assertEquals("application/json", p.get("mediaType"));
    }

    @Test
    void makePartDataHonorsCustomMediaType() {
        Map<String, Object> p = A2A.makePartData(null, "application/cloudevents+json");
        assertEquals("application/cloudevents+json", p.get("mediaType"));
    }

    @Test
    void makeMessageMinimalOmitsTaskIdAndContextId() {
        Map<String, Object> m = A2A.makeMessage(
            "m-1", "user", List.of(A2A.makePartText("hi")));
        assertEquals("m-1", m.get("messageId"));
        assertEquals("user", m.get("role"));
        // Absent vs. empty matters server-side — the helper must
        // not populate either key when omitted.
        assertFalse(m.containsKey("taskId"));
        assertFalse(m.containsKey("contextId"));
    }

    @Test
    void makeMessageThreadsTaskId() {
        Map<String, Object> m = A2A.makeMessage(
            "m-2",
            "user",
            List.of(A2A.makePartText("yes")),
            new A2A.MessageOptions().taskId("task-alpha")
        );
        assertEquals("task-alpha", m.get("taskId"));
    }

    @Test
    void makeInputResponseBindsChallenge() {
        Map<String, Object> m = A2A.makeInputResponse(
            "response-1",
            "task-alpha",
            "incident-42",
            "challenge-1",
            List.of(A2A.makePartData(Map.of("region", "us-west-2"), null))
        );
        assertEquals("user", m.get("role"));
        assertEquals("task-alpha", m.get("taskId"));
        assertEquals("incident-42", m.get("contextId"));
        assertEquals(Map.of("acteon.challengeId", "challenge-1"), m.get("metadata"));
    }

    @Test
    void makePushConfigMinimal() {
        Map<String, Object> cfg = A2A.makePushConfig("https://hook/x");
        assertEquals("https://hook/x", cfg.get("url"));
        assertEquals(1, cfg.size());
    }

    @Test
    void makePushConfigSupportsHostOwnedSecretReferences() {
        Map<String, Object> cfg = A2A.makePushConfig(
            "https://hook/x",
            new A2A.PushConfigOptions()
                .id("cfg-1")
                .tokenRef(new SecretRef("environment", "callbacks/token", "v2"))
                .authentication(new A2A.PushAuthenticationOptions(
                    List.of("Bearer"), null, new SecretRef("environment", "callbacks/auth", "v2")))
        );
        assertEquals("cfg-1", cfg.get("id"));
        assertEquals("callbacks/token", ((SecretRef) cfg.get("tokenRef")).key());
        assertEquals(List.of("Bearer"), ((Map<?, ?>) cfg.get("authentication")).get("schemes"));
        assertEquals("callbacks/auth", ((SecretRef) ((Map<?, ?>) cfg.get("authentication")).get("credentialsRef")).key());
    }

    @Test
    void makePushConfigRejectsInlineAndReferencedToken() {
        var opts = new A2A.PushConfigOptions()
            .token("inline")
            .tokenRef(new SecretRef("environment", "callbacks/token", null));
        assertThrows(IllegalArgumentException.class, () -> A2A.makePushConfig("https://hook/x", opts));
    }

    // ----------------------------------------------------------------
    // Client URLs + headers via in-process HttpServer
    // ----------------------------------------------------------------

    /** Records one inbound request for test assertions. */
    private static final class Captured {
        String method;
        String path;
        Map<String, List<String>> headers;
        String body;
    }

    /** Spin up an in-process HttpServer that answers every request
     *  with the supplied status + JSON body, recording the inbound
     *  request into the returned reference. */
    private static HttpServer startCapturing(
        int status, Object responseBody, AtomicReference<Captured> sink
    ) throws IOException {
        HttpServer server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        server.createContext("/", (HttpExchange ex) -> {
            Captured cap = new Captured();
            cap.method = ex.getRequestMethod();
            // getRequestURI().getRawPath() preserves percent-escapes;
            // getPath() would have already decoded them, which would
            // hide whether the client encoded reserved characters.
            cap.path = ex.getRequestURI().getRawPath();
            cap.headers = new HashMap<>(ex.getRequestHeaders());
            cap.body = new String(ex.getRequestBody().readAllBytes(), StandardCharsets.UTF_8);
            sink.set(cap);
            byte[] payload = responseBody == null
                ? new byte[0]
                : MAPPER.writeValueAsBytes(responseBody);
            ex.getResponseHeaders().add("Content-Type", "application/json");
            ex.sendResponseHeaders(status, payload.length);
            if (payload.length > 0) {
                ex.getResponseBody().write(payload);
            }
            ex.close();
        });
        server.start();
        return server;
    }

    private static String firstHeader(Map<String, List<String>> headers, String name) {
        // HTTP headers are case-insensitive; the JDK server
        // normalizes inbound names to Camel-Hyphen so we compare
        // case-insensitively.
        for (var e : headers.entrySet()) {
            if (e.getKey().equalsIgnoreCase(name)) {
                List<String> vs = e.getValue();
                return vs.isEmpty() ? null : vs.get(0);
            }
        }
        return null;
    }

    private static String urlOf(HttpServer server) {
        return "http://127.0.0.1:" + server.getAddress().getPort();
    }

    @Test
    void a2aSendMessageUrlAndA2aVersionHeader() throws Exception {
        AtomicReference<Captured> sink = new AtomicReference<>();
        Map<String, Object> body = new HashMap<>();
        body.put("id", "task-1");
        body.put("status", Map.of("state", "submitted"));
        HttpServer server = startCapturing(200, body, sink);
        try {
            ActeonClient c = new ActeonClient(urlOf(server), "k");
            Map<String, Object> msg = A2A.makeMessage(
                "m-1", "user", List.of(A2A.makePartText("hi")));
            c.a2aSendMessage(msg);
            Captured cap = sink.get();
            assertNotNull(cap);
            assertEquals("POST", cap.method);
            assertEquals("/a2a/v1/message:send", cap.path);
            assertEquals(A2A.PROTOCOL_VERSION,
                firstHeader(cap.headers, A2A.VERSION_HEADER));
            assertEquals("Bearer k", firstHeader(cap.headers, "Authorization"));
            // Body must wrap message in {"message": ...} per spec.
            @SuppressWarnings("unchecked")
            Map<String, Object> bodyMap = MAPPER.readValue(cap.body, Map.class);
            assertTrue(bodyMap.containsKey("message"));
        } finally {
            server.stop(0);
        }
    }

    @Test
    void a2aCancelTaskKeepsCancelVerbInSegment() throws Exception {
        AtomicReference<Captured> sink = new AtomicReference<>();
        HttpServer server = startCapturing(200,
            Map.of("id", "task-1", "status", Map.of("state", "canceled")),
            sink);
        try {
            ActeonClient c = new ActeonClient(urlOf(server));
            c.a2aCancelTask("task-1");
            assertEquals("/a2a/v1/tasks/task-1:cancel", sink.get().path);
            assertEquals("POST", sink.get().method);
        } finally {
            server.stop(0);
        }
    }

    @Test
    void a2aDeletePushConfigUrl() throws Exception {
        AtomicReference<Captured> sink = new AtomicReference<>();
        HttpServer server = startCapturing(200, null, sink);
        try {
            ActeonClient c = new ActeonClient(urlOf(server));
            c.a2aDeletePushConfig("task-1", "cfg-a");
            assertEquals(
                "/a2a/v1/tasks/task-1/pushNotificationConfigs/cfg-a",
                sink.get().path);
            assertEquals("DELETE", sink.get().method);
        } finally {
            server.stop(0);
        }
    }

    @Test
    void a2aDiscoverAgentUsesCredentialContext() throws Exception {
        AtomicReference<Captured> sink = new AtomicReference<>();
        HttpServer server = startCapturing(200,
            Map.of("agent_id", "tenant"), sink);
        try {
            // Discovery is tenant-owned and carries the configured credential.
            ActeonClient c = new ActeonClient(urlOf(server), "k");
            c.a2aDiscoverAgent();
            assertEquals("/a2a/.well-known/agent.json", sink.get().path);
            assertEquals("Bearer k", firstHeader(sink.get().headers, "Authorization"));
        } finally {
            server.stop(0);
        }
    }

    @Test
    void a2aGetAuthenticatedExtendedCardUsesJsonRpcEnvelope() throws Exception {
        AtomicReference<Captured> sink = new AtomicReference<>();
        Map<String, Object> jsonRpcReply = new HashMap<>();
        jsonRpcReply.put("jsonrpc", "2.0");
        jsonRpcReply.put("id", 1);
        jsonRpcReply.put("result", Map.of(
            "agent_id", "tenant",
            "capabilities", Map.of()));
        HttpServer server = startCapturing(200, jsonRpcReply, sink);
        try {
            ActeonClient c = new ActeonClient(urlOf(server), "k");
            Map<String, Object> card =
                c.a2aGetAuthenticatedExtendedCard();
            assertEquals("/a2a", sink.get().path);
            @SuppressWarnings("unchecked")
            Map<String, Object> bodyMap = MAPPER.readValue(sink.get().body, Map.class);
            assertEquals("agent/getAuthenticatedExtendedCard", bodyMap.get("method"));
            // The mixin unwraps the JSON-RPC envelope on the way out.
            assertEquals("tenant", card.get("agent_id"));
        } finally {
            server.stop(0);
        }
    }

    @Test
    void a2aGetAuthenticatedExtendedCardJsonRpcErrorSurfacesAsApiException() throws Exception {
        AtomicReference<Captured> sink = new AtomicReference<>();
        Map<String, Object> jsonRpcReply = new HashMap<>();
        jsonRpcReply.put("jsonrpc", "2.0");
        jsonRpcReply.put("id", 1);
        jsonRpcReply.put("error", Map.of("code", -32001, "message", "task not found"));
        HttpServer server = startCapturing(200, jsonRpcReply, sink);
        try {
            ActeonClient c = new ActeonClient(urlOf(server), "k");
            ApiException ex = assertThrows(ApiException.class,
                () -> c.a2aGetAuthenticatedExtendedCard());
            assertTrue(ex.getMessage().contains("task not found"),
                "got: " + ex.getMessage());
        } finally {
            server.stop(0);
        }
    }

    @Test
    void a2aPathSegmentsArePercentEncoded() throws Exception {
        AtomicReference<Captured> sink = new AtomicReference<>();
        HttpServer server = startCapturing(200, new HashMap<String, Object>(), sink);
        try {
            ActeonClient c = new ActeonClient(urlOf(server));
            // A task id with a slash must be percent-encoded so
            // it cannot leak into additional path components.
            c.a2aGetTask("task/escape");
            assertTrue(sink.get().path.contains("/tasks/task%2Fescape"),
                "path must percent-encode slash; got: " + sink.get().path);
        } finally {
            server.stop(0);
        }
    }
}
