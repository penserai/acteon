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

    // =========================================================================
    // Events (State Machine Lifecycle)
    // =========================================================================

    /**
     * Lists events filtered by namespace, tenant, and optionally status.
     */
    public EventListResponse listEvents(EventQuery query) throws ActeonException {
        try {
            List<String> params = new ArrayList<>();
            params.add("namespace=" + URLEncoder.encode(query.getNamespace(), StandardCharsets.UTF_8));
            params.add("tenant=" + URLEncoder.encode(query.getTenant(), StandardCharsets.UTF_8));
            if (query.getStatus() != null) {
                params.add("status=" + URLEncoder.encode(query.getStatus(), StandardCharsets.UTF_8));
            }
            if (query.getLimit() != null) {
                params.add("limit=" + query.getLimit());
            }

            String path = "/v1/events?" + String.join("&", params);
            HttpRequest request = requestBuilder(path)
                .GET()
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return parseResponse(response, EventListResponse.class);
            } else {
                throw new HttpException(response.statusCode(), "Failed to list events");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    /**
     * Gets the current state of an event by fingerprint.
     */
    public Optional<EventState> getEvent(String fingerprint, String namespace, String tenant) throws ActeonException {
        try {
            String path = "/v1/events/" + URLEncoder.encode(fingerprint, StandardCharsets.UTF_8)
                + "?namespace=" + URLEncoder.encode(namespace, StandardCharsets.UTF_8)
                + "&tenant=" + URLEncoder.encode(tenant, StandardCharsets.UTF_8);

            HttpRequest request = requestBuilder(path)
                .GET()
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return Optional.of(parseResponse(response, EventState.class));
            } else if (response.statusCode() == 404) {
                return Optional.empty();
            } else {
                throw new HttpException(response.statusCode(), "Failed to get event");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    /**
     * Transitions an event to a new state.
     */
    public TransitionResponse transitionEvent(String fingerprint, String toState, String namespace, String tenant) throws ActeonException {
        try {
            String body = objectMapper.writeValueAsString(Map.of(
                "to", toState,
                "namespace", namespace,
                "tenant", tenant
            ));
            HttpRequest request = requestBuilder("/v1/events/" + URLEncoder.encode(fingerprint, StandardCharsets.UTF_8) + "/transition")
                .PUT(HttpRequest.BodyPublishers.ofString(body))
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return parseResponse(response, TransitionResponse.class);
            } else if (response.statusCode() == 404) {
                throw new HttpException(response.statusCode(), "Event not found: " + fingerprint);
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
    // Approvals (Human-in-the-Loop)
    // =========================================================================

    /**
     * Approves a pending action by namespace, tenant, ID, and HMAC signature.
     * Does not require authentication -- the HMAC signature serves as proof of authorization.
     */
    public ApprovalActionResponse approve(String namespace, String tenant, String id, String sig) throws ActeonException {
        try {
            String path = "/v1/approvals/"
                + URLEncoder.encode(namespace, StandardCharsets.UTF_8) + "/"
                + URLEncoder.encode(tenant, StandardCharsets.UTF_8) + "/"
                + URLEncoder.encode(id, StandardCharsets.UTF_8) + "/approve"
                + "?sig=" + URLEncoder.encode(sig, StandardCharsets.UTF_8);

            HttpRequest request = requestBuilder(path)
                .POST(HttpRequest.BodyPublishers.noBody())
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return parseResponse(response, ApprovalActionResponse.class);
            } else if (response.statusCode() == 404) {
                throw new HttpException(response.statusCode(), "Approval not found or expired");
            } else if (response.statusCode() == 410) {
                throw new HttpException(response.statusCode(), "Approval already decided");
            } else {
                throw new HttpException(response.statusCode(), "Failed to approve");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    /**
     * Rejects a pending action by namespace, tenant, ID, and HMAC signature.
     * Does not require authentication -- the HMAC signature serves as proof of authorization.
     */
    public ApprovalActionResponse reject(String namespace, String tenant, String id, String sig) throws ActeonException {
        try {
            String path = "/v1/approvals/"
                + URLEncoder.encode(namespace, StandardCharsets.UTF_8) + "/"
                + URLEncoder.encode(tenant, StandardCharsets.UTF_8) + "/"
                + URLEncoder.encode(id, StandardCharsets.UTF_8) + "/reject"
                + "?sig=" + URLEncoder.encode(sig, StandardCharsets.UTF_8);

            HttpRequest request = requestBuilder(path)
                .POST(HttpRequest.BodyPublishers.noBody())
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return parseResponse(response, ApprovalActionResponse.class);
            } else if (response.statusCode() == 404) {
                throw new HttpException(response.statusCode(), "Approval not found or expired");
            } else if (response.statusCode() == 410) {
                throw new HttpException(response.statusCode(), "Approval already decided");
            } else {
                throw new HttpException(response.statusCode(), "Failed to reject");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    /**
     * Gets the status of an approval by namespace, tenant, ID, and HMAC signature.
     * Returns empty if not found or expired.
     */
    public Optional<ApprovalStatus> getApproval(String namespace, String tenant, String id, String sig) throws ActeonException {
        try {
            String path = "/v1/approvals/"
                + URLEncoder.encode(namespace, StandardCharsets.UTF_8) + "/"
                + URLEncoder.encode(tenant, StandardCharsets.UTF_8) + "/"
                + URLEncoder.encode(id, StandardCharsets.UTF_8)
                + "?sig=" + URLEncoder.encode(sig, StandardCharsets.UTF_8);

            HttpRequest request = requestBuilder(path)
                .GET()
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return Optional.of(parseResponse(response, ApprovalStatus.class));
            } else if (response.statusCode() == 404) {
                return Optional.empty();
            } else {
                throw new HttpException(response.statusCode(), "Failed to get approval");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    /**
     * Lists pending approvals filtered by namespace and tenant.
     * Requires authentication.
     */
    public ApprovalListResponse listApprovals(String namespace, String tenant) throws ActeonException {
        try {
            String path = "/v1/approvals?"
                + "namespace=" + URLEncoder.encode(namespace, StandardCharsets.UTF_8)
                + "&tenant=" + URLEncoder.encode(tenant, StandardCharsets.UTF_8);

            HttpRequest request = requestBuilder(path)
                .GET()
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return parseResponse(response, ApprovalListResponse.class);
            } else {
                throw new HttpException(response.statusCode(), "Failed to list approvals");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    // =========================================================================
    // Groups (Event Batching)
    // =========================================================================

    /**
     * Lists all active event groups.
     */
    public GroupListResponse listGroups() throws ActeonException {
        try {
            HttpRequest request = requestBuilder("/v1/groups")
                .GET()
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return parseResponse(response, GroupListResponse.class);
            } else {
                throw new HttpException(response.statusCode(), "Failed to list groups");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    /**
     * Gets details of a specific group.
     */
    public Optional<GroupDetail> getGroup(String groupKey) throws ActeonException {
        try {
            HttpRequest request = requestBuilder("/v1/groups/" + URLEncoder.encode(groupKey, StandardCharsets.UTF_8))
                .GET()
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return Optional.of(parseResponse(response, GroupDetail.class));
            } else if (response.statusCode() == 404) {
                return Optional.empty();
            } else {
                throw new HttpException(response.statusCode(), "Failed to get group");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    /**
     * Forces a group to flush, triggering immediate notification.
     */
    public FlushGroupResponse flushGroup(String groupKey) throws ActeonException {
        try {
            HttpRequest request = requestBuilder("/v1/groups/" + URLEncoder.encode(groupKey, StandardCharsets.UTF_8))
                .DELETE()
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return parseResponse(response, FlushGroupResponse.class);
            } else if (response.statusCode() == 404) {
                throw new HttpException(response.statusCode(), "Group not found: " + groupKey);
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
}
