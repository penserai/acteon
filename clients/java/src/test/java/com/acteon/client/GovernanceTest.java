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
import java.util.Map;
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

    @Test void federationManagementAndInertImportPreserveWire() throws Exception {
        var mapper = JsonMapper.build();
        var fixture = mapper.readTree(Files.readString(Path.of("../contract-fixtures/federation-management.json")));
        var paths = new java.util.concurrent.CopyOnWriteArrayList<String>();
        var bodies = new java.util.concurrent.CopyOnWriteArrayList<com.fasterxml.jackson.databind.JsonNode>();
        var server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        server.createContext("/", exchange -> {
            paths.add(exchange.getRequestURI().getPath());
            var body = mapper.readTree(exchange.getRequestBody());
            if (body != null && !body.isMissingNode()) bodies.add(body);
            assertEquals("Bearer federation-key", exchange.getRequestHeaders().getFirst("Authorization"));
            var path = exchange.getRequestURI().getPath();
            var response = path.endsWith("/correlation")
                ? fixture.get("finality_correlation")
                : path.endsWith(":reconcile")
                ? fixture.get("reconciled_evidence")
                : path.endsWith(":abandon")
                ? fixture.get("abandoned_evidence")
                : path.equals("/v1/governance/federation/deliveries")
                ? fixture.get("delivery_evidence_page")
                : path.endsWith("/capabilities")
                ? fixture.get("signed_capability_card")
                : path.contains("/federation/exports/")
                ? fixture.get("export_receipt")
                : path.endsWith("message:send")
                ? mapper.readTree("{\"id\":\"federated-task\",\"namespace\":\"prod\",\"tenant\":\"acme\"}")
                : path.endsWith("envelopes:import") ? fixture.get("import_receipt") : fixture.get("change_receipt");
            byte[] wire = response.toString().getBytes(StandardCharsets.UTF_8);
            exchange.getResponseHeaders().add("Content-Type", "application/json");
            exchange.sendResponseHeaders(200, wire.length);
            exchange.getResponseBody().write(wire);
            exchange.close();
        });
        server.start();
        try (var client = new ActeonClient("http://127.0.0.1:" + server.getAddress().getPort(), "federation-key")) {
            var publication = mapper.treeToValue(fixture.get("publication"), Governance.PublishFederationTrustRequest.class);
            var revocation = mapper.treeToValue(fixture.get("revocation"), Governance.RevokeFederationTrustRequest.class);
            var envelope = mapper.treeToValue(fixture.get("envelope"), Governance.SignedFederationEnvelopeDeclaration.class);
            client.publishFederationTrust(publication);
            client.revokeFederationTrust(revocation);
            var capability = client.federationCapabilities("prod", "acme", "maya");
            var receipt = client.importFederationEnvelope("prod", "acme", envelope);
            var invocation = new Governance.SignedFederationInvocationDeclaration("signed-payload", "a".repeat(128));
            var message = Map.<String, Object>of("messageId", "message-1", "role", "user",
                "parts", java.util.List.of(Map.of("text", "inspect")));
            var task = client.invokeFederatedAgent("prod", "acme", "maya", invocation, message);
            var exportMessage = mapper.convertValue(fixture.get("export_message"),
                new com.fasterxml.jackson.core.type.TypeReference<Map<String, Object>>() {});
            var sourceTask = "11111111-1111-4111-8111-111111111111";
            var exported = client.exportFederatedMessage("prod", "acme", "maya", sourceTask,
                "remote-triage", exportMessage);
            client.observeFederatedExport("prod", "acme", "maya", sourceTask,
                "remote-triage", exportMessage);
            client.replayFederatedExport("prod", "acme", "maya", sourceTask,
                "remote-triage", exportMessage);
            var evidence = client.federationDeliveryEvidence("prod", "acme", null, 25);
            var abandonRequest = mapper.treeToValue(fixture.get("abandon_request"),
                Governance.AbandonFederationDeliveryRequest.class);
            var abandoned = client.abandonFederationDelivery(
                evidence.deliveries().get(0).deliveryId(), abandonRequest);
            var correlation = client.federationFinalityCorrelation("prod", "acme",
                evidence.deliveries().get(0).deliveryId());
            var reconcileRequest = mapper.treeToValue(fixture.get("reconcile_request"),
                Governance.ReconcileFederationDeliveryRequest.class);
            var reconciled = client.reconcileFederationDelivery(
                evidence.deliveries().get(0).deliveryId(), reconcileRequest);
            assertFalse(receipt.executionStarted());
            assertEquals(fixture.get("signed_capability_card").get("payload").asText(), capability.payload());
            assertEquals("federated-task", task.get("id").asText());
            assertEquals("in_flight", exported.attemptStatus());
            assertEquals("export-message-1", evidence.deliveries().get(0).messageId());
            assertEquals("abandoned", abandoned.status().get("status").asText());
            assertEquals("import", correlation.stage());
            assertEquals(1, reconciled.reconciliations().size());
            assertEquals(java.util.List.of(
                "/v1/governance/federation/trusts",
                "/v1/governance/federation/trusts/revoke",
                "/a2a/prod/acme/federation/agents/maya/capabilities",
                "/a2a/prod/acme/federation/envelopes:import",
                "/a2a/prod/acme/federation/agents/maya/v1/message:send",
                "/a2a/prod/acme/agents/maya/v1/tasks/" + sourceTask + "/federation/exports/remote-triage/message:send",
                "/a2a/prod/acme/agents/maya/v1/tasks/" + sourceTask + "/federation/exports/remote-triage/message:observe",
                "/a2a/prod/acme/agents/maya/v1/tasks/" + sourceTask + "/federation/exports/remote-triage/message:replay",
                "/v1/governance/federation/deliveries",
                "/v1/governance/federation/deliveries/" + evidence.deliveries().get(0).deliveryId() + ":abandon",
                "/v1/governance/federation/deliveries/" + evidence.deliveries().get(0).deliveryId() + "/correlation",
                "/v1/governance/federation/deliveries/" + evidence.deliveries().get(0).deliveryId() + ":reconcile"), paths);
            assertEquals(fixture.get("publication"), bodies.get(0));
            assertEquals(fixture.get("revocation"), bodies.get(1));
            assertEquals(fixture.get("envelope"), bodies.get(2).get("envelope"));
            assertEquals("signed-payload", bodies.get(3).get("invocation").get("payload").asText());
            assertEquals("message-1", bodies.get(3).get("message").get("messageId").asText());
            assertEquals(fixture.get("abandon_request"), bodies.get(7));
            assertEquals(fixture.get("reconcile_request"), bodies.get(8));
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
    @Test void registryRequestsAndCompletedReceiptsPreserveOriginalIdentity() throws Exception {
        var mapper = new ObjectMapper();
        var fixture = mapper.readTree(Files.readString(Path.of("../contract-fixtures/governance-registry.json")));
        var request = mapper.treeToValue(fixture.get("request"), Governance.RegistryMutationRequest.class);
        var receipt = new java.util.concurrent.atomic.AtomicReference<>(fixture.get("receipt"));
        var view = new java.util.concurrent.atomic.AtomicReference<>(fixture.get("view"));
        var status = new AtomicInteger(200);
        var calls = new AtomicInteger();
        var server = HttpServer.create(new InetSocketAddress("127.0.0.1",0),0);
        server.createContext("/v1/governance/registry", exchange -> {
            calls.incrementAndGet();
            assertEquals("Bearer operator-key", exchange.getRequestHeaders().getFirst("Authorization"));
            boolean read = exchange.getRequestMethod().equals("GET");
            if (read) {
                assertEquals("/v1/governance/registry/maya", exchange.getRequestURI().getPath());
                for (var part : new String[]{"namespace=prod","tenant=acme","projection=card"}) assertTrue(exchange.getRequestURI().getQuery().contains(part));
            } else assertEquals(fixture.get("request"), mapper.readTree(exchange.getRequestBody()));
            byte[] body=(read ? view.get() : receipt.get()).toString().getBytes(StandardCharsets.UTF_8);
            exchange.getResponseHeaders().add("Location","/redirected");
            exchange.sendResponseHeaders(read ? 200 : status.get(),body.length);
            exchange.getResponseBody().write(body); exchange.close();
        }); server.start();
        try (var client = new ActeonClient("http://127.0.0.1:"+server.getAddress().getPort(),"operator-key")) {
            assertEquals(fixture.get("view"),mapper.readTree(mapper.writeValueAsString(client.registryProjection("prod","acme","maya","card"))));
            for (var field : new String[]{"tenant","version","qualification_retired","registry_revision","value"}) {
                var bad = (com.fasterxml.jackson.databind.node.ObjectNode) fixture.get("view").deepCopy();
                if (field.equals("tenant")) bad.put(field,"other"); else if (field.equals("qualification_retired")) bad.putNull(field); else if (field.equals("value")) bad.put(field,"invalid"); else bad.put(field,0);
                view.set(bad); int before=calls.get();
                assertThrows(com.acteon.client.exceptions.ActeonException.class,()->client.registryProjection("prod","acme","maya","card"));
                assertEquals(before+1,calls.get());
            }
            for (int i=0;i<2;i++) assertEquals(fixture.get("receipt"),mapper.readTree(mapper.writeValueAsString(client.mutateRegistry(request))));
            for (var field : new String[]{"delivery_complete","applied","change_id","tenant","input_digest"}) {
                var bad = (com.fasterxml.jackson.databind.node.ObjectNode) fixture.get("receipt").deepCopy();
                if (field.equals("delivery_complete") || field.equals("applied")) bad.put(field,false); else bad.put(field,"bad");
                receipt.set(bad); int before=calls.get();
                assertThrows(com.acteon.client.exceptions.ActeonException.class,()->client.mutateRegistry(request));
                assertEquals(before+1,calls.get());
            }
            for (int errorStatus : new int[]{401,403,409,503,307}) {
                status.set(errorStatus); int before=calls.get();
                assertEquals(errorStatus,assertThrows(HttpException.class,()->client.mutateRegistry(request)).getStatus());
                assertEquals(before+1,calls.get());
            }
        } finally {server.stop(0);}
    }

}
