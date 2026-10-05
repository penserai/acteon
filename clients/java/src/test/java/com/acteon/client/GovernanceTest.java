package com.acteon.client;

import com.acteon.client.exceptions.HttpException;
import com.acteon.client.models.Governance;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.sun.net.httpserver.HttpServer;
import org.junit.jupiter.api.Test;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.concurrent.atomic.AtomicInteger;
import static org.junit.jupiter.api.Assertions.*;

class GovernanceTest {
    @Test void typedWireContractsAndRefusals() throws Exception {
        var mapper = new ObjectMapper();
        var fixture = mapper.readTree(Files.readString(Path.of("../contract-fixtures/governance-management.json")));
        var requests = new java.util.concurrent.CopyOnWriteArrayList<String>();
        var bodies = new java.util.concurrent.CopyOnWriteArrayList<String>();
        var authorization = new java.util.concurrent.CopyOnWriteArrayList<String>();
        var status = new AtomicInteger(200);
        var server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        server.createContext("/v1/governance", exchange -> {
            requests.add(exchange.getRequestMethod() + " " + exchange.getRequestURI());
            bodies.add(new String(exchange.getRequestBody().readAllBytes(), StandardCharsets.UTF_8));
            authorization.add(exchange.getRequestHeaders().getFirst("Authorization"));
            var result = status.get() == 200 ? fixture.get(exchange.getRequestMethod().equals("GET") ? "scope" : "receipt") : mapper.createObjectNode().put("error", "denied");
            byte[] wire = result.toString().getBytes(StandardCharsets.UTF_8);
            exchange.getResponseHeaders().add("Content-Type", "application/json");
            exchange.sendResponseHeaders(status.get(), wire.length);
            exchange.getResponseBody().write(wire); exchange.close();
        });
        server.start();
        try (var client = new ActeonClient("http://127.0.0.1:" + server.getAddress().getPort(), "operator-key")) {
            var scope = client.governance("prod", "acme");
            assertEquals(fixture.get("scope"), mapper.readTree(mapper.writeValueAsString(scope)));
            var publication = mapper.treeToValue(fixture.get("publication"), Governance.PublishPermitRequest.class);
            var intervention = mapper.treeToValue(fixture.get("intervention"), Governance.InterventionRequest.class);
            assertEquals(fixture.get("receipt"), mapper.readTree(mapper.writeValueAsString(client.publishGovernancePermit(publication))));
            assertEquals(fixture.get("receipt"), mapper.readTree(mapper.writeValueAsString(client.interveneGovernance(intervention))));
            assertTrue(requests.get(0).contains("namespace=prod"));
            assertTrue(requests.get(0).contains("tenant=acme"));
            assertEquals(fixture.get("publication"), mapper.readTree(bodies.get(1)));
            assertEquals(fixture.get("intervention"), mapper.readTree(bodies.get(2)));
            assertTrue(authorization.stream().allMatch("Bearer operator-key"::equals));
            for (int denied : new int[]{401,403,409,503}) {
                status.set(denied); int count = requests.size();
                var error = assertThrows(HttpException.class, () -> client.interveneGovernance(intervention));
                assertEquals(denied, error.getStatus());
                assertEquals(count + 1, requests.size());
            }
        } finally { server.stop(0); }
    }
}
