package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.Map;

/**
 * Request to render a template profile with payload data.
 */
public class RenderPreviewRequest {
    @JsonProperty("profile")
    private String profile;

    @JsonProperty("namespace")
    private String namespace;

    @JsonProperty("tenant")
    private String tenant;

    @JsonProperty("payload")
    private Map<String, Object> payload;

    public RenderPreviewRequest() {}

    public RenderPreviewRequest(String profile, String namespace, String tenant, Map<String, Object> payload) {
        this.profile = profile;
        this.namespace = namespace;
        this.tenant = tenant;
        this.payload = payload;
    }

    public String getProfile() { return profile; }
    public void setProfile(String profile) { this.profile = profile; }

    public String getNamespace() { return namespace; }
    public void setNamespace(String namespace) { this.namespace = namespace; }

    public String getTenant() { return tenant; }
    public void setTenant(String tenant) { this.tenant = tenant; }

    public Map<String, Object> getPayload() { return payload; }
    public void setPayload(Map<String, Object> payload) { this.payload = payload; }
}
