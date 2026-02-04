package com.acteon.client.models;

import java.util.Map;

/**
 * Result from a batch dispatch operation.
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

    @SuppressWarnings("unchecked")
    public static BatchResult fromMap(Map<String, Object> data) {
        BatchResult result = new BatchResult();

        if (data.containsKey("error")) {
            result.success = false;
            result.error = ErrorResponse.fromMap((Map<String, Object>) data.get("error"));
        } else {
            result.success = true;
            result.outcome = ActionOutcome.fromMap(data);
        }

        return result;
    }
}
