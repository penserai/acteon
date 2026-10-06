package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

/** Finality observations and opaque proofs; never execution authority. */
public final class ProviderReconciliation {
    private ProviderReconciliation() {}
    public record Context(@JsonProperty("context_id") String contextId,
        @JsonProperty("execution_id") String executionId, String namespace, String tenant,
        PrincipalIdentity principal, @JsonProperty("request_digest") String requestDigest) {}
    public record Correlation(Context context, @JsonProperty("action_id") String actionId,
        @JsonProperty("attempt_id") String attemptId, long ordinal, String token,
        @JsonProperty("binding_digest") String bindingDigest) {}
    public record Request(@JsonProperty("proof_base64") String proofBase64) {}
}
