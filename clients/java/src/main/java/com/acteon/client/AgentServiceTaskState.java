package com.acteon.client;

/** Official A2A ProtoJSON TaskState values accepted by ListTasks. */
public enum AgentServiceTaskState {
    UNSPECIFIED("TASK_STATE_UNSPECIFIED"),
    SUBMITTED("TASK_STATE_SUBMITTED"),
    WORKING("TASK_STATE_WORKING"),
    COMPLETED("TASK_STATE_COMPLETED"),
    FAILED("TASK_STATE_FAILED"),
    CANCELED("TASK_STATE_CANCELED"),
    INPUT_REQUIRED("TASK_STATE_INPUT_REQUIRED"),
    AUTH_REQUIRED("TASK_STATE_AUTH_REQUIRED"),
    REJECTED("TASK_STATE_REJECTED");

    private final String wire;
    AgentServiceTaskState(String wire) { this.wire = wire; }
    public String wire() { return wire; }
}
