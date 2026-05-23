package com.acteon.client.models;

/**
 * Result from a batch dispatch operation. The wire shape is either
 * {@code {"error": {...}}} or a bare {@link ActionOutcome} variant —
 * see {@code BatchResultDeserializer} for the polymorphic decode.
 */
public class BatchResult {
    private boolean success;
    private ActionOutcome outcome;
    private ErrorResponse error;

    public boolean isSuccess() { return success; }
    public void setSuccess(boolean success) { this.success = success; }

    public ActionOutcome getOutcome() { return outcome; }
    public void setOutcome(ActionOutcome outcome) { this.outcome = outcome; }

    public ErrorResponse getError() { return error; }
    public void setError(ErrorResponse error) { this.error = error; }
}
