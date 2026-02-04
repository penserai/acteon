package com.acteon.client.exceptions;

/**
 * Base exception for Acteon client errors.
 */
public class ActeonException extends Exception {
    public ActeonException(String message) {
        super(message);
    }

    public ActeonException(String message, Throwable cause) {
        super(message, cause);
    }

    /**
     * Returns true if this error is retryable.
     */
    public boolean isRetryable() {
        return false;
    }
}
