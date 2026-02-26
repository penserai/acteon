package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;
import java.util.Map;

/**
 * Detailed status of a single chain step.
 *
 * <p>The {@code status} field is one of: "pending", "running", "completed",
 * "failed", "skipped", "waiting_sub_chain", "waiting_parallel". Parallel
 * sub-steps may also report "cancelled".
 */
public class ChainStepStatus {
    private String name;
    private String provider;
    private String status;

    @JsonProperty("response_body")
    private Object responseBody;

    private String error;

    @JsonProperty("completed_at")
    private String completedAt;

    @JsonProperty("sub_chain")
    private String subChain;

    @JsonProperty("child_chain_id")
    private String childChainId;

    @JsonProperty("parallel_sub_steps")
    private List<ChainStepStatus> parallelSubSteps;

    public String getName() { return name; }
    public void setName(String name) { this.name = name; }

    public String getProvider() { return provider; }
    public void setProvider(String provider) { this.provider = provider; }

    public String getStatus() { return status; }
    public void setStatus(String status) { this.status = status; }

    public Object getResponseBody() { return responseBody; }
    public void setResponseBody(Object responseBody) { this.responseBody = responseBody; }

    public String getError() { return error; }
    public void setError(String error) { this.error = error; }

    public String getCompletedAt() { return completedAt; }
    public void setCompletedAt(String completedAt) { this.completedAt = completedAt; }

    public String getSubChain() { return subChain; }
    public void setSubChain(String subChain) { this.subChain = subChain; }

    public String getChildChainId() { return childChainId; }
    public void setChildChainId(String childChainId) { this.childChainId = childChainId; }

    public List<ChainStepStatus> getParallelSubSteps() { return parallelSubSteps; }
    public void setParallelSubSteps(List<ChainStepStatus> parallelSubSteps) { this.parallelSubSteps = parallelSubSteps; }
}
