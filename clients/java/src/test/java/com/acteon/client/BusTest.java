package com.acteon.client;

import com.fasterxml.jackson.databind.ObjectMapper;
import org.junit.jupiter.api.Test;

import com.sun.net.httpserver.HttpServer;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.Map;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Phase 8d: Java SDK bus surface tests.
 *
 * Live HTTP tests would need a running Acteon instance with the
 * bus feature enabled; these tests exercise wire-level serde
 * (request/response round-trip) plus a small in-process HttpServer
 * that asserts the SDK builds the right paths and branches the
 * 6c approval gate on 200 vs 202.
 */
class BusTest {
    private static final ObjectMapper MAPPER = JsonMapper.build();

    // -------------------------------------------------------------------------
    // Request body serde
    // -------------------------------------------------------------------------

    @Test
    void createTopicMinimalDropsOptionalFields() throws Exception {
        Bus.CreateBusTopic req = new Bus.CreateBusTopic("t", "n", "te");
        String json = MAPPER.writeValueAsString(req);
        // `@JsonInclude(NON_NULL)` on the record drops every nullable
        // optional from the wire form.
        assertFalse(json.contains("\"partitions\""), json);
        assertFalse(json.contains("\"replication_factor\""), json);
        assertFalse(json.contains("\"labels\""), json);
        assertTrue(json.contains("\"name\":\"t\""));
    }

    @Test
    void createTopicSnakeCasesOptionalFields() throws Exception {
        Bus.CreateBusTopic req = new Bus.CreateBusTopic(
            "t", "n", "te", 4, 2, 86_400_000L, "demo", Map.of("env", "prod"));
        String json = MAPPER.writeValueAsString(req);
        assertTrue(json.contains("\"replication_factor\":2"), json);
        assertTrue(json.contains("\"retention_ms\":86400000"), json);
        assertTrue(json.contains("\"labels\":{\"env\":\"prod\"}"), json);
    }

    @Test
    void postBusToolCallBasicHasNoApprovalGate() throws Exception {
        Bus.PostBusToolCall req = new Bus.PostBusToolCall(
            "call-1", "calendar.list", Map.of());
        String json = MAPPER.writeValueAsString(req);
        // Default `requireApproval=false` is dropped via NON_DEFAULT.
        assertFalse(json.contains("require_approval"), json);
    }

    @Test
    void postBusToolCallWithApprovalGate() throws Exception {
        Bus.PostBusToolCall req = new Bus.PostBusToolCall(
            "call-1", "billing.charge", Map.of("usd", 42),
            null, null, "planner-1", null,
            true, "paid action", 600_000L);
        String json = MAPPER.writeValueAsString(req);
        assertTrue(json.contains("\"require_approval\":true"), json);
        assertTrue(json.contains("\"approval_reason\":\"paid action\""), json);
        assertTrue(json.contains("\"approval_ttl_ms\":600000"), json);
    }

    @Test
    void postBusToolResultErrorCase() throws Exception {
        Bus.PostBusToolResult req = new Bus.PostBusToolResult(
            "call-1", "error", Map.of(),
            "upstream gave up", null, "calendar-svc", null);
        String json = MAPPER.writeValueAsString(req);
        assertTrue(json.contains("\"status\":\"error\""), json);
        assertTrue(json.contains("\"error_message\":\"upstream gave up\""), json);
    }

    @Test
    void postBusStreamChunkSerde() throws Exception {
        Bus.PostBusStreamChunk req = new Bus.PostBusStreamChunk(
            "s1", 0L, Map.of("token", "Once "));
        String json = MAPPER.writeValueAsString(req);
        assertTrue(json.contains("\"stream_id\":\"s1\""), json);
        assertTrue(json.contains("\"chunk_seq\":0"), json);
        assertTrue(json.contains("\"body\":{\"token\":\"Once \"}"), json);
    }

    @Test
    void busApprovalDecisionSerde() throws Exception {
        Bus.BusApprovalDecision d = new Bus.BusApprovalDecision("ops-1", "verified PO");
        String json = MAPPER.writeValueAsString(d);
        assertTrue(json.contains("\"decided_by\":\"ops-1\""), json);
        assertTrue(json.contains("\"decision_note\":\"verified PO\""), json);
    }

