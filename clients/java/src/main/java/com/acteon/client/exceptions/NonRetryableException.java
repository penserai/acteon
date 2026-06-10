package com.acteon.client.exceptions;

/**
 * Thrown by a {@link com.acteon.client.Worker.TaskHandler} to mark a
 * task failure as terminal.
 *
 * <p>The {@link com.acteon.client.Worker} reports it with
 * {@code retryable=false} so the server fails the task immediately
 * instead of re-queueing it with backoff. Any other exception thrown
 * from a handler fails the task with {@code retryable=true}.
 */
public class NonRetryableException extends ActeonException {
    public NonRetryableException(String message) {
        super(message);
    }

    public NonRetryableException(String message, Throwable cause) {
        super(message, cause);
    }
}
