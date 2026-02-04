package com.acteon.client.models;

import java.util.Map;

/**
 * Error response from the API.
 */
public class ErrorResponse {
    private String code;
    private String message;
    private boolean retryable;

    public String getCode() { return code; }
    public void setCode(String code) { this.code = code; }

    public String getMessage() { return message; }
    public void setMessage(String message) { this.message = message; }

    public boolean isRetryable() { return retryable; }
    public void setRetryable(boolean retryable) { this.retryable = retryable; }

    public static ErrorResponse fromMap(Map<String, Object> data) {
        ErrorResponse response = new ErrorResponse();
        response.code = (String) data.getOrDefault("code", "UNKNOWN");
        response.message = (String) data.getOrDefault("message", "Unknown error");
        response.retryable = (Boolean) data.getOrDefault("retryable", false);
        return response;
    }
}
