package com.acteon.client;

import com.acteon.client.exceptions.*;
import com.acteon.client.models.*;
import com.fasterxml.jackson.core.type.TypeReference;
import com.fasterxml.jackson.databind.ObjectMapper;

import java.io.IOException;
import java.io.InputStream;
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
     * Dispatches a single action in dry-run mode.
     * Rules are evaluated but the action is not executed and no state is mutated.
     */
    public ActionOutcome dispatchDryRun(Action action) throws ActeonException {
        try {
            String body = objectMapper.writeValueAsString(action);
            HttpRequest request = requestBuilder("/v1/dispatch?dry_run=true")
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

    /**
     * Dispatches multiple actions in dry-run mode.
     * Rules are evaluated for each action but none are executed and no state is mutated.
     */
    public List<BatchResult> dispatchBatchDryRun(List<Action> actions) throws ActeonException {
        try {
            String body = objectMapper.writeValueAsString(actions);
            HttpRequest request = requestBuilder("/v1/dispatch/batch?dry_run=true")
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
    // Audit Replay
    // =========================================================================

    /**
     * Replays a single action from the audit trail by its action ID.
     * The action is reconstructed from the stored payload and dispatched with a new ID.
     */
    public ReplayResult replayAction(String actionId) throws ActeonException {
        try {
            HttpRequest request = requestBuilder("/v1/audit/" + URLEncoder.encode(actionId, StandardCharsets.UTF_8) + "/replay")
                .POST(HttpRequest.BodyPublishers.noBody())
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return parseResponse(response, ReplayResult.class);
            } else if (response.statusCode() == 404) {
                throw new HttpException(response.statusCode(), "Audit record not found: " + actionId);
            } else if (response.statusCode() == 422) {
                throw new HttpException(response.statusCode(), "No stored payload available for replay");
            } else {
                throw new HttpException(response.statusCode(), "Failed to replay action");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    /**
     * Bulk replays actions from the audit trail matching the given query parameters.
     */
    public ReplaySummary replayAudit(AuditQuery query) throws ActeonException {
        try {
            StringBuilder path = new StringBuilder("/v1/audit/replay");
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
            }

            if (!params.isEmpty()) {
                path.append("?").append(String.join("&", params));
            }

            HttpRequest request = requestBuilder(path.toString())
                .POST(HttpRequest.BodyPublishers.noBody())
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return parseResponse(response, ReplaySummary.class);
            } else {
                throw new HttpException(response.statusCode(), "Failed to replay audit");
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
    public ApprovalActionResponse approve(String namespace, String tenant, String id, String sig, long expiresAt) throws ActeonException {
        return approve(namespace, tenant, id, sig, expiresAt, null);
    }

    /**
     * Approves a pending action by namespace, tenant, ID, HMAC signature, and optional key ID.
     * Does not require authentication -- the HMAC signature serves as proof of authorization.
     *
     * @param kid Optional key ID identifying which HMAC key was used. Pass null to omit.
     */
    public ApprovalActionResponse approve(String namespace, String tenant, String id, String sig, long expiresAt, String kid) throws ActeonException {
        try {
            String path = "/v1/approvals/"
                + URLEncoder.encode(namespace, StandardCharsets.UTF_8) + "/"
                + URLEncoder.encode(tenant, StandardCharsets.UTF_8) + "/"
                + URLEncoder.encode(id, StandardCharsets.UTF_8) + "/approve"
                + "?sig=" + URLEncoder.encode(sig, StandardCharsets.UTF_8)
                + "&expires_at=" + expiresAt;
            if (kid != null) {
                path += "&kid=" + URLEncoder.encode(kid, StandardCharsets.UTF_8);
            }

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
    public ApprovalActionResponse reject(String namespace, String tenant, String id, String sig, long expiresAt) throws ActeonException {
        return reject(namespace, tenant, id, sig, expiresAt, null);
    }

    /**
     * Rejects a pending action by namespace, tenant, ID, HMAC signature, and optional key ID.
     * Does not require authentication -- the HMAC signature serves as proof of authorization.
     *
     * @param kid Optional key ID identifying which HMAC key was used. Pass null to omit.
     */
    public ApprovalActionResponse reject(String namespace, String tenant, String id, String sig, long expiresAt, String kid) throws ActeonException {
        try {
            String path = "/v1/approvals/"
                + URLEncoder.encode(namespace, StandardCharsets.UTF_8) + "/"
                + URLEncoder.encode(tenant, StandardCharsets.UTF_8) + "/"
                + URLEncoder.encode(id, StandardCharsets.UTF_8) + "/reject"
                + "?sig=" + URLEncoder.encode(sig, StandardCharsets.UTF_8)
                + "&expires_at=" + expiresAt;
            if (kid != null) {
                path += "&kid=" + URLEncoder.encode(kid, StandardCharsets.UTF_8);
            }

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
    public Optional<ApprovalStatus> getApproval(String namespace, String tenant, String id, String sig, long expiresAt) throws ActeonException {
        return getApproval(namespace, tenant, id, sig, expiresAt, null);
    }

    /**
     * Gets the status of an approval by namespace, tenant, ID, HMAC signature, and optional key ID.
     * Returns empty if not found or expired.
     *
     * @param kid Optional key ID identifying which HMAC key was used. Pass null to omit.
     */
    public Optional<ApprovalStatus> getApproval(String namespace, String tenant, String id, String sig, long expiresAt, String kid) throws ActeonException {
        try {
            String path = "/v1/approvals/"
                + URLEncoder.encode(namespace, StandardCharsets.UTF_8) + "/"
                + URLEncoder.encode(tenant, StandardCharsets.UTF_8) + "/"
                + URLEncoder.encode(id, StandardCharsets.UTF_8)
                + "?sig=" + URLEncoder.encode(sig, StandardCharsets.UTF_8)
                + "&expires_at=" + expiresAt;
            if (kid != null) {
                path += "&kid=" + URLEncoder.encode(kid, StandardCharsets.UTF_8);
            }

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

    // =========================================================================
    // Recurring Actions
    // =========================================================================

    /**
     * Creates a recurring action.
     */
    public CreateRecurringResponse createRecurring(CreateRecurringAction recurring) throws ActeonException {
        try {
            String body = objectMapper.writeValueAsString(recurring);
            HttpRequest request = requestBuilder("/v1/recurring")
                .POST(HttpRequest.BodyPublishers.ofString(body))
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 201) {
                return parseResponse(response, CreateRecurringResponse.class);
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
     * Lists recurring actions with optional filters.
     */
    public ListRecurringResponse listRecurring(RecurringFilter filter) throws ActeonException {
        try {
            StringBuilder path = new StringBuilder("/v1/recurring");
            List<String> params = new ArrayList<>();

            if (filter != null) {
                if (filter.getNamespace() != null) {
                    params.add("namespace=" + URLEncoder.encode(filter.getNamespace(), StandardCharsets.UTF_8));
                }
                if (filter.getTenant() != null) {
                    params.add("tenant=" + URLEncoder.encode(filter.getTenant(), StandardCharsets.UTF_8));
                }
                if (filter.getStatus() != null) {
                    params.add("status=" + URLEncoder.encode(filter.getStatus(), StandardCharsets.UTF_8));
                }
                if (filter.getLimit() != null) {
                    params.add("limit=" + filter.getLimit());
                }
                if (filter.getOffset() != null) {
                    params.add("offset=" + filter.getOffset());
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
                return parseResponse(response, ListRecurringResponse.class);
            } else {
                throw new HttpException(response.statusCode(), "Failed to list recurring actions");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    /**
     * Gets details of a specific recurring action.
     */
    public Optional<RecurringDetail> getRecurring(String recurringId, String namespace, String tenant) throws ActeonException {
        try {
            String path = "/v1/recurring/" + URLEncoder.encode(recurringId, StandardCharsets.UTF_8)
                + "?namespace=" + URLEncoder.encode(namespace, StandardCharsets.UTF_8)
                + "&tenant=" + URLEncoder.encode(tenant, StandardCharsets.UTF_8);

            HttpRequest request = requestBuilder(path)
                .GET()
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return Optional.of(parseResponse(response, RecurringDetail.class));
            } else if (response.statusCode() == 404) {
                return Optional.empty();
            } else {
                throw new HttpException(response.statusCode(), "Failed to get recurring action");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    /**
     * Updates a recurring action.
     */
    public RecurringDetail updateRecurring(String recurringId, UpdateRecurringAction update) throws ActeonException {
        try {
            String body = objectMapper.writeValueAsString(update);
            HttpRequest request = requestBuilder("/v1/recurring/" + URLEncoder.encode(recurringId, StandardCharsets.UTF_8))
                .PUT(HttpRequest.BodyPublishers.ofString(body))
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return parseResponse(response, RecurringDetail.class);
            } else if (response.statusCode() == 404) {
                throw new HttpException(response.statusCode(), "Recurring action not found: " + recurringId);
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
     * Deletes a recurring action.
     */
    public void deleteRecurring(String recurringId, String namespace, String tenant) throws ActeonException {
        try {
            String path = "/v1/recurring/" + URLEncoder.encode(recurringId, StandardCharsets.UTF_8)
                + "?namespace=" + URLEncoder.encode(namespace, StandardCharsets.UTF_8)
                + "&tenant=" + URLEncoder.encode(tenant, StandardCharsets.UTF_8);

            HttpRequest request = requestBuilder(path)
                .DELETE()
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 204) {
                return;
            } else if (response.statusCode() == 404) {
                throw new HttpException(response.statusCode(), "Recurring action not found: " + recurringId);
            } else {
                throw new HttpException(response.statusCode(), "Failed to delete recurring action");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    /**
     * Pauses a recurring action.
     */
    public RecurringDetail pauseRecurring(String recurringId, String namespace, String tenant) throws ActeonException {
        try {
            String body = objectMapper.writeValueAsString(Map.of("namespace", namespace, "tenant", tenant));
            HttpRequest request = requestBuilder("/v1/recurring/" + URLEncoder.encode(recurringId, StandardCharsets.UTF_8) + "/pause")
                .POST(HttpRequest.BodyPublishers.ofString(body))
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return parseResponse(response, RecurringDetail.class);
            } else if (response.statusCode() == 404) {
                throw new HttpException(response.statusCode(), "Recurring action not found: " + recurringId);
            } else if (response.statusCode() == 409) {
                throw new HttpException(response.statusCode(), "Recurring action is already paused");
            } else {
                throw new HttpException(response.statusCode(), "Failed to pause recurring action");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    /**
     * Resumes a paused recurring action.
     */
    public RecurringDetail resumeRecurring(String recurringId, String namespace, String tenant) throws ActeonException {
        try {
            String body = objectMapper.writeValueAsString(Map.of("namespace", namespace, "tenant", tenant));
            HttpRequest request = requestBuilder("/v1/recurring/" + URLEncoder.encode(recurringId, StandardCharsets.UTF_8) + "/resume")
                .POST(HttpRequest.BodyPublishers.ofString(body))
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return parseResponse(response, RecurringDetail.class);
            } else if (response.statusCode() == 404) {
                throw new HttpException(response.statusCode(), "Recurring action not found: " + recurringId);
            } else if (response.statusCode() == 409) {
                throw new HttpException(response.statusCode(), "Recurring action is already active");
            } else {
                throw new HttpException(response.statusCode(), "Failed to resume recurring action");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    // =========================================================================
    // Quotas
    // =========================================================================

    /**
     * Creates a quota policy.
     */
    public QuotaPolicy createQuota(CreateQuotaRequest req) throws ActeonException {
        try {
            String body = objectMapper.writeValueAsString(req);
            HttpRequest request = requestBuilder("/v1/quotas")
                .POST(HttpRequest.BodyPublishers.ofString(body))
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 201) {
                return parseResponse(response, QuotaPolicy.class);
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
     * Lists quota policies with optional namespace and tenant filters.
     */
    public ListQuotasResponse listQuotas(String namespace, String tenant) throws ActeonException {
        try {
            List<String> params = new ArrayList<>();
            if (namespace != null) {
                params.add("namespace=" + URLEncoder.encode(namespace, StandardCharsets.UTF_8));
            }
            if (tenant != null) {
                params.add("tenant=" + URLEncoder.encode(tenant, StandardCharsets.UTF_8));
            }

            String path = "/v1/quotas";
            if (!params.isEmpty()) {
                path += "?" + String.join("&", params);
            }

            HttpRequest request = requestBuilder(path)
                .GET()
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return parseResponse(response, ListQuotasResponse.class);
            } else {
                throw new HttpException(response.statusCode(), "Failed to list quotas");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    /**
     * Gets a single quota policy by ID.
     */
    public Optional<QuotaPolicy> getQuota(String quotaId) throws ActeonException {
        try {
            String path = "/v1/quotas/" + URLEncoder.encode(quotaId, StandardCharsets.UTF_8);

            HttpRequest request = requestBuilder(path)
                .GET()
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return Optional.of(parseResponse(response, QuotaPolicy.class));
            } else if (response.statusCode() == 404) {
                return Optional.empty();
            } else {
                throw new HttpException(response.statusCode(), "Failed to get quota");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    /**
     * Updates a quota policy.
     */
    public QuotaPolicy updateQuota(String quotaId, UpdateQuotaRequest update) throws ActeonException {
        try {
            String body = objectMapper.writeValueAsString(update);
            HttpRequest request = requestBuilder("/v1/quotas/" + URLEncoder.encode(quotaId, StandardCharsets.UTF_8))
                .PUT(HttpRequest.BodyPublishers.ofString(body))
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return parseResponse(response, QuotaPolicy.class);
            } else if (response.statusCode() == 404) {
                throw new HttpException(response.statusCode(), "Quota not found: " + quotaId);
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
     * Deletes a quota policy.
     */
    public void deleteQuota(String quotaId, String namespace, String tenant) throws ActeonException {
        try {
            String path = "/v1/quotas/" + URLEncoder.encode(quotaId, StandardCharsets.UTF_8)
                + "?namespace=" + URLEncoder.encode(namespace, StandardCharsets.UTF_8)
                + "&tenant=" + URLEncoder.encode(tenant, StandardCharsets.UTF_8);

            HttpRequest request = requestBuilder(path)
                .DELETE()
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 204) {
                return;
            } else if (response.statusCode() == 404) {
                throw new HttpException(response.statusCode(), "Quota not found: " + quotaId);
            } else {
                throw new HttpException(response.statusCode(), "Failed to delete quota");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    /**
     * Gets current usage statistics for a quota policy.
     */
    public QuotaUsage getQuotaUsage(String quotaId) throws ActeonException {
        try {
            String path = "/v1/quotas/" + URLEncoder.encode(quotaId, StandardCharsets.UTF_8) + "/usage";

            HttpRequest request = requestBuilder(path)
                .GET()
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return parseResponse(response, QuotaUsage.class);
            } else if (response.statusCode() == 404) {
                throw new HttpException(response.statusCode(), "Quota not found: " + quotaId);
            } else {
                throw new HttpException(response.statusCode(), "Failed to get quota usage");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    // =========================================================================
    // Chains (Task Chain Orchestration)
    // =========================================================================

    /**
     * Lists chain executions filtered by namespace, tenant, and optional status.
     *
     * @param namespace namespace to filter by
     * @param tenant    tenant to filter by
     * @param status    optional status filter ({@code "running"}, {@code "completed"},
     *                  {@code "failed"}, {@code "cancelled"}, {@code "timed_out"})
     */
    public ListChainsResponse listChains(String namespace, String tenant, String status) throws ActeonException {
        try {
            List<String> params = new ArrayList<>();
            params.add("namespace=" + URLEncoder.encode(namespace, StandardCharsets.UTF_8));
            params.add("tenant=" + URLEncoder.encode(tenant, StandardCharsets.UTF_8));
            if (status != null) {
                params.add("status=" + URLEncoder.encode(status, StandardCharsets.UTF_8));
            }

            String path = "/v1/chains?" + String.join("&", params);
            HttpRequest request = requestBuilder(path)
                .GET()
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return parseResponse(response, ListChainsResponse.class);
            } else {
                throw new HttpException(response.statusCode(), "Failed to list chains");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    /**
     * Gets full details of a chain execution including step results.
     *
     * @param chainId   chain execution ID
     * @param namespace namespace the chain belongs to
     * @param tenant    tenant the chain belongs to
     */
    public Optional<ChainDetailResponse> getChain(String chainId, String namespace, String tenant) throws ActeonException {
        try {
            String path = "/v1/chains/" + URLEncoder.encode(chainId, StandardCharsets.UTF_8)
                + "?namespace=" + URLEncoder.encode(namespace, StandardCharsets.UTF_8)
                + "&tenant=" + URLEncoder.encode(tenant, StandardCharsets.UTF_8);

            HttpRequest request = requestBuilder(path)
                .GET()
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return Optional.of(parseResponse(response, ChainDetailResponse.class));
            } else if (response.statusCode() == 404) {
                return Optional.empty();
            } else {
                throw new HttpException(response.statusCode(), "Failed to get chain");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    /**
     * Cancels a running chain execution.
     *
     * @param chainId chain execution ID
     * @param request cancel request containing namespace, tenant, and optional reason
     */
    public ChainDetailResponse cancelChain(String chainId, CancelChainRequest request) throws ActeonException {
        try {
            String body = objectMapper.writeValueAsString(request);
            HttpRequest httpReq = requestBuilder("/v1/chains/" + URLEncoder.encode(chainId, StandardCharsets.UTF_8) + "/cancel")
                .POST(HttpRequest.BodyPublishers.ofString(body))
                .build();

            HttpResponse<String> response = httpClient.send(httpReq, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return parseResponse(response, ChainDetailResponse.class);
            } else if (response.statusCode() == 404) {
                throw new HttpException(response.statusCode(), "Chain not found: " + chainId);
            } else if (response.statusCode() == 409) {
                throw new HttpException(response.statusCode(), "Chain is not running");
            } else {
                throw new HttpException(response.statusCode(), "Failed to cancel chain");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    // =========================================================================
    // Dead-Letter Queue (DLQ)
    // =========================================================================

    /**
     * Gets dead-letter queue statistics.
     */
    public DlqStatsResponse dlqStats() throws ActeonException {
        try {
            HttpRequest request = requestBuilder("/v1/dlq/stats")
                .GET()
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return parseResponse(response, DlqStatsResponse.class);
            } else {
                throw new HttpException(response.statusCode(), "Failed to get DLQ stats");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    /**
     * Drains all entries from the dead-letter queue.
     * Returns the drained entries for manual processing or resubmission.
     */
    public DlqDrainResponse dlqDrain() throws ActeonException {
        try {
            HttpRequest request = requestBuilder("/v1/dlq/drain")
                .POST(HttpRequest.BodyPublishers.noBody())
                .build();

            HttpResponse<String> response = httpClient.send(request, HttpResponse.BodyHandlers.ofString());

            if (response.statusCode() == 200) {
                return parseResponse(response, DlqDrainResponse.class);
            } else if (response.statusCode() == 404) {
                throw new HttpException(response.statusCode(), "Dead-letter queue is not enabled");
            } else {
                throw new HttpException(response.statusCode(), "Failed to drain DLQ");
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    // =========================================================================
    // Subscribe (SSE -- Entity-specific)
    // =========================================================================

    /**
     * Subscribes to real-time events for a specific entity via Server-Sent Events.
     *
     * <p>Returns an {@link SseEventIterator} that lazily reads SSE events from the
     * server. The iterator should be closed when no longer needed.</p>
     *
     * <p>Example usage:</p>
     * <pre>{@code
     * SubscribeOptions opts = new SubscribeOptions("ns", "tenant-1");
     * try (SseEventIterator events = client.subscribe("chain", "chain-42", opts)) {
     *     while (events.hasNext()) {
     *         SseEvent event = events.next();
     *         System.out.println(event.getEvent() + ": " + event.getData());
     *     }
     * }
     * }</pre>
     *
     * @param entityType entity type ({@code "chain"}, {@code "group"}, or {@code "action"})
     * @param entityId   entity identifier
     * @param options    subscribe options (namespace, tenant, include_history)
     */
    public SseEventIterator subscribe(String entityType, String entityId, SubscribeOptions options) throws ActeonException {
        try {
            List<String> params = new ArrayList<>();
            if (options != null) {
                if (options.getNamespace() != null) {
                    params.add("namespace=" + URLEncoder.encode(options.getNamespace(), StandardCharsets.UTF_8));
                }
                if (options.getTenant() != null) {
                    params.add("tenant=" + URLEncoder.encode(options.getTenant(), StandardCharsets.UTF_8));
                }
                if (options.getIncludeHistory() != null) {
                    params.add("include_history=" + options.getIncludeHistory());
                }
            }

            String path = "/v1/subscribe/"
                + URLEncoder.encode(entityType, StandardCharsets.UTF_8) + "/"
                + URLEncoder.encode(entityId, StandardCharsets.UTF_8);
            if (!params.isEmpty()) {
                path += "?" + String.join("&", params);
            }

            HttpRequest.Builder builder = HttpRequest.newBuilder()
                .uri(URI.create(baseUrl + path))
                .header("Accept", "text/event-stream");

            if (apiKey != null && !apiKey.isEmpty()) {
                builder.header("Authorization", "Bearer " + apiKey);
            }

            HttpRequest request = builder.GET().build();

            HttpResponse<InputStream> response = httpClient.send(request, HttpResponse.BodyHandlers.ofInputStream());

            if (response.statusCode() == 200) {
                return new SseEventIterator(response.body());
            } else {
                // Read the error body and close the stream.
                try (InputStream body = response.body()) {
                    String errorBody = new String(body.readAllBytes(), StandardCharsets.UTF_8);
                    throw new HttpException(response.statusCode(), "Subscribe failed: " + errorBody);
                }
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }

    // =========================================================================
    // Stream (SSE -- General)
    // =========================================================================

    /**
     * Opens a general-purpose SSE stream for real-time action outcomes.
     *
     * <p>Returns an {@link SseEventIterator} that lazily reads SSE events from the
     * server. The iterator should be closed when no longer needed.</p>
     *
     * <p>Example usage:</p>
     * <pre>{@code
     * StreamOptions opts = new StreamOptions();
     * opts.setNamespace("alerts");
     * opts.setOutcome("failed");
     * try (SseEventIterator events = client.stream(opts)) {
     *     while (events.hasNext()) {
     *         SseEvent event = events.next();
     *         System.out.println(event.getEvent() + ": " + event.getData());
     *     }
     * }
     * }</pre>
     *
     * @param options stream filter options (namespace, action_type, outcome, event_type, etc.)
     */
    public SseEventIterator stream(StreamOptions options) throws ActeonException {
        try {
            List<String> params = new ArrayList<>();
            String lastEventId = null;

            if (options != null) {
                if (options.getNamespace() != null) {
                    params.add("namespace=" + URLEncoder.encode(options.getNamespace(), StandardCharsets.UTF_8));
                }
                if (options.getActionType() != null) {
                    params.add("action_type=" + URLEncoder.encode(options.getActionType(), StandardCharsets.UTF_8));
                }
                if (options.getOutcome() != null) {
                    params.add("outcome=" + URLEncoder.encode(options.getOutcome(), StandardCharsets.UTF_8));
                }
                if (options.getEventType() != null) {
                    params.add("event_type=" + URLEncoder.encode(options.getEventType(), StandardCharsets.UTF_8));
                }
                if (options.getChainId() != null) {
                    params.add("chain_id=" + URLEncoder.encode(options.getChainId(), StandardCharsets.UTF_8));
                }
                if (options.getGroupId() != null) {
                    params.add("group_id=" + URLEncoder.encode(options.getGroupId(), StandardCharsets.UTF_8));
                }
                if (options.getActionId() != null) {
                    params.add("action_id=" + URLEncoder.encode(options.getActionId(), StandardCharsets.UTF_8));
                }
                lastEventId = options.getLastEventId();
            }

            String path = "/v1/stream";
            if (!params.isEmpty()) {
                path += "?" + String.join("&", params);
            }

            HttpRequest.Builder builder = HttpRequest.newBuilder()
                .uri(URI.create(baseUrl + path))
                .header("Accept", "text/event-stream");

            if (apiKey != null && !apiKey.isEmpty()) {
                builder.header("Authorization", "Bearer " + apiKey);
            }

            if (lastEventId != null) {
                builder.header("Last-Event-ID", lastEventId);
            }

            HttpRequest request = builder.GET().build();

            HttpResponse<InputStream> response = httpClient.send(request, HttpResponse.BodyHandlers.ofInputStream());

            if (response.statusCode() == 200) {
                return new SseEventIterator(response.body());
            } else {
                // Read the error body and close the stream.
                try (InputStream body = response.body()) {
                    String errorBody = new String(body.readAllBytes(), StandardCharsets.UTF_8);
                    throw new HttpException(response.statusCode(), "Stream failed: " + errorBody);
                }
            }
        } catch (IOException e) {
            throw new ConnectionException(e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new ConnectionException("Request interrupted", e);
        }
    }
}
