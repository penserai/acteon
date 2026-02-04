package com.acteon.client.models;

import java.util.Map;

/**
 * Response from a provider.
 */
public class ProviderResponse {
    private String status;
    private Map<String, Object> body;
    private Map<String, String> headers;

    public String getStatus() { return status; }
    public void setStatus(String status) { this.status = status; }

    public Map<String, Object> getBody() { return body; }
    public void setBody(Map<String, Object> body) { this.body = body; }

    public Map<String, String> getHeaders() { return headers; }
    public void setHeaders(Map<String, String> headers) { this.headers = headers; }

    @SuppressWarnings("unchecked")
    public static ProviderResponse fromMap(Map<String, Object> data) {
        ProviderResponse response = new ProviderResponse();
        response.status = (String) data.getOrDefault("status", "success");
        response.body = (Map<String, Object>) data.getOrDefault("body", Map.of());
        response.headers = (Map<String, String>) data.getOrDefault("headers", Map.of());
        return response;
    }
}
