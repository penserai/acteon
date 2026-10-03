package com.acteon.client;

import com.acteon.client.exceptions.HttpException;
import com.sun.net.httpserver.HttpServer;
import java.net.InetSocketAddress;
import java.net.URLEncoder;
import java.nio.charset.StandardCharsets;
import java.util.Map;
import java.util.List;
import java.util.HashMap;
import java.util.concurrent.atomic.AtomicReference;
import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.*;

class PlatformTest {
    @Test void allOperationsPreserveWireContracts() throws Exception {
        var server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        var current = new AtomicReference<PlatformOperation>();
        var failure = new AtomicReference<Throwable>();
        server.createContext("/", exchange -> {
            var op = current.get();
            try {
                assertEquals(op.method, exchange.getRequestMethod());
                String expected = op.path;
                for (String key : op.parameters) expected = expected.replace("{" + key + "}", URLEncoder.encode("team/child ?#%", StandardCharsets.UTF_8).replace("+", "%20"));
                assertEquals(expected, exchange.getRequestURI().getRawPath());
                assertEquals("Bearer test-key", exchange.getRequestHeaders().getFirst("Authorization"));
                assertEquals("filter=a+b&filter=c%26d", exchange.getRequestURI().getRawQuery());
                if (!op.method.equals("GET")) assertEquals("{\"request_id\":\"stable\"}", new String(exchange.getRequestBody().readAllBytes(), StandardCharsets.UTF_8));
            } catch (Throwable error) { failure.set(error); }
            byte[] body = (op.text ? "metric 1\n" : "{\"opaque\":[1,null]}").getBytes(StandardCharsets.UTF_8);
            exchange.sendResponseHeaders(200, body.length);
            exchange.getResponseBody().write(body);
            exchange.close();
        });
        server.start();
        try (var client = new ActeonClient("http://127.0.0.1:" + server.getAddress().getPort(), "test-key")) {
            for (var op : PlatformOperation.values()) {
                current.set(op);
                Map<String,String> paths = new HashMap<>();
                for (String key : op.parameters) paths.put(key, "team/child ?#%");
                var result = client.platformRequest(op, paths, Map.of("filter", List.of("a b", "c&d")), op.method.equals("GET") ? null : Map.of("request_id", "stable"));
                if (failure.get() != null) throw new AssertionError(op.name(), failure.get());
                if (op.text) assertEquals("metric 1\n", result.asText());
                else assertTrue(result.get("opaque").get(1).isNull());
            }
            assertThrows(IllegalArgumentException.class, () -> client.platformRequest(PlatformOperation.BUS_STAGES_STATUS, Map.of("namespace","n","tenant","t","id",".."), null, null));
        } finally { server.stop(0); }
    }

    @Test void conflictIsNotRetriedAndEmptySuccessIsNull() throws Exception {
        var server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        var calls = new java.util.concurrent.atomic.AtomicInteger();
        server.createContext("/", exchange -> { exchange.sendResponseHeaders(calls.incrementAndGet() == 1 ? 409 : 204, -1); exchange.close(); });
        server.start();
        try (var client = new ActeonClient("http://127.0.0.1:" + server.getAddress().getPort())) {
            assertThrows(HttpException.class, () -> client.platformRequest(PlatformOperation.AUTH_LOGOUT, null, null, null));
            assertEquals(1, calls.get());
            assertTrue(client.platformRequest(PlatformOperation.AUTH_LOGOUT, null, null, null).isNull());
        } finally { server.stop(0); }
    }

    @Test void governanceOutcomesKeepEveryFieldAndBatchVariant() throws Exception {
        var mapper = JsonMapper.build();
        var fixtures = mapper.readTree(java.nio.file.Files.readString(java.nio.file.Path.of("../contract-fixtures/dispatch-outcomes.json")));
        for (var fixture : fixtures) {
            var wire = fixture.get("wire");
            var outcome = mapper.treeToValue(wire, com.acteon.client.models.ActionOutcome.class);
            assertEquals(fixture.get("type").asText().toUpperCase(), outcome.getType().name());
            assertFalse(outcome.isExecuted());
            assertFalse(outcome.isFailed());
            var batch = mapper.treeToValue(wire, com.acteon.client.models.BatchResult.class);
            assertTrue(batch.isSuccess());
            assertEquals(outcome.getType(), batch.getOutcome().getType());
            if (!wire.isObject()) continue;
            var fields = wire.elements().next().fields();
            while (fields.hasNext()) {
                var field = fields.next();
                var getter = new StringBuilder("get");
                for (String word : field.getKey().split("_")) getter.append(Character.toUpperCase(word.charAt(0))).append(word.substring(1));
                Object value = outcome.getClass().getMethod(getter.toString()).invoke(outcome);
                assertEquals(field.getValue().toString(), mapper.valueToTree(value).toString(), field.getKey());
            }
        }
    }

    @Test void typedTopicDeletionResolvesScopedKafkaName() throws Exception {
        var seen = new java.util.concurrent.CopyOnWriteArrayList<String>();
        var server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        server.createContext("/", exchange -> {
            seen.add(exchange.getRequestMethod() + " " + exchange.getRequestURI().getPath());
            if (exchange.getRequestMethod().equals("GET")) {
                byte[] body = """
                    {"topics":[{"name":"logs","namespace":"n","tenant":"other","kafka_name":"wrong"},{"name":"logs","namespace":"n","tenant":"t","kafka_name":"actual.logs"}]}
                    """.getBytes(StandardCharsets.UTF_8);
                exchange.sendResponseHeaders(200, body.length);
                exchange.getResponseBody().write(body);
            } else exchange.sendResponseHeaders(204, -1);
            exchange.close();
        });
        server.start();
        try (var client = new ActeonClient("http://127.0.0.1:" + server.getAddress().getPort())) {
            client.deleteBusTopic("n", "t", "logs");
            assertEquals(List.of("GET /v1/bus/topics", "DELETE /v1/bus/topics/actual.logs"), seen);
        } finally { server.stop(0); }
    }
}
