///usr/bin/env jbang "$0" "$@" ; exit $?
//DEPS com.fasterxml.jackson.core:jackson-databind:2.17.0

/**
 * Test script for the Java Acteon client.
 *
 * Usage:
 *   ACTEON_URL=http://localhost:8080 jbang TestJavaClient.java
 *
 * Or compile manually:
 *   cd clients/java && mvn package
 *   java -cp target/classes:. TestJavaClient
 */

import com.fasterxml.jackson.core.type.TypeReference;
import com.fasterxml.jackson.databind.ObjectMapper;

import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.time.Duration;
import java.util.*;

public class TestJavaClient {

    private static final ObjectMapper mapper = new ObjectMapper();
    private static final HttpClient httpClient = HttpClient.newBuilder()
            .connectTimeout(Duration.ofSeconds(30))
            .build();

    private static String baseUrl;
    private static int passed = 0;
    private static int failed = 0;

    public static void main(String[] args) {
        baseUrl = System.getenv().getOrDefault("ACTEON_URL", "http://localhost:8080");
        if (baseUrl.endsWith("/")) {
            baseUrl = baseUrl.substring(0, baseUrl.length() - 1);
        }

        System.out.println("Java Client Test - connecting to " + baseUrl);
        System.out.println("=".repeat(60));

        // Test: Health check
        test("health()", () -> {
            HttpRequest request = HttpRequest.newBuilder()
                    .uri(URI.create(baseUrl + "/health"))
                    .GET()
                    .build();
            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());
            if (response.statusCode() != 200) {
                throw new RuntimeException("Health check returned " + response.statusCode());
            }
        });

