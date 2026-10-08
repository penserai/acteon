package com.acteon.client;

import com.acteon.client.exceptions.HttpException;
import com.sun.net.httpserver.HttpServer;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Map;
import java.util.List;
import com.acteon.client.models.PermitReference;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;
import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.*;

class AgentServiceTest {
    @Test void peerToolSendsNoAuthorityFieldsAndValidatesReceipt() throws Exception {
        var mapper = JsonMapper.build();
        var task = mapper.readTree("{\"id\":\"job-1\",\"namespace\":\"prod\",\"tenant\":\"acme\"}");
        var source = new AgentServiceReceipt("prod","acme","notifier","job-1","host-only",task);
        var failure = new AtomicReference<Throwable>();
        var server = HttpServer.create(new InetSocketAddress("127.0.0.1",0),0);
        server.createContext("/",exchange -> {
            try {
                assertNull(exchange.getRequestHeaders().getFirst(AgentServiceReceipt.SOURCE_CONTEXT_HEADER));
                assertNull(exchange.getRequestHeaders().getFirst("x-acteon-execution-context"));
                assertNull(exchange.getRequestHeaders().getFirst("x-acteon-execution-permits"));
                if (exchange.getRequestMethod().equals("GET")) {
                    assertTrue(exchange.getRequestURI().getRawPath().endsWith("/tasks/job-1/peers"));
                    assertEquals("skill=diagnose", exchange.getRequestURI().getRawQuery());
                } else assertTrue(exchange.getRequestURI().getRawPath().contains("/peers/team%2Fresolver/diagnose/"));
            } catch(Throwable error) { failure.set(error); }
            exchange.getResponseHeaders().set(A2A.VERSION_HEADER,"1.0");
            var body = (exchange.getRequestMethod().equals("GET")
                ? "{\"peers\":[{\"agent_id\":\"resolver\",\"skill\":\"diagnose\",\"description_untrusted\":\"Investigates incidents\",\"card_version\":\"v1\",\"binding_digest\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"checked_at_ms\":42}]}"
                : exchange.getRequestURI().getPath().endsWith(":cancel")
                ? "{\"submission_id\":\"f47ac10b-58cc-5372-a567-0e02b2c3d479\",\"cancellation_id\":\"67e55044-10b1-526f-9247-bb680e5fe0c8\",\"status\":{\"state\":\"reconciled\",\"task\":{\"id\":\"remote-1\",\"namespace\":\"prod\",\"tenant\":\"acme\",\"status\":{\"state\":\"canceled\"}}}}"
                : "{\"submission_id\":\"f47ac10b-58cc-5372-a567-0e02b2c3d479\",\"status\":{\"state\":\"accepted\",\"task\":{\"id\":\"remote-1\",\"namespace\":\"prod\",\"tenant\":\"acme\"}}}").getBytes(StandardCharsets.UTF_8);
            exchange.sendResponseHeaders(200,body.length); exchange.getResponseBody().write(body); exchange.close();
        }); server.start();
        try(var client = new ActeonClient("http://127.0.0.1:"+server.getAddress().getPort())) {
            var peers = client.agentServiceDiscoverPeers(source,"diagnose");
            assertEquals("resolver",peers.get(0).agentId());
            var receipt = client.agentServiceSendPeer(source,"team/resolver","diagnose",Map.of("messageId","peer-1"));
            assertEquals("accepted",receipt.state());
            var refreshed = client.agentServiceRefreshPeer(source,"team/resolver","diagnose",receipt);
            assertEquals(receipt.submissionId(),refreshed.submissionId());
            var canceled = client.agentServiceCancelPeer(source,"team/resolver","diagnose",receipt);
            assertEquals("reconciled",canceled.state());
            if(failure.get()!=null) throw new AssertionError(failure.get());
        } finally { server.stop(0); }
    }

