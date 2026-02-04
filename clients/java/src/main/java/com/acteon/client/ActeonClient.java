package com.acteon.client;

import com.acteon.client.exceptions.*;
import com.acteon.client.models.*;
import com.fasterxml.jackson.core.type.TypeReference;
import com.fasterxml.jackson.databind.ObjectMapper;

import java.io.IOException;
import java.net.URI;
import java.net.URLEncoder;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Optional;

/**
 * HTTP client for the Acteon action gateway.
 *
 * <p>Example usage:</p>
 * <pre>{@code
 * ActeonClient client = new ActeonClient("http://localhost:8080");
 *
 * if (client.health()) {
 *     Action action = new Action("ns", "tenant", "email", "send", Map.of("to", "user@example.com"));
 *     ActionOutcome outcome = client.dispatch(action);
 *     System.out.println("Outcome: " + outcome.getType());
 * }
 * }</pre>
 */
public class ActeonClient implements AutoCloseable {
    private final String baseUrl;
    private final HttpClient httpClient;
    private final ObjectMapper objectMapper;
    private final String apiKey;

    /**
     * Creates a new Acteon client with default settings.
     */
    public ActeonClient(String baseUrl) {
        this(baseUrl, null, Duration.ofSeconds(30));
    }

    /**
     * Creates a new Acteon client with an API key.
     */
    public ActeonClient(String baseUrl, String apiKey) {
        this(baseUrl, apiKey, Duration.ofSeconds(30));
    }

    /**
     * Creates a new Acteon client with custom settings.
     */
    public ActeonClient(String baseUrl, String apiKey, Duration timeout) {
        this.baseUrl = baseUrl.replaceAll("/$", "");
        this.apiKey = apiKey;
        this.objectMapper = new ObjectMapper();
        this.httpClient = HttpClient.newBuilder()
            .connectTimeout(timeout)
            .build();
    }

    private HttpRequest.Builder requestBuilder(String path) {
        HttpRequest.Builder builder = HttpRequest.newBuilder()
            .uri(URI.create(baseUrl + path))
            .header("Content-Type", "application/json");

        if (apiKey != null && !apiKey.isEmpty()) {
            builder.header("Authorization", "Bearer " + apiKey);
        }

        return builder;
    }

    private <T> T parseResponse(HttpResponse<String> response, Class<T> clazz) throws IOException {
        return objectMapper.readValue(response.body(), clazz);
    }

    private <T> T parseResponse(HttpResponse<String> response, TypeReference<T> typeRef) throws IOException {
        return objectMapper.readValue(response.body(), typeRef);
    }

    @Override
    public void close() {
        // HttpClient doesn't need explicit closing in Java 11+
    }

    // =========================================================================
    // Health
    // =========================================================================