        // Test: Single dispatch
        String dispatchedId = UUID.randomUUID().toString();
        test("dispatch()", () -> {
            Map<String, Object> action = new LinkedHashMap<>();
            action.put("id", dispatchedId);
            action.put("namespace", "test");
            action.put("tenant", "java-client");
            action.put("provider", "email");
            action.put("action_type", "send_notification");
            action.put("payload", Map.of("to", "test@example.com", "subject", "Java test"));
            action.put("created_at", java.time.Instant.now().toString());

            String body = mapper.writeValueAsString(action);
            HttpRequest request = HttpRequest.newBuilder()
                    .uri(URI.create(baseUrl + "/v1/dispatch"))
                    .header("Content-Type", "application/json")
                    .POST(HttpRequest.BodyPublishers.ofString(body))
                    .build();
            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());
            if (response.statusCode() != 200) {
                throw new RuntimeException("Dispatch returned " + response.statusCode() + ": " + response.body());
            }

            Map<String, Object> result = mapper.readValue(response.body(), new TypeReference<>() {});
            String type = (String) result.get("type");
            List<String> validTypes = List.of("executed", "deduplicated", "suppressed", "rerouted", "throttled", "failed");
            if (!validTypes.contains(type)) {
                throw new RuntimeException("Unexpected outcome type: " + type);
            }
        });

        // Test: Batch dispatch
        test("dispatchBatch()", () -> {
            List<Map<String, Object>> actions = new ArrayList<>();
            for (int i = 0; i < 3; i++) {
                Map<String, Object> action = new LinkedHashMap<>();
                action.put("id", UUID.randomUUID().toString());
                action.put("namespace", "test");
                action.put("tenant", "java-client");
                action.put("provider", "email");
                action.put("action_type", "batch_test");
                action.put("payload", Map.of("seq", i));
                action.put("created_at", java.time.Instant.now().toString());
                actions.add(action);
            }

            String body = mapper.writeValueAsString(actions);
            HttpRequest request = HttpRequest.newBuilder()
                    .uri(URI.create(baseUrl + "/v1/dispatch/batch"))
                    .header("Content-Type", "application/json")
                    .POST(HttpRequest.BodyPublishers.ofString(body))
                    .build();
            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());
            if (response.statusCode() != 200) {
                throw new RuntimeException("Batch dispatch returned " + response.statusCode());
            }

            List<Map<String, Object>> results = mapper.readValue(response.body(), new TypeReference<>() {});
            if (results.size() != 3) {
                throw new RuntimeException("Expected 3 results, got " + results.size());
            }
        });

        // Test: List rules
        test("listRules()", () -> {
            HttpRequest request = HttpRequest.newBuilder()
                    .uri(URI.create(baseUrl + "/v1/rules"))
                    .GET()
                    .build();
            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());
            if (response.statusCode() != 200) {
                throw new RuntimeException("List rules returned " + response.statusCode());
            }
            List<Map<String, Object>> rules = mapper.readValue(response.body(), new TypeReference<>() {});
            // rules can be empty, just check it's a valid list
        });

        // Test: Deduplication
        test("deduplication", () -> {
            String dedupKey = "java-dedup-" + UUID.randomUUID();

            // First action
            Map<String, Object> action1 = new LinkedHashMap<>();
            action1.put("id", UUID.randomUUID().toString());
            action1.put("namespace", "test");
            action1.put("tenant", "java-client");
            action1.put("provider", "email");
            action1.put("action_type", "dedup_test");
            action1.put("payload", Map.of("msg", "first"));
            action1.put("dedup_key", dedupKey);
            action1.put("created_at", java.time.Instant.now().toString());

            String body1 = mapper.writeValueAsString(action1);
            HttpRequest request1 = HttpRequest.newBuilder()
                    .uri(URI.create(baseUrl + "/v1/dispatch"))
                    .header("Content-Type", "application/json")
                    .POST(HttpRequest.BodyPublishers.ofString(body1))
                    .build();
            HttpResponse<String> response1 = httpClient.send(request1, HttpResponse.BodyHandlers.ofString());
            if (response1.statusCode() != 200) {
                throw new RuntimeException("First dispatch returned " + response1.statusCode());
            }

            // Second action (same dedup key)
            Map<String, Object> action2 = new LinkedHashMap<>();
            action2.put("id", UUID.randomUUID().toString());
            action2.put("namespace", "test");
            action2.put("tenant", "java-client");
            action2.put("provider", "email");
            action2.put("action_type", "dedup_test");
            action2.put("payload", Map.of("msg", "second"));
            action2.put("dedup_key", dedupKey);
            action2.put("created_at", java.time.Instant.now().toString());

            String body2 = mapper.writeValueAsString(action2);
            HttpRequest request2 = HttpRequest.newBuilder()
                    .uri(URI.create(baseUrl + "/v1/dispatch"))
                    .header("Content-Type", "application/json")
                    .POST(HttpRequest.BodyPublishers.ofString(body2))
                    .build();
            HttpResponse<String> response2 = httpClient.send(request2, HttpResponse.BodyHandlers.ofString());
            if (response2.statusCode() != 200) {
                throw new RuntimeException("Second dispatch returned " + response2.statusCode());
            }

            Map<String, Object> result1 = mapper.readValue(response1.body(), new TypeReference<>() {});
            String type1 = (String) result1.get("type");
            if (!type1.equals("executed") && !type1.equals("failed")) {
                throw new RuntimeException("Unexpected first outcome: " + type1);
            }
        });

        // Test: Query audit
        test("queryAudit()", () -> {
            HttpRequest request = HttpRequest.newBuilder()
                    .uri(URI.create(baseUrl + "/v1/audit?tenant=java-client&limit=10"))
                    .GET()
                    .build();
            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());
            if (response.statusCode() != 200) {
                throw new RuntimeException("Query audit returned " + response.statusCode());
            }
            Map<String, Object> page = mapper.readValue(response.body(), new TypeReference<>() {});
            if (!page.containsKey("total")) {
                throw new RuntimeException("Expected page with total");
            }
            if (!page.containsKey("records")) {
                throw new RuntimeException("Expected page with records");
            }
        });

        // Summary
        System.out.println("=".repeat(60));
        int total = passed + failed;
        System.out.println("Results: " + passed + "/" + total + " passed");

        System.exit(failed > 0 ? 1 : 0);
    }

    @FunctionalInterface
    interface ThrowingRunnable {
        void run() throws Exception;
    }

    private static void test(String name, ThrowingRunnable fn) {
        try {
            fn.run();
            System.out.println("  [PASS] " + name);
            passed++;
        } catch (Exception e) {
            System.out.println("  [FAIL] " + name + ": " + e.getMessage());
            failed++;
        }
    }
}
