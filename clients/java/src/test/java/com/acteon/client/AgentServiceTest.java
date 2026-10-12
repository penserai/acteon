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
    @Test void officialStreamsPreserveRequestIdentityAndSourceOutOfBand() throws Exception {
        var mapper = JsonMapper.build();
        var task = mapper.readTree("{\"id\":\"job-1\",\"namespace\":\"prod\",\"tenant\":\"acme\"}");
        var receipt = new AgentServiceReceipt("prod","acme","notifier","job-1","host-only",task);
        var calls = new AtomicInteger(); var failure = new AtomicReference<Throwable>();
        var server = HttpServer.create(new InetSocketAddress("127.0.0.1",0),0);
        server.createContext("/",exchange -> {
            try {
                calls.incrementAndGet();
                assertEquals("/a2a/prod/acme/agents/notifier", exchange.getRequestURI().getPath());
                assertEquals("text/event-stream", exchange.getRequestHeaders().getFirst("Accept"));
                var request = mapper.readTree(exchange.getRequestBody());
                if (request.path("method").asText().equals("SubscribeToTask")) {
                    assertEquals("job-1",request.path("params").path("id").asText());
                    assertEquals("host-only",exchange.getRequestHeaders().getFirst(AgentServiceReceipt.SOURCE_CONTEXT_HEADER));
                } else {
                    assertEquals("SendStreamingMessage",request.path("method").asText());
                    assertNull(exchange.getRequestHeaders().getFirst(AgentServiceReceipt.SOURCE_CONTEXT_HEADER));
                }
                exchange.getResponseHeaders().set(A2A.VERSION_HEADER,"1.0");
                exchange.getResponseHeaders().set("Content-Type","text/event-stream");
                if (request.path("method").asText().equals("SendStreamingMessage"))
                    exchange.getResponseHeaders().set(AgentServiceReceipt.SOURCE_CONTEXT_HEADER,"host-only");
                var frame = mapper.createObjectNode();
                frame.put("jsonrpc","2.0"); frame.set("id",request.path("id"));
                frame.set("result",mapper.readTree("{\"task\":{\"id\":\"job-1\"}}"));
                var body = (": keepalive\n\ndata: "+mapper.writeValueAsString(frame)+"\n\n").getBytes(StandardCharsets.UTF_8);
                exchange.sendResponseHeaders(200,body.length); exchange.getResponseBody().write(body); exchange.close();
            } catch(Throwable error) { failure.set(error); exchange.close(); }
        }); server.start();
        try(var client = new ActeonClient("http://127.0.0.1:"+server.getAddress().getPort())) {
            try(var stream = client.agentServiceSendStreamingMessage("prod","acme","notifier",Map.of("messageId","m1"))) {
                assertEquals("host-only",stream.sourceContext());
                assertEquals("job-1",stream.next().path("task").path("id").asText());
            }
            try(var stream = client.agentServiceSubscribeTask(receipt)) {
                assertNull(stream.sourceContext());
                assertEquals("job-1",stream.next().path("task").path("id").asText());
            }
            assertEquals(2,calls.get()); if(failure.get()!=null) throw new AssertionError(failure.get());
        } finally { server.stop(0); }
    }

    @Test void officialCancelPreservesRequestIdentityAndSource() throws Exception {
        var mapper = JsonMapper.build();
        var task = mapper.readTree("{\"id\":\"job-1\",\"namespace\":\"prod\",\"tenant\":\"acme\"}");
        var receipt = new AgentServiceReceipt("prod","acme","notifier","job-1","host-only",task);
        var failure = new AtomicReference<Throwable>();
        var server = HttpServer.create(new InetSocketAddress("127.0.0.1",0),0);
        server.createContext("/",exchange -> {
            try {
                assertEquals("/a2a/prod/acme/agents/notifier", exchange.getRequestURI().getPath());
                assertEquals("host-only", exchange.getRequestHeaders().getFirst(AgentServiceReceipt.SOURCE_CONTEXT_HEADER));
                var request = mapper.readTree(exchange.getRequestBody());
                assertEquals("CancelTask", request.path("method").asText());
                assertEquals("cancel:job-1", request.path("id").asText());
                exchange.getResponseHeaders().set(A2A.VERSION_HEADER,"1.0");
                var body = "{\"jsonrpc\":\"2.0\",\"id\":\"cancel:job-1\",\"result\":{\"id\":\"job-1\",\"status\":{\"state\":\"TASK_STATE_CANCELED\"},\"metadata\":{\"acteonCancellation\":{\"state\":\"confirmed\"}}}}".getBytes(StandardCharsets.UTF_8);
                exchange.sendResponseHeaders(200,body.length); exchange.getResponseBody().write(body); exchange.close();
            } catch(Throwable error) { failure.set(error); exchange.close(); }
        }); server.start();
        try(var client = new ActeonClient("http://127.0.0.1:"+server.getAddress().getPort())) {
            assertEquals("job-1", client.agentServiceCancelTask(receipt).path("id").asText());
            if(failure.get()!=null) throw new AssertionError(failure.get());
        } finally { server.stop(0); }
    }

    @Test void officialListTasksReturnsTypedSourceBoundPage() throws Exception {
        var mapper = JsonMapper.build();
        var task = mapper.readTree("{\"id\":\"job-1\",\"namespace\":\"prod\",\"tenant\":\"acme\"}");
        var receipt = new AgentServiceReceipt("prod","acme","notifier","job-1","host-only",task);
        var failure = new AtomicReference<Throwable>();
        var server = HttpServer.create(new InetSocketAddress("127.0.0.1",0),0);
        server.createContext("/",exchange -> {
            try {
                assertEquals("/a2a/prod/acme/agents/notifier", exchange.getRequestURI().getPath());
                assertEquals("host-only", exchange.getRequestHeaders().getFirst(AgentServiceReceipt.SOURCE_CONTEXT_HEADER));
                var request = mapper.readTree(exchange.getRequestBody());
                assertEquals("ListTasks", request.path("method").asText());
                assertEquals("list:notifier", request.path("id").asText());
                assertEquals("TASK_STATE_WORKING", request.path("params").path("status").asText());
                assertEquals(1, request.path("params").path("pageSize").asInt());
                exchange.getResponseHeaders().set(A2A.VERSION_HEADER,"1.0");
                var body = "{\"jsonrpc\":\"2.0\",\"id\":\"list:notifier\",\"result\":{\"tasks\":[{\"id\":\"job-1\",\"status\":{\"state\":\"TASK_STATE_WORKING\"}}],\"nextPageToken\":\"next-1\",\"pageSize\":1,\"totalSize\":2}}".getBytes(StandardCharsets.UTF_8);
                exchange.sendResponseHeaders(200,body.length); exchange.getResponseBody().write(body); exchange.close();
            } catch(Throwable error) { failure.set(error); exchange.close(); }
        }); server.start();
        try(var client = new ActeonClient("http://127.0.0.1:"+server.getAddress().getPort())) {
            var options = new AgentServiceListOptions(receipt,null,AgentServiceTaskState.WORKING,1,null,null,null,false);
            var page = client.agentServiceListTasks("prod","acme","notifier",options);
            assertEquals("job-1",page.tasks().get(0).path("id").asText());
            assertEquals("next-1",page.nextPageToken());
            assertEquals(2,page.totalSize());
            if(failure.get()!=null) throw new AssertionError(failure.get());
        } finally { server.stop(0); }
    }

    @Test void officialPushConfigCrudIsTypedAndTaskBound() throws Exception {
        var mapper = JsonMapper.build();
        var task = mapper.readTree("{\"id\":\"job-1\",\"namespace\":\"prod\",\"tenant\":\"acme\"}");
        var receipt = new AgentServiceReceipt("prod","acme","notifier","job-1","host-only",task);
        var failure = new AtomicReference<Throwable>(); var calls = new AtomicInteger();
        var server = HttpServer.create(new InetSocketAddress("127.0.0.1",0),0);
        server.createContext("/",exchange -> {
            try {
                calls.incrementAndGet();
                assertEquals("host-only", exchange.getRequestHeaders().getFirst(AgentServiceReceipt.SOURCE_CONTEXT_HEADER));
                var request = mapper.readTree(exchange.getRequestBody());
                assertEquals("job-1", request.path("params").path("taskId").asText());
                var stored = mapper.readTree("{\"id\":\"push-1\",\"taskId\":\"job-1\",\"url\":\"https://hooks.example/a2a\",\"token\":\"callback-token\",\"authentication\":{\"scheme\":\"Bearer\",\"credentials\":\"secret\"}}");
                com.fasterxml.jackson.databind.JsonNode result;
                if (request.path("method").asText().equals("ListTaskPushNotificationConfigs")) {
                    result = mapper.createObjectNode().set("configs", mapper.createArrayNode().add(stored));
                    ((com.fasterxml.jackson.databind.node.ObjectNode) result).put("nextPageToken", "next");
                } else if (request.path("method").asText().equals("DeleteTaskPushNotificationConfig")) {
                    result = mapper.createObjectNode();
                } else {
                    result = stored;
                }
                var envelope = mapper.createObjectNode(); envelope.put("jsonrpc","2.0");
                envelope.set("id", request.path("id")); envelope.set("result", result);
                exchange.getResponseHeaders().set(A2A.VERSION_HEADER,"1.0");
                var body = mapper.writeValueAsBytes(envelope);
                exchange.sendResponseHeaders(200,body.length); exchange.getResponseBody().write(body); exchange.close();
            } catch(Throwable error) { failure.set(error); exchange.close(); }
        }); server.start();
        try(var client = new ActeonClient("http://127.0.0.1:"+server.getAddress().getPort())) {
            var input = new AgentServicePushConfigInput("push-1", "https://hooks.example/a2a", "callback-token", new AgentServicePushAuthentication("Bearer", "secret"));
            var created = client.agentServiceCreatePushConfig("prod","acme","notifier","job-1",input,receipt);
            assertEquals("secret", created.authentication().credentials());
            assertEquals("push-1", client.agentServiceGetPushConfig("prod","acme","notifier","job-1","push-1",receipt).id());
            assertEquals("next", client.agentServiceListPushConfigs("prod","acme","notifier","job-1",1,null,receipt).nextPageToken());
            client.agentServiceDeletePushConfig("prod","acme","notifier","job-1","push-1",receipt);
            assertEquals(4,calls.get());
            var foreign = new AgentServiceReceipt("prod","acme","notifier","job-2","host-only",task);
            assertThrows(IllegalArgumentException.class, () -> client.agentServiceGetPushConfig("prod","acme","notifier","job-1","push-1",foreign));
            assertEquals(4,calls.get()); if(failure.get()!=null) throw new AssertionError(failure.get());
        } finally { server.stop(0); }
    }

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
                } else {
                    assertTrue(exchange.getRequestURI().getRawPath().contains("/peers/team%2Fresolver/diagnose/"));
                    if (exchange.getRequestURI().getPath().endsWith("/authorization:resolve"))
                        assertEquals("challenge-42", mapper.readTree(exchange.getRequestBody()).path("challengeId").asText());
                }
            } catch(Throwable error) { failure.set(error); }
            exchange.getResponseHeaders().set(A2A.VERSION_HEADER,"1.0");
            var body = (exchange.getRequestMethod().equals("GET")
                ? "{\"peers\":[{\"agent_id\":\"resolver\",\"skill\":\"diagnose\",\"description_untrusted\":\"Investigates incidents\",\"card_version\":\"v1\",\"binding_digest\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"checked_at_ms\":42}]}"
                : exchange.getRequestURI().getPath().endsWith(":cancel")
                ? "{\"submission_id\":\"f47ac10b-58cc-5372-a567-0e02b2c3d479\",\"cancellation_id\":\"67e55044-10b1-526f-9247-bb680e5fe0c8\",\"status\":{\"state\":\"restricted\",\"task\":{\"id\":\"remote-1\",\"namespace\":\"prod\",\"tenant\":\"acme\",\"status\":{\"state\":\"submitted\"}}}}"
                : exchange.getRequestURI().getPath().contains("/submissions/") && exchange.getRequestURI().getPath().endsWith("/message:send")
                ? "{\"submission_id\":\"f47ac10b-58cc-5372-a567-0e02b2c3d479\",\"continuation_id\":\"67e55044-10b1-526f-9247-bb680e5fe0c8\",\"status\":{\"state\":\"accepted\",\"task\":{\"id\":\"remote-1\",\"namespace\":\"prod\",\"tenant\":\"acme\"},\"progress_cursor\":\"\\\"remote-1:2\\\"\"}}"
                : exchange.getRequestURI().getPath().endsWith("/authorization:resolve")
                ? "{\"submission_id\":\"f47ac10b-58cc-5372-a567-0e02b2c3d479\",\"authorization_id\":\"67e55044-10b1-526f-9247-bb680e5fe0c8\",\"status\":{\"state\":\"resolved\",\"task\":{\"id\":\"remote-1\",\"namespace\":\"prod\",\"tenant\":\"acme\",\"status\":{\"state\":\"working\"}},\"progress_cursor\":\"\\\"remote-1:2\\\"\"}}"
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
            assertEquals("restricted",canceled.state());
            var continued = client.agentServiceContinuePeer(source,"team/resolver","diagnose",receipt,Map.of("messageId","answer-1","role","user"));
            assertEquals("accepted",continued.state());
            assertEquals("\"remote-1:2\"",continued.progressCursor());
            assertThrows(IllegalArgumentException.class,()->client.agentServiceContinuePeer(source,"team/resolver","diagnose",receipt,Map.of("messageId","answer-1","role","user","taskId","forged")));
            var authorized = client.agentServiceAuthorizePeer(source,"team/resolver","diagnose",receipt,"challenge-42");
            assertEquals("resolved",authorized.state());
            assertEquals("\"remote-1:2\"",authorized.progressCursor());
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
                var body = mapper.writeValueAsBytes(Map.of("task",task,"future_starts_blocked",flag,"cancellation_confirmed",false));
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
            var body = mapper.writeValueAsBytes(Map.of("task",task,"future_starts_blocked",true,"cancellation_confirmed",false,"provider_abort",Map.of("state","reconciled","proof_digest","bad")));
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

    @Test void authorizationCallsCarryOnlyOpaqueHandles() throws Exception {
        var mapper = JsonMapper.build();
        var task = mapper.readTree("{\"id\":\"job-1\",\"namespace\":\"prod\",\"tenant\":\"acme\",\"status\":{\"state\":\"auth_required\"}}");
        var receipt = new AgentServiceReceipt("prod","acme","notifier","job-1","host-only",task);
        var calls = new AtomicInteger(); var failure = new AtomicReference<Throwable>();
        var server = HttpServer.create(new InetSocketAddress("127.0.0.1",0),0);
        server.createContext("/",exchange -> {
            try {
                calls.incrementAndGet(); var body = mapper.readTree(exchange.getRequestBody());
                if(exchange.getRequestURI().getPath().endsWith("authorization:request")) {
                    assertEquals(mapper.readTree("{\"authorizationRequestId\":\"opaque-flow-42\"}"),body);
                    assertNull(exchange.getRequestHeaders().getFirst(AgentServiceReceipt.SOURCE_CONTEXT_HEADER));
                } else {
                    assertEquals(mapper.readTree("{\"challengeId\":\"challenge-42\"}"),body);
                    assertEquals("host-only",exchange.getRequestHeaders().getFirst(AgentServiceReceipt.SOURCE_CONTEXT_HEADER));
                }
            } catch(Throwable error) { failure.set(error); }
            exchange.getResponseHeaders().set(A2A.VERSION_HEADER,"1.0");
            var bytes=mapper.writeValueAsBytes(task);exchange.sendResponseHeaders(200,bytes.length);exchange.getResponseBody().write(bytes);exchange.close();
        });server.start();
        try(var client = new ActeonClient("http://127.0.0.1:"+server.getAddress().getPort())) {
            client.agentServiceRequestAuthorization("prod","acme","notifier","job-1","opaque-flow-42");
            client.agentServiceResolveAuthorization(receipt,"challenge-42");
            assertEquals(2,calls.get()); if(failure.get()!=null) throw new AssertionError(failure.get());
        } finally { server.stop(0); }
    }
}
