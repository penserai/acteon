package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.time.Duration;
import java.util.Map;

/**
 * Outcome of dispatching an action.
 */
public class ActionOutcome {
    private OutcomeType type;
    private ProviderResponse response;
    private String rule;
    private String originalProvider;
    private String newProvider;
    private Duration retryAfter;
    private ActionError error;

    public enum OutcomeType {
        EXECUTED, DEDUPLICATED, SUPPRESSED, REROUTED, THROTTLED, FAILED
    }

    // Getters and setters
    public OutcomeType getType() { return type; }
    public void setType(OutcomeType type) { this.type = type; }

    public ProviderResponse getResponse() { return response; }
    public void setResponse(ProviderResponse response) { this.response = response; }

    public String getRule() { return rule; }
    public void setRule(String rule) { this.rule = rule; }

    public String getOriginalProvider() { return originalProvider; }
    public void setOriginalProvider(String originalProvider) { this.originalProvider = originalProvider; }

    public String getNewProvider() { return newProvider; }
    public void setNewProvider(String newProvider) { this.newProvider = newProvider; }

    public Duration getRetryAfter() { return retryAfter; }
    public void setRetryAfter(Duration retryAfter) { this.retryAfter = retryAfter; }

    public ActionError getError() { return error; }
    public void setError(ActionError error) { this.error = error; }

    public boolean isExecuted() { return type == OutcomeType.EXECUTED; }
    public boolean isDeduplicated() { return type == OutcomeType.DEDUPLICATED; }
    public boolean isSuppressed() { return type == OutcomeType.SUPPRESSED; }
    public boolean isRerouted() { return type == OutcomeType.REROUTED; }
    public boolean isThrottled() { return type == OutcomeType.THROTTLED; }
    public boolean isFailed() { return type == OutcomeType.FAILED; }

    /**
     * Parse an ActionOutcome from a raw JSON map.
     */
    @SuppressWarnings("unchecked")
    public static ActionOutcome fromMap(Map<String, Object> data) {
        ActionOutcome outcome = new ActionOutcome();

        if (data.containsKey("Executed")) {
            outcome.type = OutcomeType.EXECUTED;
            Map<String, Object> resp = (Map<String, Object>) data.get("Executed");
            outcome.response = ProviderResponse.fromMap(resp);
        } else if (data.containsKey("Deduplicated") || "Deduplicated".equals(data)) {
            outcome.type = OutcomeType.DEDUPLICATED;
        } else if (data.containsKey("Suppressed")) {
            outcome.type = OutcomeType.SUPPRESSED;
            Map<String, Object> suppressed = (Map<String, Object>) data.get("Suppressed");
            outcome.rule = (String) suppressed.get("rule");
        } else if (data.containsKey("Rerouted")) {
            outcome.type = OutcomeType.REROUTED;
            Map<String, Object> rerouted = (Map<String, Object>) data.get("Rerouted");
            outcome.originalProvider = (String) rerouted.get("original_provider");
            outcome.newProvider = (String) rerouted.get("new_provider");
            if (rerouted.containsKey("response")) {
                outcome.response = ProviderResponse.fromMap((Map<String, Object>) rerouted.get("response"));
            }
        } else if (data.containsKey("Throttled")) {
            outcome.type = OutcomeType.THROTTLED;
            Map<String, Object> throttled = (Map<String, Object>) data.get("Throttled");
            Map<String, Object> retryAfter = (Map<String, Object>) throttled.get("retry_after");
            long secs = ((Number) retryAfter.getOrDefault("secs", 0)).longValue();
            long nanos = ((Number) retryAfter.getOrDefault("nanos", 0)).longValue();
            outcome.retryAfter = Duration.ofSeconds(secs).plusNanos(nanos);
        } else if (data.containsKey("Failed")) {
            outcome.type = OutcomeType.FAILED;
            Map<String, Object> failed = (Map<String, Object>) data.get("Failed");
            outcome.error = ActionError.fromMap(failed);
        } else {
            outcome.type = OutcomeType.FAILED;
            outcome.error = new ActionError("UNKNOWN", "Unknown outcome", false, 0);
        }

        return outcome;
    }
}
