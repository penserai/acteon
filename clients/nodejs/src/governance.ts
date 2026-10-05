/** Public wire types. Supplying these values does not grant management authority. */
import type { PrincipalIdentity } from "./models.js";
export interface GovernanceResource { kind: string; namespace: string; tenant: string; id: string; }
export interface GovernanceLimits { max_units: number; max_concurrent: number; deadline_ms: number; }
export interface GovernanceRoute { provider: string; action_type: string; }
export interface GovernanceEffect { operation: string; resources: GovernanceResource[]; }
export interface GovernancePermitDeclaration {
  id: string; revision: number; subject: PrincipalIdentity; routes: GovernanceRoute[];
  valid_from_ms: number; limits: GovernanceLimits;
}
export interface PublishGovernancePermitRequest {
  namespace: string; tenant: string; change_id: string; expected_revision: number;
  permit: GovernancePermitDeclaration; reason: string;
}
export type GovernanceIntervention =
  | { kind: "close_resource" | "reopen_resource"; resource: GovernanceResource }
  | { kind: "revoke_subject"; subject: PrincipalIdentity }
  | { kind: "revoke_permit"; permit_id: string; expected_revision: number }
  | { kind: "revoke_credential"; credential_id: string; expected_revision: number };
export interface GovernanceInterventionRequest {
  namespace: string; tenant: string; change_id: string; change: GovernanceIntervention; reason: string;
}
export interface GovernanceChangeReceipt {
  namespace: string; tenant: string; change_id: string; actor: string; reason: string; generation: number; pending: boolean;
}
export interface GovernancePermitView {
  id: string; revision: number; subject: PrincipalIdentity; effects: GovernanceEffect[];
  valid_from_ms: number; limits: GovernanceLimits; revoked: boolean;
}
export interface GovernanceRouteView { route: GovernanceRoute; effect: GovernanceEffect; closed: boolean; }
export interface GovernanceManagementBounds {
  subjects: PrincipalIdentity[]; can_issue_permits: boolean; can_intervene: boolean;
  valid_from_ms: number; limits: GovernanceLimits;
}
export interface GovernanceScopeView {
  management: GovernanceManagementBounds;
  namespace: string; tenant: string; incarnation: string; generation: number;
  routes: GovernanceRouteView[]; permits: GovernancePermitView[];
  closed_resources: GovernanceResource[]; revoked_subjects: string[];
}