    // -------------------------------------------------------------------------
    // Response body deserialization
    // -------------------------------------------------------------------------

    @Test
    void busTopicResponseRoundTrip() throws Exception {
        String body = """
            {
              "name": "t", "namespace": "n", "tenant": "te",
              "kafka_name": "n.te.t", "partitions": 4, "replication_factor": 2,
              "created_at": "2026-01-01T00:00:00Z",
              "updated_at": "2026-01-01T00:00:00Z"
            }
            """;
        Bus.BusTopic t = MAPPER.readValue(body, Bus.BusTopic.class);
        assertEquals("n.te.t", t.kafkaName());
        // Server omits these when not bound; record fields stay null.
        assertNull(t.schemaSubject());
        assertNull(t.schemaVersion());
    }

    @Test
    void busLagResponseRoundTrip() throws Exception {
        String body = """
            {
              "subscription_id": "s1", "topic": "n.te.t",
              "partitions": [
                {"partition": 0, "committed": 10, "high_water_mark": 12, "lag": 2},
                {"partition": 1, "committed": 0, "high_water_mark": 0, "lag": 0}
              ],
              "total_lag": 2
            }
            """;
        Bus.BusLag lag = MAPPER.readValue(body, Bus.BusLag.class);
        assertEquals(2L, lag.totalLag());
        assertEquals(2, lag.partitions().size());
        assertEquals(12L, lag.partitions().get(0).highWaterMark());
    }

    @Test
    void busApprovalViewPendingHasNullDecisionFields() throws Exception {
        String body = """
            {
              "approval_id": "appr-1", "namespace": "n", "tenant": "te",
              "conversation_id": "c1", "correlation_token": "call-1",
              "envelope_kind": "tool_call", "status": "pending",
              "created_at": "2026-01-01T00:00:00Z",
              "expires_at": "2026-01-02T00:00:00Z",
              "envelope": {"kind": "tool_call"}
            }
            """;
        Bus.BusApprovalView v = MAPPER.readValue(body, Bus.BusApprovalView.class);
        assertEquals("pending", v.status());
        assertNull(v.decidedBy());
        assertNull(v.producedOffset());
    }

    @Test
    void busApprovalDecisionResponseApprovedWithReceipt() throws Exception {
        String body = """
            {
              "approval": {
                "approval_id": "appr-1", "namespace": "n", "tenant": "te",
                "conversation_id": "c1", "correlation_token": "call-1",
                "envelope_kind": "tool_call", "status": "approved",
                "created_at": "2026-01-01T00:00:00Z",
                "expires_at": "2026-01-02T00:00:00Z",
                "envelope": {},
                "decided_by": "ops-1"
              },
              "receipt": {
                "events_topic": "n.te.events",
                "conversation_id": "c1", "call_id": "call-1",
                "partition": 0, "offset": 99,
                "produced_at": "2026-01-01T00:00:01Z",
                "cursor": "xx"
              }
            }
            """;
        Bus.BusApprovalDecisionResponse r = MAPPER.readValue(body, Bus.BusApprovalDecisionResponse.class);
        assertEquals("approved", r.approval().status());
        assertNotNull(r.receipt());
        assertEquals(99L, r.receipt().offset());
    }

    @Test
    void busApprovalDecisionResponseRejectedHasNullReceipt() throws Exception {
        String body = """
            {
              "approval": {
                "approval_id": "appr-1", "namespace": "n", "tenant": "te",
                "conversation_id": "c1", "correlation_token": "call-1",
                "envelope_kind": "tool_call", "status": "rejected",
                "created_at": "2026-01-01T00:00:00Z",
                "expires_at": "2026-01-02T00:00:00Z",
                "envelope": {},
                "decided_by": "ops-1",
                "decision_note": "scope too broad"
              },
              "receipt": null
            }
            """;
        Bus.BusApprovalDecisionResponse r = MAPPER.readValue(body, Bus.BusApprovalDecisionResponse.class);
        assertEquals("rejected", r.approval().status());
        assertNull(r.receipt());
    }