    /**
     * Checks if the server is healthy.
     */
    public boolean health() {
        try {
            HttpRequest request = requestBuilder("/health")
                .GET()
                .build();
            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());
            return response.statusCode() == 200;
        } catch (Exception e) {
            return false;
        }
    }

    // =========================================================================
    // Action Dispatch
    // =========================================================================

    /**
     * Dispatches a single action.
     */
    public ActionOutcome dispatch(Action action) throws ActeonException {
        try {
            String body = objectMapper.writeValueAsString(action);
            HttpRequest request = requestBuilder("/v1/dispatch")
                .POST(HttpRequest.BodyPublishers.ofString(body))
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                Map<String, Object> data = objectMapper.readValue(
                    response.body(),
                    new TypeReference<Map<String, Object>>() {}
                );
                return ActionOutcome.fromMap(data);
            } else {
                ErrorResponse error = parseResponse(response, ErrorResponse.class);
                throw new ApiException(error.getCode(), error.getMessage(), error.isRetryable());
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    /**
     * Dispatches multiple actions in a single request.
     */
    public List<BatchResult> dispatchBatch(List<Action> actions) throws ActeonException {
        try {
            String body = objectMapper.writeValueAsString(actions);
            HttpRequest request = requestBuilder("/v1/dispatch/batch")
                .POST(HttpRequest.BodyPublishers.ofString(body))
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                List<Map<String, Object>> data = objectMapper.readValue(
                    response.body(),
                    new TypeReference<List<Map<String, Object>>>() {}
                );
                List<BatchResult> results = new ArrayList<>();
                for (Map<String, Object> item : data) {
                    results.add(BatchResult.fromMap(item));
                }
                return results;
            } else {
                ErrorResponse error = parseResponse(response, ErrorResponse.class);
                throw new ApiException(error.getCode(), error.getMessage(), error.isRetryable());
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    // =========================================================================
    // Rules Management
    // =========================================================================

    /**
     * Lists all loaded rules.
     */
    public List<RuleInfo> listRules() throws ActeonException {
        try {
            HttpRequest request = requestBuilder("/v1/rules")
                .GET()
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return parseResponse(response, new TypeReference<List<RuleInfo>>() {});
            } else {
                throw new HttpException(response.statusCode(), "Failed to list rules");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    /**
     * Reloads rules from the configured directory.
     */
    public ReloadResult reloadRules() throws ActeonException {
        try {
            HttpRequest request = requestBuilder("/v1/rules/reload")
                .POST(HttpRequest.BodyPublishers.noBody())
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return parseResponse(response, ReloadResult.class);
            } else {
                throw new HttpException(response.statusCode(), "Failed to reload rules");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    /**
     * Enables or disables a specific rule.
     */
    public void setRuleEnabled(String ruleName, boolean enabled) throws ActeonException {
        try {
            String body = objectMapper.writeValueAsString(Map.of("enabled", enabled));
            HttpRequest request = requestBuilder("/v1/rules/" + URLEncoder.encode(ruleName, StandardCharsets.UTF_8) + "/enabled")
                .PUT(HttpRequest.BodyPublishers.ofString(body))
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() != 200) {
                throw new HttpException(response.statusCode(), "Failed to set rule enabled");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    // =========================================================================
    // Audit Trail
    // =========================================================================

    /**
     * Queries audit records.
     */
    public AuditPage queryAudit(AuditQuery query) throws ActeonException {
        try {
            StringBuilder path = new StringBuilder("/v1/audit");
            List<String> params = new ArrayList<>();

            if (query != null) {
                if (query.getNamespace() != null) {
                    params.add("namespace=" + URLEncoder.encode(query.getNamespace(), StandardCharsets.UTF_8));
                }
                if (query.getTenant() != null) {
                    params.add("tenant=" + URLEncoder.encode(query.getTenant(), StandardCharsets.UTF_8));
                }
                if (query.getProvider() != null) {
                    params.add("provider=" + URLEncoder.encode(query.getProvider(), StandardCharsets.UTF_8));
                }
                if (query.getActionType() != null) {
                    params.add("action_type=" + URLEncoder.encode(query.getActionType(), StandardCharsets.UTF_8));
                }
                if (query.getOutcome() != null) {
                    params.add("outcome=" + URLEncoder.encode(query.getOutcome(), StandardCharsets.UTF_8));
                }
                if (query.getLimit() != null) {
                    params.add("limit=" + query.getLimit());
                }
                if (query.getOffset() != null) {
                    params.add("offset=" + query.getOffset());
                }
            }

            if (!params.isEmpty()) {
                path.append("?").append(String.join("&", params));
            }

            HttpRequest request = requestBuilder(path.toString())
                .GET()
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return parseResponse(response, AuditPage.class);
            } else {
                throw new HttpException(response.statusCode(), "Failed to query audit");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    /**
     * Gets a specific audit record by action ID.
     */
    public Optional<AuditRecord> getAuditRecord(String actionId) throws ActeonException {
        try {
            HttpRequest request = requestBuilder("/v1/audit/" + URLEncoder.encode(actionId, StandardCharsets.UTF_8))
                .GET()
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return Optional.of(parseResponse(response, AuditRecord.class));
            } else if (response.statusCode() == 404) {
                return Optional.empty();
            } else {
                throw new HttpException(response.statusCode(), "Failed to get audit record");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }
}
