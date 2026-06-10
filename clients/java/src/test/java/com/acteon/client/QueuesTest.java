package com.acteon.client;

import com.acteon.client.exceptions.ApiException;
import com.fasterxml.jackson.databind.ObjectMapper;
import org.junit.jupiter.api.Test;

import com.sun.net.httpserver.HttpServer;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Java SDK worker task-queue surface tests.
 *
 * <p>Mirrors the Go/Python/Node SDK queue tests: wire-level serde
 * (request/response round-trip) plus a small in-process HttpServer
 * that asserts the SDK builds the right paths, query strings, and
 * 404 handling for the {@code /v1/queues} surface.
 */
class QueuesTest {
    private static final ObjectMapper MAPPER = JsonMapper.build();

    private static final String TASK_WIRE = """
        {
          "task_id": "t-1", "queue": "emails", "action_type": "send_email",
          "payload": {"to": "a@b.c"}, "status": "leased",
          "attempt": 1, "max_attempts": 3,
          "lease_token": "lease-1",
          "lease_expires_at": "2026-01-01T00:01:00Z",
          "chain_id": "chain-9",
          "created_at": "2026-01-01T00:00:00Z",
          "updated_at": "2026-01-01T00:00:30Z"
        }
        """;

    // -------------------------------------------------------------------------
    // Request body serde
    // -------------------------------------------------------------------------

    @Test
    void enqueueRequestMinimalDropsMaxAttempts() throws Exception {
        Queues.EnqueueTaskRequest req = new Queues.EnqueueTaskRequest(
            "ns", "tnt", "send_email", Map.of("to", "a@b.c"));
        String json = MAPPER.writeValueAsString(req);
        assertFalse(json.contains("max_attempts"), json);
        assertTrue(json.contains("\"action_type\":\"send_email\""), json);
        assertTrue(json.contains("\"payload\":{\"to\":\"a@b.c\"}"), json);
    }

    @Test
    void enqueueRequestFullSnakeCasesMaxAttempts() throws Exception {
        Queues.EnqueueTaskRequest req = new Queues.EnqueueTaskRequest(
            "ns", "tnt", "send_email", Map.of(), 5);
        String json = MAPPER.writeValueAsString(req);
        assertTrue(json.contains("\"max_attempts\":5"), json);
    }

    @Test
    void pollRequestSnakeCasesOptionalFields() throws Exception {
        Queues.PollTasksRequest req = new Queues.PollTasksRequest(
            "ns", "tnt", 4, 120, "w-1");
        String json = MAPPER.writeValueAsString(req);
        assertTrue(json.contains("\"max_tasks\":4"), json);
        assertTrue(json.contains("\"lease_seconds\":120"), json);
        assertTrue(json.contains("\"worker_id\":\"w-1\""), json);
    }

    @Test
    void pollRequestMinimalDropsOptionalFields() throws Exception {
        Queues.PollTasksRequest req = new Queues.PollTasksRequest("ns", "tnt");
        String json = MAPPER.writeValueAsString(req);
        assertEquals("{\"namespace\":\"ns\",\"tenant\":\"tnt\"}", json);
    }

    @Test
    void heartbeatRequestSerde() throws Exception {
        Queues.HeartbeatTaskRequest req = new Queues.HeartbeatTaskRequest(
            "ns", "tnt", "lease-1", 90);
        String json = MAPPER.writeValueAsString(req);
        assertTrue(json.contains("\"lease_token\":\"lease-1\""), json);
        assertTrue(json.contains("\"extend_seconds\":90"), json);
    }

    @Test
    void failRequestAlwaysCarriesRetryable() throws Exception {
        Queues.FailTaskRequest req = new Queues.FailTaskRequest(
            "ns", "tnt", "lease-1", "smtp timeout", false);
        String json = MAPPER.writeValueAsString(req);
        assertTrue(json.contains("\"error\":\"smtp timeout\""), json);
        assertTrue(json.contains("\"retryable\":false"), json);
    }

    // -------------------------------------------------------------------------
    // Response body deserialization
    // -------------------------------------------------------------------------

