package com.acteon.client.exceptions;

/**
 * Raised for API-level errors returned by the server.
 */
public class ApiException extends ActeonException {
    private final String code;
    private final boolean retryable;

    public ApiException(String code, String message, boolean retryable) {
        super("API error [" + code + "]: " + message);
        this.code = code;
        this.retryable = retryable;
    }

    public String getCode() {
        return code;
    }

    @Override
    public boolean isRetryable() {
        return retryable;
    }
}
