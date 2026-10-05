package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.fasterxml.jackson.annotation.JsonTypeInfo;
import com.fasterxml.jackson.annotation.JsonSubTypes;
import java.util.List;

/** Typed workforce organization and bounded management declarations. */
public final class Workforce {
    private Workforce() {}
    public record TeamRef(String domain, String tenant, String id) {}
    @JsonTypeInfo(use=JsonTypeInfo.Id.NAME, property="kind")
    @JsonSubTypes({@JsonSubTypes.Type(value=HumanRepresentation.class,name="human"), @JsonSubTypes.Type(value=TeamRepresentation.class,name="team")})
    public sealed interface RepresentedParty permits HumanRepresentation, TeamRepresentation {}
    public record HumanRepresentation(PrincipalIdentity principal) implements RepresentedParty {}
    public record TeamRepresentation(TeamRef team) implements RepresentedParty {}
    public record Reference(String id, @JsonProperty("accepted_revision") long acceptedRevision) {}
    public record Dependency(String kind, Reference reference) {}
    public record Team(TeamRef team, long revision, String name) {}
    public record Membership(String id, long revision, TeamRef team, PrincipalIdentity human, List<String> roles,
        @JsonProperty("valid_from_ms") long validFromMs, @JsonProperty("deadline_ms") long deadlineMs) {}
    public record Ownership(PrincipalIdentity agent, long revision, RepresentedParty owner) {}
    public record Assignment(String id, long revision, TeamRef team, PrincipalIdentity agent,
        @JsonProperty("job_classes") List<String> jobClasses, @JsonProperty("valid_from_ms") long validFromMs,
        @JsonProperty("deadline_ms") long deadlineMs) {}
    public record MandateDeclaration(String id, long revision, RepresentedParty represented, PrincipalIdentity actor,
        @JsonProperty("job_class") String jobClass, @JsonProperty("eligible_initiators") List<PrincipalIdentity> eligibleInitiators,
        Reference ownership, List<Dependency> dependencies, @JsonProperty("valid_from_ms") long validFromMs, Governance.Limits limits, List<Governance.Route> routes) {}
    public record MandateView(String id, long revision, RepresentedParty represented, PrincipalIdentity actor,
        @JsonProperty("job_class") String jobClass, @JsonProperty("eligible_initiators") List<PrincipalIdentity> eligibleInitiators,
        Reference ownership, List<Dependency> dependencies, @JsonProperty("valid_from_ms") long validFromMs, Governance.Limits limits, List<Governance.Effect> effects) {}
    @JsonTypeInfo(use=JsonTypeInfo.Id.NAME, property="kind")
    @JsonSubTypes({@JsonSubTypes.Type(value=PutTeam.class,name="put_team"), @JsonSubTypes.Type(value=DisbandTeam.class,name="disband_team"), @JsonSubTypes.Type(value=PutMembership.class,name="put_membership"), @JsonSubTypes.Type(value=RemoveMembership.class,name="remove_membership"), @JsonSubTypes.Type(value=PutOwnership.class,name="put_ownership"), @JsonSubTypes.Type(value=PutAssignment.class,name="put_assignment"), @JsonSubTypes.Type(value=RemoveAssignment.class,name="remove_assignment"), @JsonSubTypes.Type(value=PutMandate.class,name="put_mandate"), @JsonSubTypes.Type(value=RevokeMandate.class,name="revoke_mandate"), @JsonSubTypes.Type(value=PublishRepresentedPermit.class,name="publish_represented_permit")})
    public sealed interface Change permits PutTeam, DisbandTeam, PutMembership, RemoveMembership, PutOwnership, PutAssignment, RemoveAssignment, PutMandate, RevokeMandate, PublishRepresentedPermit {}
    public record PutTeam(Team team) implements Change {}
    public record DisbandTeam(TeamRef team, @JsonProperty("expected_revision") long expectedRevision) implements Change {}
    public record PutMembership(Membership membership) implements Change {}
    public record RemoveMembership(String id, @JsonProperty("expected_revision") long expectedRevision) implements Change {}
    public record PutOwnership(Ownership ownership) implements Change {}
    public record PutAssignment(Assignment assignment) implements Change {}
    public record RemoveAssignment(String id, @JsonProperty("expected_revision") long expectedRevision) implements Change {}
    public record PutMandate(MandateDeclaration mandate) implements Change {}
    public record RevokeMandate(String id, @JsonProperty("expected_revision") long expectedRevision) implements Change {}
    public record PublishRepresentedPermit(Governance.PermitDeclaration permit, Reference mandate) implements Change {}
    public record ChangeRequest(String namespace, String tenant, @JsonProperty("change_id") String changeId, Change change, String reason) {}
    public record Entry<T>(T value, boolean revoked) {}
    public record PermitBindingView(@JsonProperty("permit_id") String permitId, @JsonProperty("permit_revision") long permitRevision, Reference mandate) {}
    public record ManagementBounds(List<TeamRef> teams, List<PrincipalIdentity> principals, @JsonProperty("job_classes") List<String> jobClasses,
        @JsonProperty("can_manage_roster") boolean canManageRoster, @JsonProperty("can_issue_mandates") boolean canIssueMandates,
        @JsonProperty("can_issue_permits") boolean canIssuePermits, @JsonProperty("valid_from_ms") long validFromMs, Governance.Limits limits) {}
    public record ScopeView(String namespace, String tenant, String incarnation, long generation, ManagementBounds management,
        List<Governance.RouteView> routes, List<Entry<Team>> teams, List<Entry<Membership>> memberships,
        List<Entry<Ownership>> ownership, List<Entry<Assignment>> assignments, List<Entry<MandateView>> mandates,
        @JsonProperty("permit_bindings") List<PermitBindingView> permitBindings) {}
}
