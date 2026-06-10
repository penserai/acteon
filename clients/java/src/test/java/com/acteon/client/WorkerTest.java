package com.acteon.client;

import com.acteon.client.exceptions.NonRetryableException;
import com.fasterxml.jackson.core.type.TypeReference;
import com.fasterxml.jackson.databind.ObjectMapper;
import org.junit.jupiter.api.Test;

import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpServer;
import java.io.IOException;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.time.Instant;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Java SDK polling Worker tests.
 *
 * <p>These tests drive the {@link Worker} against a stateful
 * in-process HttpServer mock of the {@code /v1/queues} surface,
 * mirroring the Go/Python/Node SDK worker tests. The contract under
 * test: poll → handle → complete on success; handler exceptions fail
 * with retryable=true; {@link NonRetryableException} fails with
 * retryable=false; unregistered action types fail retryable; slow
 * handlers get automatic heartbeats; run() exits cleanly on stop().
 */
class WorkerTest {
    private static final ObjectMapper MAPPER = JsonMapper.build();

    /**
     * Stateful mock queue server: serves {@code firstBatch} on the
     * first poll and an empty batch on every poll after, and records
     * every poll / heartbeat / complete / fail body it receives.
     */
    static final class QueueMock implements AutoCloseable {
        final HttpServer server;
        final AtomicInteger pollCount = new AtomicInteger(0);
        final List<Map<String, Object>> pollBodies = Collections.synchronizedList(new ArrayList<>());
        final List<Map<String, Object>> heartbeats = Collections.synchronizedList(new ArrayList<>());
        final List<Map<String, Object>> completes = Collections.synchronizedList(new ArrayList<>());
        final List<Map<String, Object>> fails = Collections.synchronizedList(new ArrayList<>());
        private final List<Map<String, Object>> firstBatch;

        QueueMock(List<Map<String, Object>> firstBatch) throws IOException {
            this.firstBatch = firstBatch;
            this.server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
            this.server.createContext("/", this::handle);
            // Heartbeats overlap with the handler's settle calls; a
            // multi-threaded executor keeps them from serializing.
            this.server.setExecutor(Executors.newCachedThreadPool());
            this.server.start();
        }

        String url() {
            return "http://127.0.0.1:" + server.getAddress().getPort();
        }

        private void handle(HttpExchange exchange) throws IOException {
            String path = exchange.getRequestURI().getPath();
            byte[] raw = exchange.getRequestBody().readAllBytes();
            Map<String, Object> body = raw.length == 0
                ? Map.of()
                : MAPPER.readValue(raw, new TypeReference<Map<String, Object>>() {});

            Object resp;
            int status = 200;
            if (path.endsWith("/poll")) {
                pollBodies.add(body);
                List<Map<String, Object>> batch =
                    pollCount.incrementAndGet() == 1 ? firstBatch : List.of();
                resp = Map.of("tasks", batch);
            } else if (path.endsWith("/heartbeat")) {
                heartbeats.add(body);
                resp = taskWire("hb", "x", "lease", "leased");
            } else if (path.endsWith("/complete")) {
                completes.add(body);
                resp = taskWire("done", "x", "lease", "completed");
            } else if (path.endsWith("/fail")) {
                fails.add(body);
                resp = taskWire("failed", "x", "lease", "failed");
            } else {
                status = 404;
                resp = Map.of("error", "not found");
            }
            byte[] respBody = MAPPER.writeValueAsBytes(resp);
            exchange.getResponseHeaders().set("Content-Type", "application/json");
            exchange.sendResponseHeaders(status, respBody.length);
            exchange.getResponseBody().write(respBody);
            exchange.close();
        }

        @Override
        public void close() {
            server.stop(0);
        }
    }

    /** Builds a wire-form task for mock responses. */
    static Map<String, Object> taskWire(String taskId, String actionType, String leaseToken, String status) {
        Map<String, Object> wire = new HashMap<>();
        wire.put("task_id", taskId);
        wire.put("queue", "q");
        wire.put("action_type", actionType);
        wire.put("payload", Map.of());
        wire.put("status", status);
        wire.put("attempt", 1);
        wire.put("max_attempts", 3);
        wire.put("lease_token", leaseToken);
        wire.put("lease_expires_at", Instant.now().plusSeconds(60).toString());
        wire.put("created_at", Instant.now().toString());
        wire.put("updated_at", Instant.now().toString());
        return wire;
    }

