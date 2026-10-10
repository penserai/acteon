package com.acteon.client;

import java.time.Instant;

/** Optional official A2A filters for exact-agent ListTasks. */
public record AgentServiceListOptions(
    AgentServiceReceipt source,
    String contextId,
    AgentServiceTaskState status,
    Integer pageSize,
    String pageToken,
    Integer historyLength,
    Instant statusTimestampAfter,
    boolean includeArtifacts
) {
    public static AgentServiceListOptions defaults() {
        return new AgentServiceListOptions(null, null, null, null, null, null, null, false);
    }
}