    @Test
    void workerTaskRoundTrip() throws Exception {
        Queues.WorkerTask t = MAPPER.readValue(TASK_WIRE, Queues.WorkerTask.class);
        assertEquals("t-1", t.taskId());
        assertEquals("emails", t.queue());
        assertEquals("send_email", t.actionType());
        assertEquals("a@b.c", t.payload().get("to").asText());
        assertEquals(Queues.TASK_STATUS_LEASED, t.status());
        assertEquals(1, t.attempt());
        assertEquals(3, t.maxAttempts());
        assertEquals("lease-1", t.leaseToken());
        assertEquals("2026-01-01T00:01:00Z", t.leaseExpiresAt());
        assertEquals("chain-9", t.chainId());
        // Server omits these when unset; record fields stay null.
        assertNull(t.result());
        assertNull(t.error());
        assertNull(t.workflowExecutionId());
    }

    // -------------------------------------------------------------------------
    // Server-driven tests (HttpServer)
    // -------------------------------------------------------------------------

    @Test
    void enqueueTaskPostsBodyAndParses201() throws Exception {
        HttpServer server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        AtomicReference<String> seenPath = new AtomicReference<>();
        AtomicReference<String> seenMethod = new AtomicReference<>();
        AtomicReference<String> seenBody = new AtomicReference<>();
        server.createContext("/", exchange -> {
            seenPath.set(exchange.getRequestURI().getPath());
            seenMethod.set(exchange.getRequestMethod());
            seenBody.set(new String(exchange.getRequestBody().readAllBytes(), StandardCharsets.UTF_8));
            byte[] respBody = TASK_WIRE.replace("\"leased\"", "\"pending\"")
                .getBytes(StandardCharsets.UTF_8);
            exchange.getResponseHeaders().set("Content-Type", "application/json");
            exchange.sendResponseHeaders(201, respBody.length);
            exchange.getResponseBody().write(respBody);
            exchange.close();
        });
        server.start();
        try {
            int port = server.getAddress().getPort();
            try (ActeonClient client = new ActeonClient("http://127.0.0.1:" + port)) {
                Queues.WorkerTask t = client.enqueueTask("emails",
                    new Queues.EnqueueTaskRequest("ns", "tnt", "send_email", Map.of("to", "a@b.c")));
                assertEquals(Queues.TASK_STATUS_PENDING, t.status());
                assertEquals("t-1", t.taskId());
            }
            assertEquals("POST", seenMethod.get());
            assertEquals("/v1/queues/emails/tasks", seenPath.get());
            assertTrue(seenBody.get().contains("\"action_type\":\"send_email\""), seenBody.get());
        } finally {
            server.stop(0);
        }
    }

    @Test
    void pollTasksParsesBatchWithLeaseTokens() throws Exception {
        HttpServer server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        AtomicReference<String> seenPath = new AtomicReference<>();
        server.createContext("/", exchange -> {
            seenPath.set(exchange.getRequestURI().getPath());
            byte[] respBody = ("{\"tasks\":[" + TASK_WIRE + "]}").getBytes(StandardCharsets.UTF_8);
            exchange.getResponseHeaders().set("Content-Type", "application/json");
            exchange.sendResponseHeaders(200, respBody.length);
            exchange.getResponseBody().write(respBody);
            exchange.close();
        });
        server.start();
        try {
            int port = server.getAddress().getPort();
            try (ActeonClient client = new ActeonClient("http://127.0.0.1:" + port)) {
                List<Queues.WorkerTask> tasks = client.pollTasks("emails",
                    new Queues.PollTasksRequest("ns", "tnt", 2, 60, "w-1"));
                assertEquals(1, tasks.size());
                assertEquals("lease-1", tasks.get(0).leaseToken());
            }
            assertEquals("/v1/queues/emails/poll", seenPath.get());
        } finally {
            server.stop(0);
        }
    }

    @Test
    void getTaskReturnsEmptyOn404() throws Exception {
        HttpServer server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        AtomicReference<String> seenQuery = new AtomicReference<>();
        server.createContext("/", exchange -> {
            seenQuery.set(exchange.getRequestURI().getRawQuery());
            byte[] respBody = "{\"error\":\"task not found\"}".getBytes(StandardCharsets.UTF_8);
            exchange.getResponseHeaders().set("Content-Type", "application/json");
            exchange.sendResponseHeaders(404, respBody.length);
            exchange.getResponseBody().write(respBody);
            exchange.close();
        });
        server.start();
        try {
            int port = server.getAddress().getPort();
            try (ActeonClient client = new ActeonClient("http://127.0.0.1:" + port)) {
                Optional<Queues.WorkerTask> t = client.getTask("missing", "ns", "tnt");
                assertTrue(t.isEmpty());
            }
            String q = seenQuery.get();
            assertNotNull(q);
            assertTrue(q.contains("namespace=ns"), q);
            assertTrue(q.contains("tenant=tnt"), q);
        } finally {
            server.stop(0);
        }
    }

