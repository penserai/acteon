package com.acteon.client.models;

import java.util.Map;

/**
 * Response from a provider.
 */
public class ProviderResponse {
    /**
     * Defaults to {@code "success"} when the server omits the field —
     * matches the pre-Jackson-migration {@code fromMap} behavior.
     */
    private String status = "success";
    private Map<String, Object> body;
    private Map<String, String> headers;

    public String getStatus() { return status; }
    public void setStatus(String status) { this.status = status; }

    public Map<String, Object> getBody() { return body; }
    public void setBody(Map<String, Object> body) { this.body = body; }

    public Map<String, String> getHeaders() { return headers; }
    public void setHeaders(Map<String, String> headers) { this.headers = headers; }
}