    // -------------------------------------------------------------------------
    // SSE consume URL builder
    // -------------------------------------------------------------------------

    @Test
    void busStreamConsumeUrlSimple() {
        try (ActeonClient client = new ActeonClient("http://localhost:3000")) {
            String url = client.busStreamConsumeUrl("agents", "demo", "thread-1", "stream-1");
            assertEquals("http://localhost:3000/v1/bus/streams/agents/demo/thread-1/stream-1", url);
        }
    }

    @Test
    void busStreamConsumeUrlEncodesSlashesAndSpaces() {
        try (ActeonClient client = new ActeonClient("http://localhost:3000")) {
            String url = client.busStreamConsumeUrl("agents/x", "demo", "thread/with/slashes", "story 1");
            assertTrue(url.contains("agents%2Fx"), url);
            assertTrue(url.contains("thread%2Fwith%2Fslashes"), url);
            // Java's URLEncoder turns spaces into `+` for form-urlencoded
            // but the bus REST surface accepts both — the important
            // contract is that the space doesn't slip through raw.
            assertTrue(url.contains("story+1") || url.contains("story%20"), url);
        }
    }

    // -------------------------------------------------------------------------
    // Server-driven tests (HttpServer + sealed-interface branch)
    // -------------------------------------------------------------------------

    @Test
    void postBusToolCallBranchesOn202() throws Exception {
        HttpServer server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        AtomicReference<String> seenPath = new AtomicReference<>();
        server.createContext("/", exchange -> {
            seenPath.set(exchange.getRequestURI().getPath());
            String body = new String(exchange.getRequestBody().readAllBytes(), StandardCharsets.UTF_8);
            boolean wantsApproval = body.contains("\"require_approval\":true");
            byte[] respBody;
            int status;
            if (wantsApproval) {
                status = 202;
                respBody = """
                    {
                      "approval_id": "appr-1",
                      "namespace": "n", "tenant": "te",
                      "conversation_id": "c1",
                      "correlation_token": "call-1",
                      "status": "pending",
                      "created_at": "2026-01-01T00:00:00Z",
                      "expires_at": "2026-01-02T00:00:00Z"
                    }
                    """.getBytes(StandardCharsets.UTF_8);
            } else {
                status = 200;
                respBody = """
                    {
                      "events_topic": "n.te.events",
                      "conversation_id": "c1", "call_id": "call-1",
                      "partition": 0, "offset": 17,
                      "produced_at": "2026-01-01T00:00:00Z",
                      "cursor": "abc"
                    }
                    """.getBytes(StandardCharsets.UTF_8);
            }
            exchange.getResponseHeaders().set("Content-Type", "application/json");
            exchange.sendResponseHeaders(status, respBody.length);
            exchange.getResponseBody().write(respBody);
            exchange.close();
        });
        server.start();
        try {
            int port = server.getAddress().getPort();
            try (ActeonClient client = new ActeonClient("http://127.0.0.1:" + port)) {
                // Immediate produce path.
                Bus.PostBusToolCallOutcome produced = client.postBusToolCall("n", "te", "c1",
                    new Bus.PostBusToolCall("call-1", "calendar.list", Map.of()));
                assertFalse(produced.isParked());
                Bus.PostBusToolCallOutcome.Produced p = (Bus.PostBusToolCallOutcome.Produced) produced;
                assertEquals(17L, p.receipt().offset());

                // Approval gate path.
                Bus.PostBusToolCallOutcome parked = client.postBusToolCall("n", "te", "c1",
                    new Bus.PostBusToolCall(
                        "call-1", "billing.charge", Map.of("usd", 42),
                        null, null, null, null, true, "paid action", null));
                assertTrue(parked.isParked());
                Bus.PostBusToolCallOutcome.Parked pk = (Bus.PostBusToolCallOutcome.Parked) parked;
                assertEquals("appr-1", pk.receipt().approvalId());
            }
            assertNotNull(seenPath.get());
            assertTrue(seenPath.get().endsWith("/tool-calls"), seenPath.get());
        } finally {
            server.stop(0);
        }
    }