    @Test void parentContextAndPermitsAreRequestLocal() throws Exception {
        var mapper = JsonMapper.build();
        var fixture = mapper.readTree(Files.readString(Path.of("../contract-fixtures/agent-services.json")));
        var failure = new AtomicReference<Throwable>();
        var server = HttpServer.create(new InetSocketAddress("127.0.0.1",0),0);
        server.createContext("/",exchange -> {
            try {
                assertEquals(fixture.path("parent").path("execution_context").asText(), exchange.getRequestHeaders().getFirst("x-acteon-execution-context"));
                assertEquals(fixture.path("parent").path("permits"), mapper.readTree(exchange.getRequestHeaders().getFirst("x-acteon-execution-permits")));
                assertNull(exchange.getRequestHeaders().getFirst(AgentServiceReceipt.SOURCE_CONTEXT_HEADER));
            } catch(Throwable error) { failure.set(error); }
            var job = fixture.path("jobs").get(0);
            exchange.getResponseHeaders().set(A2A.VERSION_HEADER,"1.0");
            exchange.getResponseHeaders().set(AgentServiceReceipt.SOURCE_CONTEXT_HEADER,job.path("source_context").asText());
            var body = mapper.writeValueAsBytes(job.path("task"));
            exchange.sendResponseHeaders(200,body.length); exchange.getResponseBody().write(body); exchange.close();
        }); server.start();
        try(var client = new ActeonClient("http://127.0.0.1:"+server.getAddress().getPort())) {
            var p = fixture.path("parent").path("permits").get(0);
            client.agentServiceSendMessage("prod","acme","notifier",Map.of(),new AgentServiceParent(
                fixture.path("parent").path("execution_context").asText(), List.of(new PermitReference(p.path("id").asText(),p.path("accepted_revision").asLong()))));
            if(failure.get()!=null) throw new AssertionError(failure.get());
        } finally { server.stop(0); }
    }
    @Test void providerAbortRequiresCanonicalUuidV5() throws Exception {
        var mapper = JsonMapper.build();
        for (var id : new String[]{"F47AC10B-58CC-5372-A567-0E02B2C3D479","f47ac10b-58cc-4372-a567-0e02b2c3d479"}) {
            var value = mapper.readTree("{\"state\":\"uncertain\",\"attempt_id\":\""+id+"\"}");
            assertThrows(com.acteon.client.exceptions.ActeonException.class,()->AgentServiceProviderAbort.parse(value));
        }
    }
    @Test void receiptsRetainOriginalJobWithRequestLocalHeaders() throws Exception {
        var mapper = JsonMapper.build();
        var fixture = mapper.readTree(Files.readString(Path.of("../contract-fixtures/agent-services.json")));
        var calls = new AtomicInteger(); var failure = new AtomicReference<Throwable>();
        var server = HttpServer.create(new InetSocketAddress("127.0.0.1",0),0);
        server.createContext("/",exchange -> {
            calls.incrementAndGet(); int index = 0;
            try {
                assertEquals("Bearer caller-key",exchange.getRequestHeaders().getFirst("Authorization"));
                assertEquals("1.0",exchange.getRequestHeaders().getFirst(A2A.VERSION_HEADER));
                if (exchange.getRequestMethod().equals("POST") && !exchange.getRequestURI().getPath().endsWith("/stop")) {
                    var request = mapper.readTree(exchange.getRequestBody());
                    if (request.path("message").path("messageId").asText().equals("m2")) index = 1;
                    assertNull(exchange.getRequestHeaders().getFirst(AgentServiceReceipt.SOURCE_CONTEXT_HEADER));
                } else {
                    if ((exchange.getRequestURI().getPath().endsWith("job-2") || exchange.getRequestURI().getPath().endsWith("job-2/stop"))) index = 1;
                    assertEquals(fixture.path("jobs").get(index).path("source_context").asText(),exchange.getRequestHeaders().getFirst(AgentServiceReceipt.SOURCE_CONTEXT_HEADER));
                }
            } catch(Throwable error) { failure.set(error); }
            var job = fixture.path("jobs").get(index);
            exchange.getResponseHeaders().set(A2A.VERSION_HEADER,"1.0");
            exchange.getResponseHeaders().set(AgentServiceReceipt.SOURCE_CONTEXT_HEADER,job.path("source_context").asText());
            var body = mapper.writeValueAsBytes(job.path(exchange.getRequestURI().getPath().endsWith("/stop") ? "stop_response" : "task"));
            exchange.sendResponseHeaders(200,body.length); exchange.getResponseBody().write(body); exchange.close();
        }); server.start();
        try(var client = new ActeonClient("http://127.0.0.1:"+server.getAddress().getPort(),"caller-key")) {
            var first = client.agentServiceSendMessage("prod","acme","notifier",Map.of("messageId","m1"));
            var second = client.agentServiceSendMessage("prod","acme","notifier",Map.of("messageId","m2"));
            ((com.fasterxml.jackson.databind.node.ObjectNode)first.task()).put("id","tampered-model-id");
            var restored = mapper.readValue(mapper.writeValueAsString(first),AgentServiceReceipt.class);
            assertEquals("job-1",client.agentServiceGetTask(restored).path("id").asText());
            assertEquals("job-2",client.agentServiceGetTask(second).path("id").asText());
            assertFalse(first.toString().contains(first.sourceContext()));
            var stopped = client.agentServiceStopTask(restored);
            assertTrue(stopped.futureStartsBlocked());
            assertEquals("job-1",stopped.task().path("id").asText());
            assertEquals("submitted",stopped.task().path("status").path("state").asText());
            assertEquals("restricted_only",stopped.providerAbort().state());
            var stoppedSecond = client.agentServiceStopTask(second);
            assertEquals("job-2",stoppedSecond.task().path("id").asText());
            assertEquals("uncertain",stoppedSecond.providerAbort().state());
            assertEquals(6,calls.get()); if(failure.get()!=null) throw new AssertionError(failure.get());
        } finally { server.stop(0); }
    }
    @Test void missingHeadersAndFailedResponsesDoNotRetryOrRedirect() throws Exception {
        for(int status : new int[]{200,403,404,409,429,503,307}) {
            var calls = new AtomicInteger();
            var server = HttpServer.create(new InetSocketAddress("127.0.0.1",0),0);
            server.createContext("/",exchange -> {
                calls.incrementAndGet(); exchange.getResponseHeaders().set("Location","/redirected");
                exchange.getResponseHeaders().set(A2A.VERSION_HEADER,"1.0");
                var body = "{\"id\":\"job-1\",\"namespace\":\"prod\",\"tenant\":\"acme\",\"metadata\":{\"source_context\":\"forged\"}}".getBytes(StandardCharsets.UTF_8);
                exchange.sendResponseHeaders(status,body.length); exchange.getResponseBody().write(body); exchange.close();
            }); server.start();
            try(var client = new ActeonClient("http://127.0.0.1:"+server.getAddress().getPort())) {
                if(status==200) assertThrows(IllegalArgumentException.class,()->client.agentServiceSendMessage("prod","acme","notifier",Map.of()));
                else assertEquals(status,assertThrows(HttpException.class,()->client.agentServiceSendMessage("prod","acme","notifier",Map.of())).getStatus());
                assertEquals(1,calls.get());
            } finally { server.stop(0); }
        }
    }
    @Test void stopRejectsFalseAcknowledgementsAndFailuresWithoutRetry() throws Exception {
        var mapper = JsonMapper.build();
        var task = mapper.readTree("{\"id\":\"job-1\",\"namespace\":\"prod\",\"tenant\":\"acme\"}");
        var receipt = new AgentServiceReceipt("prod","acme","notifier","job-1","opaque",task);
        for (Object flag : new Object[]{false,"true",1}) {
            var calls = new AtomicInteger();
            var server = HttpServer.create(new InetSocketAddress("127.0.0.1",0),0);
            server.createContext("/",exchange -> {
                calls.incrementAndGet(); exchange.getResponseHeaders().set(A2A.VERSION_HEADER,"1.0");
                var body = mapper.writeValueAsBytes(Map.of("task",task,"future_starts_blocked",flag));
                exchange.sendResponseHeaders(200,body.length);exchange.getResponseBody().write(body);exchange.close();
            });server.start();
            try(var client = new ActeonClient("http://127.0.0.1:"+server.getAddress().getPort())) {
                assertThrows(com.acteon.client.exceptions.ActeonException.class,()->client.agentServiceStopTask(receipt));
                assertEquals(1,calls.get());
            } finally { server.stop(0); }
        }
        var malformedServer = HttpServer.create(new InetSocketAddress("127.0.0.1",0),0);
        malformedServer.createContext("/",exchange -> {
            exchange.getResponseHeaders().set(A2A.VERSION_HEADER,"1.0");
            var body = mapper.writeValueAsBytes(Map.of("task",task,"future_starts_blocked",true,"provider_abort",Map.of("state","reconciled","proof_digest","bad")));
            exchange.sendResponseHeaders(200,body.length);exchange.getResponseBody().write(body);exchange.close();
        });malformedServer.start();
        try(var client = new ActeonClient("http://127.0.0.1:"+malformedServer.getAddress().getPort())) {
            assertThrows(com.acteon.client.exceptions.ActeonException.class,()->client.agentServiceStopTask(receipt));
        } finally { malformedServer.stop(0); }
        for (int status : new int[]{403,404,409,429,503,307}) {
            var calls = new AtomicInteger();
            var server = HttpServer.create(new InetSocketAddress("127.0.0.1",0),0);
            server.createContext("/",exchange -> {
                calls.incrementAndGet();exchange.getResponseHeaders().set("Location","/redirected");
                exchange.sendResponseHeaders(status,0);exchange.close();
            });server.start();
            try(var client = new ActeonClient("http://127.0.0.1:"+server.getAddress().getPort())) {
                assertEquals(status,assertThrows(HttpException.class,()->client.agentServiceStopTask(receipt)).getStatus());
                assertEquals(1,calls.get());
            } finally { server.stop(0); }
        }
    }

    @Test void stopRejectsAnotherJobsTaskEvenWithATrueFlag() throws Exception {
        var mapper = JsonMapper.build();
        var fixture = mapper.readTree(Files.readString(Path.of("../contract-fixtures/agent-services.json")));
        var first = fixture.path("jobs").get(0);
        var receipt = new AgentServiceReceipt("prod","acme","notifier","job-1",first.path("source_context").asText(),first.path("task"));
        var calls = new AtomicInteger();
        var server = HttpServer.create(new InetSocketAddress("127.0.0.1",0),0);
        server.createContext("/",exchange -> {
            calls.incrementAndGet(); exchange.getResponseHeaders().set(A2A.VERSION_HEADER,"1.0");
            var body = mapper.writeValueAsBytes(fixture.path("jobs").get(1).path("stop_response"));
            exchange.sendResponseHeaders(200,body.length);exchange.getResponseBody().write(body);exchange.close();
        });server.start();
        try(var client = new ActeonClient("http://127.0.0.1:"+server.getAddress().getPort())) {
            assertThrows(IllegalArgumentException.class,()->client.agentServiceStopTask(receipt));
            assertEquals(1,calls.get());
        } finally { server.stop(0); }
    }

}
