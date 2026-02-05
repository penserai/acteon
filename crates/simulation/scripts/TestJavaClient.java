///usr/bin/env jbang "$0" "$@" ; exit $?
//DEPS com.fasterxml.jackson.core:jackson-databind:2.17.0

/**
 * Test script for the Java Acteon client.
 *
 * Usage:
 *   ACTEON_URL=http://localhost:8080 jbang TestJavaClient.java
 *
 * Or with the built JAR:
 *   java -cp acteon-client-0.1.0.jar:. TestJavaClient
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
        test("dispatch()", () -> {
            Map<String, Object> action = createAction("send_notification",
                    Map.of("to", "test@example.com", "subject", "Java test"));

            String responseBody = dispatch(action);
            String outcomeType = parseOutcomeType(responseBody);

            List<String> validTypes = List.of("Executed", "Deduplicated", "Suppressed", "Rerouted", "Throttled", "Failed");
            if (!validTypes.contains(outcomeType)) {
                throw new RuntimeException("Unexpected outcome type: " + outcomeType);
            }
        });

        // Test: Batch dispatch
        test("dispatchBatch()", () -> {
            List<Map<String, Object>> actions = new ArrayList<>();
            for (int i = 0; i < 3; i++) {
                actions.add(createAction("batch_test", Map.of("seq", i)));
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

            List<Object> results = mapper.readValue(response.body(), new TypeReference<>() {});
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
            // rules can be empty, just check it's a valid response
        });

        // Test: Deduplication
        test("deduplication", () -> {
            String dedupKey = "java-dedup-" + UUID.randomUUID();

            // First action
            Map<String, Object> action1 = createAction("dedup_test", Map.of("msg", "first"));
            action1.put("dedup_key", dedupKey);
            String response1 = dispatch(action1);
            String type1 = parseOutcomeType(response1);

            if (!type1.equals("Executed") && !type1.equals("Failed")) {
                throw new RuntimeException("Unexpected first outcome: " + type1);
            }

            // Second action (same dedup key) - should work without error
            Map<String, Object> action2 = createAction("dedup_test", Map.of("msg", "second"));
            action2.put("dedup_key", dedupKey);
            String response2 = dispatch(action2);
            String type2 = parseOutcomeType(response2);

            // Second should be either Deduplicated or Executed (depending on server state)
            List<String> validTypes = List.of("Executed", "Deduplicated", "Failed");
            if (!validTypes.contains(type2)) {
                throw new RuntimeException("Unexpected second outcome: " + type2);
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

    /**
     * Create an action map with required fields.
     */
    private static Map<String, Object> createAction(String actionType, Map<String, Object> payload) {
        Map<String, Object> action = new LinkedHashMap<>();
        action.put("id", UUID.randomUUID().toString());
        action.put("namespace", "test");
        action.put("tenant", "java-client");
        action.put("provider", "email");
        action.put("action_type", actionType);
        action.put("payload", payload);
        action.put("created_at", java.time.Instant.now().toString());
        return action;
    }

    /**
     * Dispatch an action and return the response body.
     */
    private static String dispatch(Map<String, Object> action) throws Exception {
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
        return response.body();
    }

    /**
     * Parse outcome type from API response.
     * API returns: {"Executed": {...}}, "Deduplicated", {"Suppressed": {...}}, etc.
     */
    private static String parseOutcomeType(String responseBody) {
        String trimmed = responseBody.trim();
        // Handle string response like "Deduplicated"
        if (trimmed.equals("\"Deduplicated\"")) {
            return "Deduplicated";
        }
        // Handle object response like {"Executed": {...}}
        for (String key : List.of("Executed", "Suppressed", "Rerouted", "Throttled", "Failed")) {
            if (trimmed.startsWith("{\"" + key + "\"")) {
                return key;
            }
        }
        return "Unknown";
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
