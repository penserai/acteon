package com.acteon.client;
import com.acteon.client.models.*;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.sun.net.httpserver.HttpServer;
import java.net.InetSocketAddress;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;
import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.*;
class PermitsTest {
    @Test void dispatchPermitsPreserveCredentialsAndLegacyHeaderOmission() throws Exception {
        var mapper = new ObjectMapper();
        var fixture = mapper.readTree(Files.readString(Path.of("../contract-fixtures/execution-permits.json")));
        var permits = List.of(mapper.treeToValue(fixture.get(0), PermitReference.class));
        var calls = new AtomicInteger();
        var failure = new AtomicReference<Throwable>();
        var server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        server.createContext("/v1/dispatch", exchange -> {
            try {
                assertEquals("Bearer test-key", exchange.getRequestHeaders().getFirst("Authorization"));
                var header = exchange.getRequestHeaders().getFirst("x-acteon-execution-permits");
                if (calls.getAndIncrement() == 0) assertNull(header);
                else assertEquals(fixture, mapper.readTree(header));
                var body = mapper.readTree(exchange.getRequestBody());
                var action = body.isArray() ? body.get(0) : body;
                assertEquals(42, action.get("payload").get("incident").asInt());
            } catch (Throwable e) { failure.set(e); }
            var body = (exchange.getRequestURI().getPath().endsWith("/batch") ? "[\"Deduplicated\"]" : "\"Deduplicated\"").getBytes(java.nio.charset.StandardCharsets.UTF_8);
            exchange.sendResponseHeaders(200, body.length);
            exchange.getResponseBody().write(body);
            exchange.close();
        });
        server.start();
        try (var client = new ActeonClient("http://127.0.0.1:" + server.getAddress().getPort(), "test-key")) {
            var action = new Action("prod", "acme", "incident", "execute", Map.of("incident", 42));
            client.dispatch(action);
            client.dispatch(action, permits);
            client.dispatchBatch(List.of(action), permits);
            if (failure.get() != null) throw new AssertionError(failure.get());
            assertEquals(3, calls.get());
        } finally { server.stop(0); }
    }
    @Test void refusalsPreserveHttpStatus() throws Exception {
        for (int status : new int[]{400, 403, 409}) {
            var server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
            server.createContext("/v1/dispatch", exchange -> {
                byte[] body = "{\"error\":\"permit refused\"}".getBytes(java.nio.charset.StandardCharsets.UTF_8);
                exchange.sendResponseHeaders(status, body.length);
                exchange.getResponseBody().write(body); exchange.close();
            });
            server.start();
            try (var client = new ActeonClient("http://127.0.0.1:" + server.getAddress().getPort())) {
                var action = new Action("prod", "acme", "incident", "execute", Map.of());
                var error = assertThrows(com.acteon.client.exceptions.HttpException.class,
                    () -> client.dispatch(action, List.of()));
                assertEquals(status, error.getStatus());
                var batchError = assertThrows(com.acteon.client.exceptions.HttpException.class,
                    () -> client.dispatchBatch(List.of(action), List.of()));
                assertEquals(status, batchError.getStatus());
            } finally { server.stop(0); }
        }
    }
}
