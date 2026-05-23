package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.Map;

/**
 * Response from rendering a template profile.
 */
public class RenderPreviewResponse {
    @JsonProperty("rendered")
    private Map<String, String> rendered;

    public Map<String, String> getRendered() { return rendered; }
}
