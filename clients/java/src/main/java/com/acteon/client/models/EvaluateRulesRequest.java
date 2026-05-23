package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.Map;

/**
 * Request body for rule evaluation (Rule Playground).
 */
@JsonInclude(JsonInclude.Include.NON_NULL)
public class EvaluateRulesRequest {
    @JsonProperty("namespace")
    private String namespace;

    @JsonProperty("tenant")
    private String tenant;

    @JsonProperty("provider")
    private String provider;

    @JsonProperty("action_type")
    private String actionType;

    @JsonProperty("payload")
    private Map<String, Object> payload;

    @JsonProperty("metadata")
    private Map<String, String> metadata;

    @JsonProperty("include_disabled")
    private Boolean includeDisabled;

    @JsonProperty("evaluate_all")
    private Boolean evaluateAll;

    @JsonProperty("evaluate_at")
    private String evaluateAt;

    @JsonProperty("mock_state")
    private Map<String, String> mockState;

    public EvaluateRulesRequest() {}

    public EvaluateRulesRequest(String namespace, String tenant, String provider, String actionType, Map<String, Object> payload) {
        this.namespace = namespace;
        this.tenant = tenant;
        this.provider = provider;
        this.actionType = actionType;
        this.payload = payload;
    }

    public String getNamespace() { return namespace; }
    public void setNamespace(String namespace) { this.namespace = namespace; }

    public String getTenant() { return tenant; }
    public void setTenant(String tenant) { this.tenant = tenant; }

    public String getProvider() { return provider; }
    public void setProvider(String provider) { this.provider = provider; }

    public String getActionType() { return actionType; }
    public void setActionType(String actionType) { this.actionType = actionType; }

    public Map<String, Object> getPayload() { return payload; }
    public void setPayload(Map<String, Object> payload) { this.payload = payload; }

    public Map<String, String> getMetadata() { return metadata; }
    public void setMetadata(Map<String, String> metadata) { this.metadata = metadata; }

    public Boolean getIncludeDisabled() { return includeDisabled; }
    public void setIncludeDisabled(Boolean includeDisabled) { this.includeDisabled = includeDisabled; }

    public Boolean getEvaluateAll() { return evaluateAll; }
    public void setEvaluateAll(Boolean evaluateAll) { this.evaluateAll = evaluateAll; }

    public String getEvaluateAt() { return evaluateAt; }
    public void setEvaluateAt(String evaluateAt) { this.evaluateAt = evaluateAt; }

    public Map<String, String> getMockState() { return mockState; }
    public void setMockState(Map<String, String> mockState) { this.mockState = mockState; }
}