    @Test
    void busErrorBodyMapsToApiException() throws Exception {
        HttpServer server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        server.createContext("/", exchange -> {
            byte[] respBody = "{\"error\":\"sender 'alpha' is not a participant\"}".getBytes(StandardCharsets.UTF_8);
            exchange.sendResponseHeaders(400, respBody.length);
            exchange.getResponseBody().write(respBody);
            exchange.close();
        });
        server.start();
        try {
            int port = server.getAddress().getPort();
            try (ActeonClient client = new ActeonClient("http://127.0.0.1:" + port)) {
                com.acteon.client.exceptions.ApiException ex = assertThrows(
                    com.acteon.client.exceptions.ApiException.class,
                    () -> client.createBusTopic(new Bus.CreateBusTopic("t", "n", "te")));
                assertTrue(ex.getMessage().contains("sender"), ex.getMessage());
            }
        } finally {
            server.stop(0);
        }
    }

    @Test
    void approveBusApprovalRoutesAndDecodes() throws Exception {
        HttpServer server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        AtomicReference<String> seenPath = new AtomicReference<>();
        AtomicReference<String> seenMethod = new AtomicReference<>();
        server.createContext("/", exchange -> {
            seenPath.set(exchange.getRequestURI().getPath());
            seenMethod.set(exchange.getRequestMethod());
            byte[] respBody = """
                {
                  "approval": {
                    "approval_id": "appr-1", "namespace": "agents", "tenant": "demo",
                    "conversation_id": "c1", "correlation_token": "call-1",
                    "envelope_kind": "tool_call", "status": "approved",
                    "created_at": "2026-01-01T00:00:00Z",
                    "expires_at": "2026-01-02T00:00:00Z",
                    "envelope": {},
                    "decided_by": "ops-1"
                  },
                  "receipt": {
                    "events_topic": "agents.demo.events",
                    "conversation_id": "c1", "call_id": "call-1",
                    "partition": 0, "offset": 99,
                    "produced_at": "2026-01-01T00:00:01Z",
                    "cursor": "xx"
                  }
                }
                """.getBytes(StandardCharsets.UTF_8);
            exchange.getResponseHeaders().set("Content-Type", "application/json");
            exchange.sendResponseHeaders(200, respBody.length);
            exchange.getResponseBody().write(respBody);
            exchange.close();
        });
        server.start();
        try {
            int port = server.getAddress().getPort();
            try (ActeonClient client = new ActeonClient("http://127.0.0.1:" + port)) {
                Bus.BusApprovalDecisionResponse r = client.approveBusApproval(
                    "agents", "demo", "appr-1",
                    new Bus.BusApprovalDecision("ops-1"));
                assertEquals("approved", r.approval().status());
                assertNotNull(r.receipt());
                assertEquals(99L, r.receipt().offset());
            }
            assertEquals("POST", seenMethod.get());
            assertEquals("/v1/bus/approvals/agents/demo/appr-1/approve", seenPath.get());
        } finally {
            server.stop(0);
        }
    }

    @Test
    void listBusTopicsBuildsQueryString() throws Exception {
        HttpServer server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        AtomicReference<String> seenQuery = new AtomicReference<>();
        server.createContext("/", exchange -> {
            seenQuery.set(exchange.getRequestURI().getRawQuery());
            byte[] respBody = "{\"topics\":[],\"count\":0}".getBytes(StandardCharsets.UTF_8);
            exchange.getResponseHeaders().set("Content-Type", "application/json");
            exchange.sendResponseHeaders(200, respBody.length);
            exchange.getResponseBody().write(respBody);
            exchange.close();
        });
        server.start();
        try {
            int port = server.getAddress().getPort();
            try (ActeonClient client = new ActeonClient("http://127.0.0.1:" + port)) {
                List<Bus.BusTopic> topics = client.listBusTopics("agents", "demo");
                assertTrue(topics.isEmpty());
            }
            // Both filter params present in the query string.
            String q = seenQuery.get();
            assertNotNull(q);
            assertTrue(q.contains("namespace=agents"), q);
            assertTrue(q.contains("tenant=demo"), q);
        } finally {
            server.stop(0);
        }
    }

    // -------------------------------------------------------------------------
    // SSE consumer parsing
    // -------------------------------------------------------------------------

