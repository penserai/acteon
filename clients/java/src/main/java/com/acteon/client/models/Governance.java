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
        @JsonProperty("valid_from_ms") long validFromMs, Limits limits) {}
    public record ScopeView(ManagementBounds management, String namespace, String tenant, String incarnation, long generation,
        List<RouteView> routes, List<PermitView> permits,
        @JsonProperty("closed_resources") List<Resource> closedResources,
        @JsonProperty("revoked_subjects") List<String> revokedSubjects) {}
}
