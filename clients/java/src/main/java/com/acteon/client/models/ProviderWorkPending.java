package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

/** Retained work requiring observation; this identity is not an execution credential. */
public class ProviderWorkPending {
    @JsonProperty("execution_id")
    private String executionId;
    private int attempts;
    private State state;
    public String getExecutionId() { return executionId; }
    public void setExecutionId(String value) { executionId = value; }
    public int getAttempts() { return attempts; }
    public void setAttempts(int value) { attempts = value; }
    public State getState() { return state; }
    public void setState(State value) { state = value; }

    public static class State {
        private String kind;
        @JsonProperty("attempt_id")
        private String attemptId;
        @JsonProperty("not_before_ms")
        private Long notBeforeMs;
        public String getKind() { return kind; }
        public void setKind(String value) { kind = value; }
        public String getAttemptId() { return attemptId; }
        public void setAttemptId(String value) { attemptId = value; }
        public Long getNotBeforeMs() { return notBeforeMs; }
        public void setNotBeforeMs(Long value) { notBeforeMs = value; }
    }
}