    @Test
    void busSseIteratorYieldsKeepAliveThenMessage() throws Exception {
        // Server emits: keep-alive comment, then a `bus.message` frame, then closes.
        String sse =
            ":keep-alive\n\n"
            + "event: bus.message\nid: 5\n"
            + "data: {\"topic\":\"agents.demo.events\",\"offset\":5,\"payload\":{\"k\":\"v\"}}\n\n";
        try (BusSseIterator iter = new BusSseIterator(
                new java.io.ByteArrayInputStream(sse.getBytes(StandardCharsets.UTF_8)))) {
            assertTrue(iter.hasNext());
            Bus.BusConsumeItem first = iter.next();
            assertInstanceOf(Bus.BusConsumeItem.KeepAlive.class, first);
            assertTrue(iter.hasNext());
            Bus.BusConsumeItem second = iter.next();
            assertInstanceOf(Bus.BusConsumeItem.Message.class, second);
            Bus.BusConsumedMessage msg = ((Bus.BusConsumeItem.Message) second).message();
            assertEquals("agents.demo.events", msg.topic());
            assertEquals(5L, msg.offset());
        }
    }

    @Test
    void busSseIteratorSurfacesBusErrorEvent() throws Exception {
        String sse = "event: bus.error\ndata: {\"error\":\"broker disconnected\"}\n\n";
        try (BusSseIterator iter = new BusSseIterator(
                new java.io.ByteArrayInputStream(sse.getBytes(StandardCharsets.UTF_8)))) {
            assertTrue(iter.hasNext());
            Bus.BusConsumeItem item = iter.next();
            assertInstanceOf(Bus.BusConsumeItem.Error.class, item);
            assertEquals("broker disconnected", ((Bus.BusConsumeItem.Error) item).message());
        }
    }

    @Test
    void busStreamSseIteratorClosesAfterEnd() throws Exception {
        // Server emits a chunk, then end. The iterator must yield both
        // and then report no more items (closes the stream).
        String sse =
            "event: bus.stream.chunk\nid: 0\n"
            + "data: {\"stream_id\":\"s1\",\"chunk_seq\":3,\"body\":{\"token\":\"hi\"},\"created_at\":\"2026-05-02T12:00:00Z\"}\n\n"
            + "event: bus.stream.end\nid: 1\n"
            + "data: {\"stream_id\":\"s1\",\"chunk_seq\":4,\"status\":\"complete\",\"created_at\":\"2026-05-02T12:00:01Z\"}\n\n"
            + "event: bus.stream.chunk\nid: 2\n"
            + "data: {\"stream_id\":\"s1\",\"chunk_seq\":99,\"body\":{}}\n\n";
        try (BusStreamSseIterator iter = new BusStreamSseIterator(
                new java.io.ByteArrayInputStream(sse.getBytes(StandardCharsets.UTF_8)))) {
            assertTrue(iter.hasNext());
            assertInstanceOf(Bus.BusStreamItem.Chunk.class, iter.next());
            assertTrue(iter.hasNext());
            Bus.BusStreamItem end = iter.next();
            assertInstanceOf(Bus.BusStreamItem.End.class, end);
            assertEquals("complete", ((Bus.BusStreamItem.End) end).end().status());
            // Stream should be closed; the trailing chunk after End is
            // ignored even though the underlying bytes are still
            // available.
            assertFalse(iter.hasNext());
        }
    }

    @Test
    void busStreamSseErrorWithPlainStringBody() throws Exception {
        // Defensive: future server emits an `error` event with a
        // non-JSON body. The iterator should surface the raw text.
        String sse = "event: bus.stream.error\ndata: broker disconnected\n\n";
        try (BusStreamSseIterator iter = new BusStreamSseIterator(
                new java.io.ByteArrayInputStream(sse.getBytes(StandardCharsets.UTF_8)))) {
            assertTrue(iter.hasNext());
            Bus.BusStreamItem item = iter.next();
            assertInstanceOf(Bus.BusStreamItem.Error.class, item);
            assertEquals("broker disconnected", ((Bus.BusStreamItem.Error) item).message());
        }
    }
}
