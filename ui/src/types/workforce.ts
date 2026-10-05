/** Descriptive workforce models. Current server authority remains mandatory. */
import type { PrincipalIdentity } from "./index";
import type { GovernanceEffect, GovernanceLimits, GovernancePermitDeclaration, GovernanceRoute, GovernanceRouteView } from "./governance";
export interface TeamRef { domain: string; tenant: string; id: string; }
export type RepresentedParty = { kind: "human"; principal: PrincipalIdentity } | { kind: "team"; team: TeamRef };
export interface WorkforceReference { id: string; accepted_revision: number; }
export type WorkforceDependency = { kind: "membership" | "assignment"; reference: WorkforceReference };
export type TeamRole = "requester" | "approver" | "workforce_manager" | "mandate_issuer";
export interface WorkforceTeam { team: TeamRef; revision: number; name: string; }
export interface WorkforceMembership { id: string; revision: number; team: TeamRef; human: PrincipalIdentity; roles: TeamRole[]; valid_from_ms: number; deadline_ms: number; }
export interface AgentOwnership { agent: PrincipalIdentity; revision: number; owner: RepresentedParty; }
export interface WorkforceAssignment { id: string; revision: number; team: TeamRef; agent: PrincipalIdentity; job_classes: string[]; valid_from_ms: number; deadline_ms: number; }
export interface WorkforceMandateDeclaration {
  id: string; revision: number; represented: RepresentedParty; actor: PrincipalIdentity; job_class: string;
  eligible_initiators: PrincipalIdentity[]; ownership: WorkforceReference | null; dependencies: WorkforceDependency[];
  routes: GovernanceRoute[]; valid_from_ms: number; limits: GovernanceLimits;
}
export type WorkforceChange =
  | { kind: "put_team"; team: WorkforceTeam }
  | { kind: "disband_team"; team: TeamRef; expected_revision: number }
  | { kind: "put_membership"; membership: WorkforceMembership }
  | { kind: "remove_membership"; id: string; expected_revision: number }
  | { kind: "put_ownership"; ownership: AgentOwnership }
  | { kind: "put_assignment"; assignment: WorkforceAssignment }
  | { kind: "remove_assignment"; id: string; expected_revision: number }
  | { kind: "put_mandate"; mandate: WorkforceMandateDeclaration }
  | { kind: "revoke_mandate"; id: string; expected_revision: number }
  | { kind: "publish_represented_permit"; permit: GovernancePermitDeclaration; mandate: WorkforceReference };
export interface WorkforceChangeRequest { namespace: string; tenant: string; change_id: string; change: WorkforceChange; reason: string; }
export interface WorkforceEntry<T> { value: T; revoked: boolean; }
export interface WorkforceMandateView extends Omit<WorkforceMandateDeclaration, "routes"> { effects: GovernanceEffect[]; }
export interface WorkforcePermitBindingView { permit_id: string; permit_revision: number; mandate: WorkforceReference; }
export interface WorkforceManagementBounds {
  teams: TeamRef[]; principals: PrincipalIdentity[]; job_classes: string[];
  can_manage_roster: boolean; can_issue_mandates: boolean; can_issue_permits: boolean;
  valid_from_ms: number; limits: GovernanceLimits;
}
export interface WorkforceScopeView {
  namespace: string; tenant: string; incarnation: string; generation: number; management: WorkforceManagementBounds;
  routes: GovernanceRouteView[]; teams: WorkforceEntry<WorkforceTeam>[]; memberships: WorkforceEntry<WorkforceMembership>[];
  ownership: WorkforceEntry<AgentOwnership>[]; assignments: WorkforceEntry<WorkforceAssignment>[];
  mandates: WorkforceEntry<WorkforceMandateView>[]; permit_bindings: WorkforcePermitBindingView[];
}
