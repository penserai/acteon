package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * A per-rule trace entry from rule evaluation.
 */
public class RuleTraceEntry {
    @JsonProperty("rule_name")
    private String ruleName;

    @JsonProperty("priority")
    private int priority;

    @JsonProperty("enabled")
    private boolean enabled;

    @JsonProperty("condition_display")
    private String conditionDisplay;

    @JsonProperty("result")
    private String result;

    @JsonProperty("evaluation_duration_us")
    private long evaluationDurationUs;

    @JsonProperty("action")
    private String action;

    @JsonProperty("source")
    private String source;

    @JsonProperty("description")
    private String description;

    @JsonProperty("skip_reason")
    private String skipReason;

    @JsonProperty("error")
    private String error;

    public String getRuleName() { return ruleName; }
    public void setRuleName(String ruleName) { this.ruleName = ruleName; }

    public int getPriority() { return priority; }
    public void setPriority(int priority) { this.priority = priority; }

    public boolean isEnabled() { return enabled; }
    public void setEnabled(boolean enabled) { this.enabled = enabled; }

    public String getConditionDisplay() { return conditionDisplay; }
    public void setConditionDisplay(String conditionDisplay) { this.conditionDisplay = conditionDisplay; }

    public String getResult() { return result; }
    public void setResult(String result) { this.result = result; }

    public long getEvaluationDurationUs() { return evaluationDurationUs; }
    public void setEvaluationDurationUs(long evaluationDurationUs) { this.evaluationDurationUs = evaluationDurationUs; }

    public String getAction() { return action; }
    public void setAction(String action) { this.action = action; }

    public String getSource() { return source; }
    public void setSource(String source) { this.source = source; }

    public String getDescription() { return description; }
    public void setDescription(String description) { this.description = description; }

    public String getSkipReason() { return skipReason; }
    public void setSkipReason(String skipReason) { this.skipReason = skipReason; }

    public String getError() { return error; }
    public void setError(String error) { this.error = error; }
}