    @Test
    void getTaskParsesTaskOn200() throws Exception {
        HttpServer server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        AtomicReference<String> seenPath = new AtomicReference<>();
        server.createContext("/", exchange -> {
            seenPath.set(exchange.getRequestURI().getPath());
            byte[] respBody = TASK_WIRE.getBytes(StandardCharsets.UTF_8);
            exchange.getResponseHeaders().set("Content-Type", "application/json");
            exchange.sendResponseHeaders(200, respBody.length);
            exchange.getResponseBody().write(respBody);
            exchange.close();
        });
        server.start();
        try {
            int port = server.getAddress().getPort();
            try (ActeonClient client = new ActeonClient("http://127.0.0.1:" + port)) {
                Optional<Queues.WorkerTask> t = client.getTask("t-1", "ns", "tnt");
                assertTrue(t.isPresent());
                assertEquals("t-1", t.get().taskId());
            }
            assertEquals("/v1/queues/tasks/t-1", seenPath.get());
        } finally {
            server.stop(0);
        }
    }

    @Test
    void listTasksBuildsQueryWithStatusFilter() throws Exception {
        HttpServer server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        AtomicReference<String> seenPath = new AtomicReference<>();
        AtomicReference<String> seenQuery = new AtomicReference<>();
        server.createContext("/", exchange -> {
            seenPath.set(exchange.getRequestURI().getPath());
            seenQuery.set(exchange.getRequestURI().getRawQuery());
            byte[] respBody = "{\"tasks\":[]}".getBytes(StandardCharsets.UTF_8);
            exchange.getResponseHeaders().set("Content-Type", "application/json");
            exchange.sendResponseHeaders(200, respBody.length);
            exchange.getResponseBody().write(respBody);
            exchange.close();
        });
        server.start();
        try {
            int port = server.getAddress().getPort();
            try (ActeonClient client = new ActeonClient("http://127.0.0.1:" + port)) {
                List<Queues.WorkerTask> tasks = client.listTasks(
                    "emails", "ns", "tnt", Queues.TASK_STATUS_PENDING);
                assertTrue(tasks.isEmpty());
            }
            assertEquals("/v1/queues/emails/tasks", seenPath.get());
            String q = seenQuery.get();
            assertTrue(q.contains("namespace=ns"), q);
            assertTrue(q.contains("tenant=tnt"), q);
            assertTrue(q.contains("status=pending"), q);
        } finally {
            server.stop(0);
        }
    }

    @Test
    void queueErrorBodyMapsToApiException() throws Exception {
        HttpServer server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        server.createContext("/", exchange -> {
            byte[] respBody = "{\"error\":\"lease token mismatch\"}".getBytes(StandardCharsets.UTF_8);
            exchange.getResponseHeaders().set("Content-Type", "application/json");
            exchange.sendResponseHeaders(409, respBody.length);
            exchange.getResponseBody().write(respBody);
            exchange.close();
        });
        server.start();
        try {
            int port = server.getAddress().getPort();
            try (ActeonClient client = new ActeonClient("http://127.0.0.1:" + port)) {
                ApiException ex = assertThrows(ApiException.class,
                    () -> client.completeTask("t-1", new Queues.CompleteTaskRequest(
                        "ns", "tnt", "stale-lease", Map.of())));
                assertTrue(ex.getMessage().contains("lease token mismatch"), ex.getMessage());
                // Conflicts are not transient — the lease is gone.
                assertFalse(ex.isRetryable());
            }
        } finally {
            server.stop(0);
        }
    }

    @Test
    void transientServerErrorIsRetryable() throws Exception {
        HttpServer server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        server.createContext("/", exchange -> {
            byte[] respBody = "{\"error\":\"backend unavailable\"}".getBytes(StandardCharsets.UTF_8);
            exchange.getResponseHeaders().set("Content-Type", "application/json");
            exchange.sendResponseHeaders(503, respBody.length);
            exchange.getResponseBody().write(respBody);
            exchange.close();
        });
        server.start();
        try {
            int port = server.getAddress().getPort();
            try (ActeonClient client = new ActeonClient("http://127.0.0.1:" + port)) {
                ApiException ex = assertThrows(ApiException.class,
                    () -> client.pollTasks("emails", new Queues.PollTasksRequest("ns", "tnt")));
                assertTrue(ex.isRetryable());
            }
        } finally {
            server.stop(0);
        }
    }
}
