package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;
import java.util.List;

/** Wire models for independently authorized governance management. */
public final class Governance {
    private Governance() {}
    public record Resource(String kind, String namespace, String tenant, String id) {}
    public record Limits(@JsonProperty("max_units") long maxUnits,
        @JsonProperty("max_concurrent") long maxConcurrent, @JsonProperty("deadline_ms") long deadlineMs) {}
    public record Route(String provider, @JsonProperty("action_type") String actionType) {}
    public record Effect(String operation, List<Resource> resources) {}
    public record PermitDeclaration(String id, long revision, PrincipalIdentity subject,
        List<Route> routes, @JsonProperty("valid_from_ms") long validFromMs, Limits limits) {}
    public record PublishPermitRequest(String namespace, String tenant,
        @JsonProperty("change_id") String changeId, @JsonProperty("expected_revision") long expectedRevision,
        PermitDeclaration permit, String reason) {}
    public record FederationGrantCeilingDeclaration(PrincipalIdentity source,
        PrincipalIdentity target, @JsonProperty("agent_resource") Resource agentResource,
        @JsonProperty("binding_digest") String bindingDigest, String skill,
        @JsonProperty("ingress_effect") Effect ingressEffect, List<Effect> effects,
        Limits limits, @JsonProperty("max_depth") long maxDepth) {}
    public record FederationTrustDeclaration(String id, long revision,
        @JsonProperty("foreign_domain") String foreignDomain,
        @JsonProperty("local_audience") String localAudience,
        @JsonProperty("key_id") String keyId,
        @JsonProperty("verifying_key") String verifyingKey,
        @JsonProperty("minimum_issuer_epoch") long minimumIssuerEpoch,
        @JsonProperty("max_revocation_staleness_ms") long maxRevocationStalenessMs,
        @JsonProperty("max_clock_skew_ms") long maxClockSkewMs,
        @JsonProperty("valid_from_ms") long validFromMs,
        @JsonProperty("deadline_ms") long deadlineMs,
        List<FederationGrantCeilingDeclaration> approved) {}
    public record PublishFederationTrustRequest(String namespace, String tenant,
        @JsonProperty("change_id") String changeId,
        @JsonProperty("expected_revision") long expectedRevision,
        FederationTrustDeclaration trust, String reason) {}
    public record RevokeFederationTrustRequest(String namespace, String tenant,
        @JsonProperty("change_id") String changeId,
        @JsonProperty("trust_id") String trustId,
        @JsonProperty("expected_revision") long expectedRevision, String reason) {}
    public record SignedFederationEnvelopeDeclaration(String payload, String signature) {}
    public record SignedFederationCapabilityCard(String payload, String signature) {}
    public record SignedFederationInvocationDeclaration(String payload, String signature) {}
    public record FederationImportReceipt(String namespace, String tenant,
        @JsonProperty("import_id") String importId,
        @JsonProperty("envelope_digest") String envelopeDigest,
        @JsonProperty("trust_id") String trustId,
        @JsonProperty("trust_revision") long trustRevision,
        @JsonProperty("issuer_domain") String issuerDomain,
        PrincipalIdentity source, PrincipalIdentity target,
        @JsonProperty("agent_resource") Resource agentResource,
        @JsonProperty("binding_digest") String bindingDigest, String skill,
        @JsonProperty("root_id") String rootId,
        @JsonProperty("expires_at_ms") long expiresAtMs,
        @JsonProperty("accepted_at_ms") long acceptedAtMs,
        @JsonProperty("execution_started") boolean executionStarted) {}
    public record FederationExportReceipt(
        @JsonProperty("attempt_id") String attemptId,
        @JsonProperty("attempt_status") String attemptStatus,
        @JsonProperty("delivery_id") String deliveryId,
        com.fasterxml.jackson.databind.JsonNode delivery) {}
    public record FederationDeliveryEvidence(
        @JsonProperty("delivery_id") String deliveryId,
        @JsonProperty("parent_execution_id") String parentExecutionId,
        PrincipalIdentity source,
        @JsonProperty("agent_id") String agentId,
        @JsonProperty("message_id") String messageId,
        @JsonProperty("trust_id") String trustId,
        @JsonProperty("trust_revision") long trustRevision,
        @JsonProperty("key_id") String keyId,
        @JsonProperty("issuer_epoch") long issuerEpoch,
        @JsonProperty("created_at_ms") long createdAtMs,
        @JsonProperty("expires_at_ms") long expiresAtMs,
        boolean expired,
        @JsonProperty("exact_replay_idempotent") boolean exactReplayIdempotent,
        FederationCapabilityEvidence capability,
        com.fasterxml.jackson.databind.JsonNode status,
        List<FederationDeliveryFinalityEvidence> reconciliations) {}
    public record FederationCapabilityEvidence(
        @JsonProperty("card_digest") String cardDigest,
        @JsonProperty("key_id") String keyId,
        @JsonProperty("issuer_epoch") long issuerEpoch,
        @JsonProperty("expires_at_ms") long expiresAtMs,
        @JsonProperty("protocol_revision") String protocolRevision,
        @JsonProperty("authentication_scheme") String authenticationScheme) {}
    public record FederationDeliveryFinalityEvidence(
        String stage, boolean accepted,
        @JsonProperty("source_id") String sourceId,
        @JsonProperty("qualification_ref") String qualificationRef,
        @JsonProperty("verifier_revision") String verifierRevision,
        @JsonProperty("proof_digest") String proofDigest,
        PrincipalIdentity operator,
        @JsonProperty("authority_incarnation") String authorityIncarnation,
        @JsonProperty("authority_generation") long authorityGeneration,
        @JsonProperty("accepted_at_ms") long acceptedAtMs) {}
    public record FederationFinalityCorrelation(
        @JsonProperty("delivery_id") String deliveryId,
        String stage,
        @JsonProperty("parent_execution_id") String parentExecutionId,
        PrincipalIdentity source,
        @JsonProperty("adapter_revision") String adapterRevision,
        @JsonProperty("binding_digest") String bindingDigest,
        @JsonProperty("envelope_digest") String envelopeDigest,
        @JsonProperty("invocation_digest") String invocationDigest,
        @JsonProperty("import_id") String importId,
        @JsonProperty("invocation_nonce") String invocationNonce,
        @JsonProperty("agent_id") String agentId,
        @JsonProperty("message_id") String messageId) {}
    public record FederationDeliveryEvidencePage(
        List<FederationDeliveryEvidence> deliveries,
        @JsonProperty("next_cursor") String nextCursor) {}
    public record AbandonFederationDeliveryRequest(
        String namespace,
        String tenant,
        @JsonProperty("disposition_id") String dispositionId,
        String reason) {}
    public record ReconcileFederationDeliveryRequest(
        String namespace,
        String tenant,
        @JsonProperty("proof_base64") String proofBase64) {}
    public record FederationTrustView(FederationTrustDeclaration trust, boolean revoked) {}
    @JsonInclude(JsonInclude.Include.NON_NULL)
    public record Intervention(String kind, Resource resource, PrincipalIdentity subject,
        @JsonProperty("permit_id") String permitId, @JsonProperty("credential_id") String credentialId,
        @JsonProperty("expected_revision") Long expectedRevision) {
        public static Intervention close(Resource resource) { return new Intervention("close_resource", resource, null, null, null, null); }
        public static Intervention reopen(Resource resource) { return new Intervention("reopen_resource", resource, null, null, null, null); }
        public static Intervention revokeSubject(PrincipalIdentity subject) { return new Intervention("revoke_subject", null, subject, null, null, null); }
        public static Intervention revokePermit(String id, long revision) { return new Intervention("revoke_permit", null, null, id, null, revision); }
        public static Intervention revokeCredential(String id, long revision) { return new Intervention("revoke_credential", null, null, null, id, revision); }
    }
    public record InterventionRequest(String namespace, String tenant,
        @JsonProperty("change_id") String changeId, Intervention change, String reason) {}
    public record ChangeReceipt(String namespace, String tenant, @JsonProperty("change_id") String changeId,
        String actor, String reason, long generation, boolean pending) {}
    public record PermitView(String id, long revision, PrincipalIdentity subject, List<Effect> effects,
        @JsonProperty("valid_from_ms") long validFromMs, Limits limits, boolean revoked) {}
    public record RouteView(Route route, Effect effect, boolean closed) {}
    public record ManagementBounds(List<PrincipalIdentity> subjects,
        @JsonProperty("can_issue_permits") boolean canIssuePermits,
        @JsonProperty("can_intervene") boolean canIntervene,
        @JsonInclude(JsonInclude.Include.NON_DEFAULT) @JsonProperty("can_read_history") boolean canReadHistory,
        @JsonInclude(JsonInclude.Include.NON_DEFAULT) @JsonProperty("can_reconcile") boolean canReconcile,
        @JsonInclude(JsonInclude.Include.NON_DEFAULT) @JsonProperty("can_manage_federation") boolean canManageFederation,
        @JsonProperty("valid_from_ms") long validFromMs, Limits limits) {}
    public record ScopeView(ManagementBounds management, String namespace, String tenant, String incarnation, long generation,
        List<RouteView> routes, List<PermitView> permits,
        @JsonProperty("closed_resources") List<Resource> closedResources,
        @JsonProperty("revoked_subjects") List<String> revokedSubjects,
        @JsonInclude(JsonInclude.Include.NON_EMPTY) @JsonProperty("federation_trusts") List<FederationTrustView> federationTrusts) {}
    public record RegistryMutationRequest(String namespace, String tenant,
        @JsonProperty("agent_id") String agentId, @JsonProperty("change_id") String changeId,
        @JsonProperty("expected_registry_revision") long expectedRegistryRevision, String projection,
        @JsonProperty("expected_projection_version") Long expectedProjectionVersion,
        com.fasterxml.jackson.databind.JsonNode value, String reason) {}
    public record RegistryProjectionView(String namespace, String tenant, @JsonProperty("agent_id") String agentId,
        @JsonProperty("agent_resource") Resource agentResource, String projection,
        @JsonProperty("registry_revision") long registryRevision, @JsonProperty("qualification_retired") Boolean qualificationRetired,
        Long version, com.fasterxml.jackson.databind.JsonNode value) {}
    public record RegistryMutationReceipt(String namespace, String tenant,
        @JsonProperty("agent_id") String agentId, @JsonProperty("change_id") String changeId, String projection,
        @JsonProperty("expected_registry_revision") long expectedRegistryRevision,
        @JsonProperty("input_digest") String inputDigest, String actor,
        @JsonProperty("delivery_complete") boolean deliveryComplete, boolean applied) {}

}