    /** Builds a leased wire-form task with a payload for mock poll responses. */
    static Map<String, Object> leasedTaskWire(String taskId, String actionType, String leaseToken,
                                              Map<String, Object> payload) {
        Map<String, Object> wire = taskWire(taskId, actionType, leaseToken, "leased");
        wire.put("payload", payload);
        return wire;
    }

    private static Worker.Builder testWorkerBuilder(ActeonClient client) {
        return Worker.builder(client, "ns", "tnt", "q").workerId("w-1");
    }

    // -------------------------------------------------------------------------
    // Construction
    // -------------------------------------------------------------------------

    @Test
    void builderRequiresScope() {
        try (ActeonClient client = new ActeonClient("http://localhost")) {
            assertThrows(IllegalArgumentException.class,
                () -> Worker.builder(client, "", "tnt", "q"));
            assertThrows(IllegalArgumentException.class,
                () -> Worker.builder(client, "ns", null, "q"));
            assertThrows(IllegalArgumentException.class,
                () -> Worker.builder(client, "ns", "tnt", ""));
        }
    }

    @Test
    void builderGeneratesWorkerIdWhenOmitted() {
        try (ActeonClient client = new ActeonClient("http://localhost");
             Worker worker = Worker.builder(client, "ns", "tnt", "q").build()) {
            assertTrue(worker.getWorkerId().startsWith("worker-"), worker.getWorkerId());
        }
    }

    // -------------------------------------------------------------------------
    // Dispatch + settlement
    // -------------------------------------------------------------------------

    @Test
    void happyPathCompletesWithHandlerResult() throws Exception {
        try (QueueMock mock = new QueueMock(List.of(
                leasedTaskWire("t-1", "send_email", "lease-1", Map.of("to", "a@b.c"))));
             ActeonClient client = new ActeonClient(mock.url());
             Worker worker = testWorkerBuilder(client).build()) {

            AtomicReference<String> gotTo = new AtomicReference<>();
            worker.register("send_email", payload -> {
                gotTo.set(payload.get("to").asText());
                return Map.of("sent", true);
            });

            int n = worker.runOnce();
            assertEquals(1, n);
            assertEquals("a@b.c", gotTo.get());

            assertEquals(0, mock.fails.size(), mock.fails.toString());
            assertEquals(1, mock.completes.size());
            Map<String, Object> complete = mock.completes.get(0);
            assertEquals("lease-1", complete.get("lease_token"));
            assertEquals("ns", complete.get("namespace"));
            assertEquals("tnt", complete.get("tenant"));
            @SuppressWarnings("unchecked")
            Map<String, Object> result = (Map<String, Object>) complete.get("result");
            assertEquals(Boolean.TRUE, result.get("sent"));

            // Poll body must carry the worker's scope, identity, and
            // the documented defaults (max_tasks=1, lease_seconds=60).
            Map<String, Object> poll = mock.pollBodies.get(0);
            assertEquals("ns", poll.get("namespace"));
            assertEquals("tnt", poll.get("tenant"));
            assertEquals("w-1", poll.get("worker_id"));
            assertEquals(1, poll.get("max_tasks"));
            assertEquals(60, poll.get("lease_seconds"));
        }
    }

    @Test
    void handlerExceptionFailsRetryable() throws Exception {
        try (QueueMock mock = new QueueMock(List.of(
                leasedTaskWire("t-1", "send_email", "lease-1", Map.of())));
             ActeonClient client = new ActeonClient(mock.url());
             Worker worker = testWorkerBuilder(client).build()) {

            worker.register("send_email", payload -> {
                throw new RuntimeException("smtp timeout");
            });

            assertEquals(1, worker.runOnce());

            assertEquals(0, mock.completes.size(), mock.completes.toString());
            assertEquals(1, mock.fails.size());
            Map<String, Object> fail = mock.fails.get(0);
            assertEquals("smtp timeout", fail.get("error"));
            assertEquals("lease-1", fail.get("lease_token"));
            assertEquals(Boolean.TRUE, fail.get("retryable"),
                "plain handler exceptions must fail retryable");
        }
    }

    @Test
    void nonRetryableExceptionFailsTerminal() throws Exception {
        try (QueueMock mock = new QueueMock(List.of(
                leasedTaskWire("t-1", "send_email", "lease-1", Map.of())));
             ActeonClient client = new ActeonClient(mock.url());
             Worker worker = testWorkerBuilder(client).build()) {

            worker.register("send_email", payload -> {
                throw new NonRetryableException("bad address");
            });

            assertEquals(1, worker.runOnce());

            assertEquals(1, mock.fails.size());
            Map<String, Object> fail = mock.fails.get(0);
            assertEquals(Boolean.FALSE, fail.get("retryable"),
                "NonRetryableException must fail terminal");
            assertTrue(((String) fail.get("error")).contains("bad address"), fail.toString());
        }
    }

