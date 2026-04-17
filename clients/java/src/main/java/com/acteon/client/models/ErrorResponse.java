package com.acteon.client.models;

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
}
