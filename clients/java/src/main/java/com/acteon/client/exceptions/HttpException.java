package com.acteon.client.exceptions;

/**
 * Raised for HTTP errors.
 */
public class HttpException extends ActeonException {
    private final int status;

    public HttpException(int status, String message) {
        super("HTTP " + status + ": " + message);
        this.status = status;
    }

    public int getStatus() {
        return status;
    }

    @Override
    public boolean isRetryable() {
        return status >= 500;
    }
}
