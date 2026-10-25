package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.Map;

/**
 * Request to render a template profile with payload data.
 */
public class RenderPreviewRequest {
    @JsonProperty("profile")
    private String profile;

    @JsonProperty("payload")
    private Map<String, Object> payload;

    public RenderPreviewRequest() {}

    public RenderPreviewRequest(String profile, Map<String, Object> payload) {
        this.profile = profile;

        this.payload = payload;
    }

    public String getProfile() { return profile; }
    public void setProfile(String profile) { this.profile = profile; }

    public Map<String, Object> getPayload() { return payload; }
    public void setPayload(Map<String, Object> payload) { this.payload = payload; }
}
