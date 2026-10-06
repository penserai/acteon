package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;
import java.util.List;

/** Retained evidence; inspecting a receipt never admits or resumes execution. */
public record ProviderExecutionHistory(PrincipalIdentity subject, Receipt receipt,
    @JsonProperty("observed_authority") Authority observedAuthority,
    @JsonProperty("operation_integrity") String operationIntegrity,
    Metadata metadata, Binding binding,
    @JsonProperty("cancellation_fenced") boolean cancellationFenced, List<Attempt> attempts) {
    public record Receipt(@JsonProperty("execution_id") String executionId, long attempts, Status status) {}
    public record Status(String state, @JsonProperty("attempt_id") String attemptId,
        @JsonProperty("not_before_ms") Long notBeforeMs, ActionOutcome outcome) {}
    public record Authority(String incarnation, long generation) {}
    public record Metadata(@JsonProperty("original_action_id") String originalActionId,
        @JsonProperty("max_attempts") long maxAttempts) {}
    public record Binding(String provider, @JsonProperty("provider_revision") String providerRevision,
        @JsonProperty("failure_revision") String failureRevision, Governance.Effect effect) {}
    public record Evidence(String id, String digest) {}
    public record Attempt(@JsonProperty("attempt_id") String attemptId, long ordinal,
        @JsonProperty("ledger_status") String ledgerStatus,
        @JsonProperty("original_evidence") Evidence originalEvidence,
        @JsonProperty("original_outcome") ActionOutcome originalOutcome, Reconciliation reconciliation) {}
    public record Acceptance(PrincipalIdentity operator, Authority authority,
        @JsonProperty("accepted_at_ms") long acceptedAtMs) {}
    public record Reconciliation(@JsonProperty("prior_status") String priorStatus,
        @JsonProperty("execution_id") String executionId, @JsonProperty("attempt_id") String attemptId,
        @JsonProperty("original_evidence") Evidence originalEvidence, Evidence resolution,
        @JsonProperty("verifier_revision") String verifierRevision, @JsonProperty("proof_digest") String proofDigest,
        @JsonProperty("resolved_at_ms") long resolvedAtMs, ActionOutcome outcome, Acceptance acceptance) {}
}
