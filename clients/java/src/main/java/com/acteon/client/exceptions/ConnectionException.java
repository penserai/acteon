package com.acteon.client.exceptions;

/**
 * Raised when unable to connect to the server.
 */
public class ConnectionException extends ActeonException {
    public ConnectionException(String message) {
        super("Connection error: " + message);
    }

    public ConnectionException(String message, Throwable cause) {
        super("Connection error: " + message, cause);
    }

    @Override
    public boolean isRetryable() {
        return true;
    }
}
