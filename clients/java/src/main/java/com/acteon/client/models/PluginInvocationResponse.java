package com.acteon.client.models;

import java.util.Map;

/**
 * Response from test-invoking a WASM plugin.
 */
public class PluginInvocationResponse {
    private boolean verdict;
    private String message;
    private Map<String, Object> metadata;
    private Double durationMs;

    public boolean isVerdict() { return verdict; }
    public void setVerdict(boolean verdict) { this.verdict = verdict; }

    public String getMessage() { return message; }
    public void setMessage(String message) { this.message = message; }

    public Map<String, Object> getMetadata() { return metadata; }
    public void setMetadata(Map<String, Object> metadata) { this.metadata = metadata; }

    public Double getDurationMs() { return durationMs; }
    public void setDurationMs(Double durationMs) { this.durationMs = durationMs; }
}
