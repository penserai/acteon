package com.acteon.client.models;

/**
 * Error details when an action fails.
 */
public class ActionError {
    private String code;
    private String message;
    private boolean retryable;
    private int attempts;

    public ActionError() {}

    public ActionError(String code, String message, boolean retryable, int attempts) {
        this.code = code;
        this.message = message;
        this.retryable = retryable;
        this.attempts = attempts;
    }

    public String getCode() { return code; }
    public void setCode(String code) { this.code = code; }

    public String getMessage() { return message; }
    public void setMessage(String message) { this.message = message; }

    public boolean isRetryable() { return retryable; }
    public void setRetryable(boolean retryable) { this.retryable = retryable; }

    public int getAttempts() { return attempts; }
    public void setAttempts(int attempts) { this.attempts = attempts; }
}
