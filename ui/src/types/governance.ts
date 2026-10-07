/** Public wire types. Supplying these values does not grant management authority. */
import type { PrincipalIdentity } from "./index";
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
  can_read_history?: boolean;
  can_reconcile?: boolean;
  valid_from_ms: number; limits: GovernanceLimits;
}
export interface GovernanceScopeView {
  management: GovernanceManagementBounds;
  namespace: string; tenant: string; incarnation: string; generation: number;
  routes: GovernanceRouteView[]; permits: GovernancePermitView[];
  closed_resources: GovernanceResource[]; revoked_subjects: string[];
}

export type GovernanceRegistryProjection = 'agent' | 'card';
export interface GovernanceRegistryMutationRequest {
  namespace: string; tenant: string; agent_id: string; change_id: string;
  expected_registry_revision: number; projection: GovernanceRegistryProjection;
  expected_projection_version: number | null; value: Record<string, unknown> | null; reason: string;
}
export interface GovernanceRegistryProjectionView {
  namespace: string; tenant: string; agent_id: string; agent_resource: GovernanceResource;
  projection: GovernanceRegistryProjection; registry_revision: number;
  qualification_retired: boolean | null; version: number | null;
  value: Record<string, unknown> | null;
}
export interface GovernanceRegistryMutationReceipt {
  namespace: string; tenant: string; agent_id: string; change_id: string;
  projection: GovernanceRegistryProjection; expected_registry_revision: number;
  input_digest: string; actor: string; delivery_complete: boolean; applied: boolean;
}

export type ProviderHistoryStatus =
  | { state: 'prepared' }
  | { state: 'in_flight'; attempt_id: string }
  | { state: 'awaiting_retry'; not_before_ms: number }
  | { state: 'completed'; outcome: unknown }
  | { state: 'reconciliation_required'; attempt_id: string };
export interface ProviderEvidenceReference { id: string; digest: string; }
export interface ProviderHistoryReconciliation {
  prior_status: 'in_flight' | 'settled' | 'uncertain'; execution_id: string; attempt_id: string;
  original_evidence: ProviderEvidenceReference | null; resolution: ProviderEvidenceReference;
  verifier_revision: string; proof_digest: string; resolved_at_ms: number; outcome: unknown;
  acceptance?: { operator: PrincipalIdentity; authority: { incarnation: string; generation: number }; accepted_at_ms: number } | null;
}
export interface ProviderHistoryAttempt {
  attempt_id: string; ordinal: number; ledger_status: 'in_flight' | 'settled' | 'uncertain';
  original_evidence: ProviderEvidenceReference | null; original_outcome: unknown;
  reconciliation: ProviderHistoryReconciliation | null;
}
export interface ProviderExecutionHistory {
  subject: PrincipalIdentity;
  receipt: { execution_id: string; attempts: number; status: ProviderHistoryStatus };
  observed_authority: { incarnation: string; generation: number };
  operation_integrity: 'unstarted' | 'sealed' | 'legacy';
  metadata: { original_action_id: string; max_attempts: number } | null;
  binding: { provider: string; provider_revision: string; failure_revision: string; effect: GovernanceEffect } | null;
  cancellation_fenced: boolean; attempts: ProviderHistoryAttempt[];
}
