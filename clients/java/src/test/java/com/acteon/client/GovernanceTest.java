package com.acteon.client;

import com.acteon.client.exceptions.HttpException;
import com.acteon.client.models.Governance;
import com.acteon.client.models.ProviderExecutionHistory;
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
    @Test void historyCapabilityDefaultsDeniedAndPreservesLegacyWire() throws Exception {
        var mapper = JsonMapper.build();
        var fixture = mapper.readTree(Files.readString(Path.of("../contract-fixtures/governance-management.json")));
        for (boolean granted : new boolean[]{false, true}) {
            var wire = fixture.get("scope").deepCopy();
            if (granted) {
                ((com.fasterxml.jackson.databind.node.ObjectNode) wire.get("management")).put("can_read_history", true);
                ((com.fasterxml.jackson.databind.node.ObjectNode) wire.get("management")).put("can_reconcile", true);
            }
            var scope = mapper.treeToValue(wire, Governance.ScopeView.class);
            assertEquals(granted, scope.management().canReadHistory());
            assertEquals(granted, scope.management().canReconcile());
            assertEquals(wire, mapper.readTree(mapper.writeValueAsString(scope)));
        }
    }

    @Test void providerHistoryDecodesNestedEvidence() throws Exception {
        var mapper = JsonMapper.build();
        var fixtures = mapper.readTree(Files.readString(Path.of("../contract-fixtures/provider-history.json")));
        for (var wire : fixtures) {
            var history = mapper.treeToValue(wire, ProviderExecutionHistory.class);
            assertEquals("agent/maya", history.subject().id());
            assertEquals(wire.get("receipt").get("status").get("state").asText(), history.receipt().status().state());
            assertEquals(wire.get("metadata").isNull(), history.metadata() == null);
            if (history.receipt().status().state().equals("completed")) {
                assertTrue(history.receipt().status().outcome().isExecuted());
                assertTrue(history.attempts().getFirst().originalOutcome().isFailed());
                assertTrue(history.attempts().getFirst().reconciliation().outcome().isExecuted());
                assertEquals("original-result", history.attempts().getFirst().originalEvidence().id());
                assertEquals("resolution", history.attempts().getFirst().reconciliation().resolution().id());
                var acceptance = history.attempts().getFirst().reconciliation().acceptance();
                if (wire.get("attempts").get(0).get("reconciliation").has("acceptance")) {
                    assertEquals("operator/incident", acceptance.operator().id());
                    assertEquals(6, acceptance.authority().generation());
                    assertEquals(1700000060000L, acceptance.acceptedAtMs());
                } else {
                    assertNull(acceptance);
                }
            }
        }
    }

    @Test void providerHistoryUsesAuthenticatedScopedGetAndPreservesRefusals() throws Exception {
        var mapper = JsonMapper.build();
        var fixtures = mapper.readTree(Files.readString(Path.of("../contract-fixtures/provider-history.json")));
        var executionId = fixtures.get(0).get("receipt").get("execution_id").asText();
        var calls = new AtomicInteger();
        var responseStatus = new AtomicInteger(200);
        var server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        server.createContext("/v1/governance/executions/" + executionId, exchange -> {
            assertEquals("GET", exchange.getRequestMethod());
            assertEquals("Bearer operator-key", exchange.getRequestHeaders().getFirst("Authorization"));
            assertTrue(exchange.getRequestURI().getQuery().contains("namespace=prod"));
            assertTrue(exchange.getRequestURI().getQuery().contains("tenant=acme"));
            int index = calls.getAndIncrement();
            byte[] body = (responseStatus.get() == 200 ? fixtures.get(index).toString() : "{\"error\":\"history_denied\"}")
                .getBytes(StandardCharsets.UTF_8);
            exchange.sendResponseHeaders(responseStatus.get(), body.length);
            exchange.getResponseBody().write(body);
            exchange.close();
        });
        server.start();
        try (var client = new ActeonClient("http://127.0.0.1:" + server.getAddress().getPort(), "operator-key")) {
            for (var wire : fixtures) {
                var history = client.providerExecutionHistory("prod", "acme", executionId);
                assertEquals(wire.get("receipt").get("status").get("state").asText(), history.receipt().status().state());
            }
            for (int status : new int[]{401, 403, 404, 409, 503}) {
                responseStatus.set(status);
                int before = calls.get();
                var error = assertThrows(HttpException.class, () -> client.providerExecutionHistory("prod", "acme", executionId));
                assertEquals(status, error.getStatus());
                assertEquals(before + 1, calls.get());
            }
        } finally { server.stop(0); }
    }

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

    @Test void providerReconciliationWireContracts() throws Exception {
        var mapper = JsonMapper.build();
        var fixture = mapper.readTree(Files.readString(Path.of("../contract-fixtures/provider-reconciliation.json")));
        var correlation = mapper.treeToValue(fixture.get("correlation"), com.acteon.client.models.ProviderReconciliation.Correlation.class);
        assertEquals("prod", correlation.context().namespace());
        assertEquals(0, correlation.ordinal());
        assertEquals(fixture.get("correlation"), mapper.readTree(mapper.writeValueAsString(correlation)));
        var request = mapper.treeToValue(fixture.get("request"), com.acteon.client.models.ProviderReconciliation.Request.class);
        assertEquals("b3BhcXVlLXByb29m", request.proofBase64());
        assertEquals(fixture.get("request"), mapper.readTree(mapper.writeValueAsString(request)));
        var completed = mapper.treeToValue(fixture.get("receipt"), ProviderExecutionHistory.Receipt.class);
        assertTrue(completed.status().outcome().isExecuted());
        var fenced = mapper.treeToValue(fixture.get("no_effect_receipt"), ProviderExecutionHistory.Receipt.class);
        assertTrue(fenced.status().outcome().isFailed());
    }
}