    @Test
    void unregisteredActionTypeFailsRetryable() throws Exception {
        try (QueueMock mock = new QueueMock(List.of(
                leasedTaskWire("t-1", "unknown_type", "lease-1", Map.of())));
             ActeonClient client = new ActeonClient(mock.url());
             Worker worker = testWorkerBuilder(client).build()) {

            worker.register("send_email", payload -> null);

            assertEquals(1, worker.runOnce());

            assertEquals(1, mock.fails.size());
            Map<String, Object> fail = mock.fails.get(0);
            assertEquals(Boolean.TRUE, fail.get("retryable"),
                "missing-handler failures must stay retryable");
            assertTrue(((String) fail.get("error")).contains("unknown_type"),
                "fail error must name the action type: " + fail.get("error"));
        }
    }

    // -------------------------------------------------------------------------
    // Heartbeats
    // -------------------------------------------------------------------------

    @Test
    void heartbeatsFireForSlowHandler() throws Exception {
        try (QueueMock mock = new QueueMock(List.of(
                leasedTaskWire("t-1", "slow", "lease-1", Map.of())));
             ActeonClient client = new ActeonClient(mock.url());
             // Shrink the heartbeat cadence so the test stays fast;
             // the production cadence is leaseSeconds/2.
             Worker worker = testWorkerBuilder(client)
                 .heartbeatInterval(Duration.ofMillis(20))
                 .build()) {

            worker.register("slow", payload -> {
                Thread.sleep(150);
                return Map.of("ok", true);
            });

            assertEquals(1, worker.runOnce());

            assertEquals(0, mock.fails.size(), mock.fails.toString());
            assertEquals(1, mock.completes.size());
            assertFalse(mock.heartbeats.isEmpty(),
                "expected at least one heartbeat for a slow handler");
            Map<String, Object> hb = mock.heartbeats.get(0);
            assertEquals("lease-1", hb.get("lease_token"));
            // extend_seconds must re-request the configured lease (default 60).
            assertEquals(60, hb.get("extend_seconds"));
        }
    }

    @Test
    void fastHandlerSendsNoHeartbeat() throws Exception {
        try (QueueMock mock = new QueueMock(List.of(
                leasedTaskWire("t-1", "fast", "lease-1", Map.of())));
             ActeonClient client = new ActeonClient(mock.url());
             Worker worker = testWorkerBuilder(client).build()) {

            worker.register("fast", payload -> null);

            assertEquals(1, worker.runOnce());

            // leaseSeconds defaults to 60 → first heartbeat at 30s; a
            // fast handler finishes well before the schedule fires.
            assertEquals(0, mock.heartbeats.size(), mock.heartbeats.toString());
            assertEquals(1, mock.completes.size());
        }
    }

    // -------------------------------------------------------------------------
    // Run loop + stop
    // -------------------------------------------------------------------------

    @Test
    void runProcessesTasksAndStopsCleanly() throws Exception {
        try (QueueMock mock = new QueueMock(List.of(
                leasedTaskWire("t-1", "send_email", "lease-1", Map.of())));
             ActeonClient client = new ActeonClient(mock.url());
             Worker worker = testWorkerBuilder(client)
                 .pollInterval(Duration.ofMillis(10))
                 .build()) {

            worker.register("send_email", payload -> Map.of("sent", true));

            Thread runner = new Thread(worker::run, "worker-run-test");
            runner.start();

            // Let the loop drain the first batch and idle-poll a few times.
            Thread.sleep(150);
            worker.stop();
            runner.join(2_000);
            assertFalse(runner.isAlive(), "run() did not return after stop()");

            assertEquals(1, mock.completes.size());
            assertTrue(mock.pollCount.get() >= 2,
                "run() must keep polling after an empty batch: got " + mock.pollCount.get());
        }
    }

    @Test
    void runOncePollFailureSurfacesAsException() throws Exception {
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
            try (ActeonClient client = new ActeonClient("http://127.0.0.1:" + port);
                 Worker worker = Worker.builder(client, "ns", "tnt", "q").build()) {
                assertThrows(com.acteon.client.exceptions.ActeonException.class, worker::runOnce);
            }
        } finally {
            server.stop(0);
        }
    }
}
