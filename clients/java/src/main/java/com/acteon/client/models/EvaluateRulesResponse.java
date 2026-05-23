package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.Collections;
import java.util.List;
import java.util.Map;

/**
 * Response from rule evaluation (Rule Playground).
 */
public class EvaluateRulesResponse {
    @JsonProperty("verdict")
    private String verdict;

    @JsonProperty("matched_rule")
    private String matchedRule;

    @JsonProperty("has_errors")
    private boolean hasErrors;

    @JsonProperty("total_rules_evaluated")
    private int totalRulesEvaluated;

    @JsonProperty("total_rules_skipped")
    private int totalRulesSkipped;

    @JsonProperty("evaluation_duration_us")
    private long evaluationDurationUs;

    @JsonProperty("trace")
    private List<RuleTraceEntry> trace;

    @JsonProperty("context")
    private TraceContext context;

    @JsonProperty("modified_payload")
    private Map<String, Object> modifiedPayload;

    public String getVerdict() { return verdict; }
    public void setVerdict(String verdict) { this.verdict = verdict; }

    public String getMatchedRule() { return matchedRule; }
    public void setMatchedRule(String matchedRule) { this.matchedRule = matchedRule; }

    public boolean isHasErrors() { return hasErrors; }
    public void setHasErrors(boolean hasErrors) { this.hasErrors = hasErrors; }

    public int getTotalRulesEvaluated() { return totalRulesEvaluated; }
    public void setTotalRulesEvaluated(int totalRulesEvaluated) { this.totalRulesEvaluated = totalRulesEvaluated; }

    public int getTotalRulesSkipped() { return totalRulesSkipped; }
    public void setTotalRulesSkipped(int totalRulesSkipped) { this.totalRulesSkipped = totalRulesSkipped; }

    public long getEvaluationDurationUs() { return evaluationDurationUs; }
    public void setEvaluationDurationUs(long evaluationDurationUs) { this.evaluationDurationUs = evaluationDurationUs; }

    public List<RuleTraceEntry> getTrace() { return trace; }
    public void setTrace(List<RuleTraceEntry> trace) { this.trace = trace; }

    public TraceContext getContext() { return context; }
    public void setContext(TraceContext context) { this.context = context; }

    public Map<String, Object> getModifiedPayload() { return modifiedPayload; }
    public void setModifiedPayload(Map<String, Object> modifiedPayload) { this.modifiedPayload = modifiedPayload; }

    /**
     * Nested trace context with time, environment keys, and timezone.
     */
    public static class TraceContext {
        @JsonProperty("time")
        private Map<String, Object> time;

        @JsonProperty("environment_keys")
        private List<String> environmentKeys;

        @JsonProperty("accessed_state_keys")
        @JsonInclude(JsonInclude.Include.NON_EMPTY)
        private List<String> accessedStateKeys = Collections.emptyList();

        @JsonProperty("effective_timezone")
        private String effectiveTimezone;

        public Map<String, Object> getTime() { return time; }
        public void setTime(Map<String, Object> time) { this.time = time; }

        public List<String> getEnvironmentKeys() { return environmentKeys; }
        public void setEnvironmentKeys(List<String> environmentKeys) { this.environmentKeys = environmentKeys; }

        public List<String> getAccessedStateKeys() { return accessedStateKeys; }
        public void setAccessedStateKeys(List<String> accessedStateKeys) { this.accessedStateKeys = accessedStateKeys; }

        public String getEffectiveTimezone() { return effectiveTimezone; }
        public void setEffectiveTimezone(String effectiveTimezone) { this.effectiveTimezone = effectiveTimezone; }
    }
}
