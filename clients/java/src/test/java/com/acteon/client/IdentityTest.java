package com.acteon.client;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.sun.net.httpserver.HttpServer;
import java.net.InetSocketAddress;
import java.nio.file.Files;
import java.nio.file.Path;
import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.*;

class IdentityTest {
    @Test void credentialAndActorRemainDistinctAcrossAllFixtures() throws Exception {
        var mapper = new ObjectMapper();
        var fixtures = mapper.readTree(Files.readString(Path.of("../contract-fixtures/identity.json")));
        for (var wire : fixtures) {
            var server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
            var failure = new java.util.concurrent.atomic.AtomicReference<Throwable>();
            server.createContext("/v1/auth/identity", exchange -> {
                try {
                    assertEquals("GET", exchange.getRequestMethod());
                    assertEquals("Bearer test-key", exchange.getRequestHeaders().getFirst("Authorization"));
                } catch (Throwable e) { failure.set(e); }
                byte[] body = wire.toString().getBytes(java.nio.charset.StandardCharsets.UTF_8);
                exchange.sendResponseHeaders(200, body.length);
                exchange.getResponseBody().write(body);
                exchange.close();
            });
            server.start();
            try (var client = new ActeonClient("http://127.0.0.1:" + server.getAddress().getPort(), "test-key")) {
                var identity = client.identity();
                if (failure.get() != null) throw new AssertionError(failure.get());
                assertEquals(wire.get("credential_id").asText(), identity.credentialId());
                assertEquals(wire.get("auth_method").asText(), identity.authMethod());
                assertEquals(wire.get("role").asText(), identity.role());
                if (wire.get("principal").isNull()) assertNull(identity.principal());
                else {
                    assertEquals(wire.get("principal").get("id").asText(), identity.principal().id());
                    assertEquals(wire.get("principal").get("kind").asText(), identity.principal().kind());
                }
            } finally { server.stop(0); }
        }
    }
}
