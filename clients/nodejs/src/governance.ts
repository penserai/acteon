/** Public wire types. Supplying these values does not grant management authority. */
import { parseActionOutcome, type ActionOutcome, type PrincipalIdentity } from "./models.js";
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
  /** Absent on older servers; absence means denied. */
  can_read_history?: boolean;
  /** Independent qualified finality acceptance; absent means denied. */
  can_reconcile?: boolean;
  valid_from_ms: number; limits: GovernanceLimits;
}
export interface GovernanceScopeView {
  management: GovernanceManagementBounds;
  namespace: string; tenant: string; incarnation: string; generation: number;
  routes: GovernanceRouteView[]; permits: GovernancePermitView[];
  closed_resources: GovernanceResource[]; revoked_subjects: string[];
}

/** Verified retained evidence. Reading it never admits or resumes execution. */
export interface ProviderExecutionHistory {
  subject: PrincipalIdentity;
  receipt: { execution_id: string; attempts: number; status: ProviderHistoryStatus };
  observed_authority: { incarnation: string; generation: number };
  operation_integrity: "unstarted" | "sealed" | "legacy";
  metadata: { original_action_id: string; max_attempts: number } | null;
  binding: { provider: string; provider_revision: string; failure_revision: string; effect: GovernanceEffect } | null;
  cancellation_fenced: boolean;
  attempts: ProviderHistoryAttempt[];
}
export type ProviderHistoryStatus =
  | { state: "prepared" }
  | { state: "in_flight" | "reconciliation_required"; attempt_id: string }
  | { state: "awaiting_retry"; not_before_ms: number }
  | { state: "completed"; outcome: ActionOutcome };
export type ProviderAttemptStatus = "in_flight" | "settled" | "uncertain";
export interface ProviderEvidenceReference { id: string; digest: string; }
export interface ProviderHistoryAttempt {
  attempt_id: string; ordinal: number; ledger_status: ProviderAttemptStatus;
  original_evidence: ProviderEvidenceReference | null;
  original_outcome: ActionOutcome | null;
  reconciliation: ProviderHistoryReconciliation | null;
}
export interface ProviderReconciliationAcceptance {
  operator: PrincipalIdentity;
  authority: { incarnation: string; generation: number };
  accepted_at_ms: number;
}
export interface ProviderHistoryReconciliation {
  prior_status: ProviderAttemptStatus; execution_id: string; attempt_id: string;
  original_evidence: ProviderEvidenceReference | null; resolution: ProviderEvidenceReference;
  verifier_revision: string; proof_digest: string; resolved_at_ms: number; outcome: ActionOutcome;
  acceptance?: ProviderReconciliationAcceptance | null;
}
/** Server wire outcomes are Rust enums, separate from normalized SDK outcomes. */
export type ProviderExecutionHistoryWire = Omit<ProviderExecutionHistory, "receipt" | "attempts"> & {
  receipt: Omit<ProviderExecutionHistory["receipt"], "status"> & { status:
    Exclude<ProviderHistoryStatus, { state: "completed" }> | { state: "completed"; outcome: unknown } };
  attempts: Array<Omit<ProviderHistoryAttempt, "original_outcome" | "reconciliation"> & {
    original_outcome: unknown | null;
    reconciliation: (Omit<ProviderHistoryReconciliation, "outcome"> & { outcome: unknown }) | null;
  }>;
};
export interface ProviderReconciliationContext {
  context_id: string; execution_id: string; namespace: string; tenant: string;
  principal: PrincipalIdentity; request_digest: string;
}
export interface ProviderReconciliationCorrelation {
  context: ProviderReconciliationContext; action_id: string; attempt_id: string;
  ordinal: number; token: string; binding_digest: string;
}
export interface ProviderReconciliationRequest { proof_base64: string; }
export type ProviderHistoryReceipt = ProviderExecutionHistory["receipt"];
export function parseProviderHistoryReceipt(data: ProviderExecutionHistoryWire["receipt"]): ProviderHistoryReceipt {
  return { ...data, status: data.status.state === "completed"
    ? { ...data.status, outcome: parseActionOutcome(data.status.outcome) } : data.status };
}
/** Normalize every nested outcome using the same decoder as dispatch. */
export function parseProviderExecutionHistory(data: ProviderExecutionHistoryWire): ProviderExecutionHistory {
  return {
    ...data,
    receipt: parseProviderHistoryReceipt(data.receipt),
    attempts: data.attempts.map(attempt => ({
      ...attempt,
      original_outcome: attempt.original_outcome === null ? null : parseActionOutcome(attempt.original_outcome),
      reconciliation: attempt.reconciliation === null ? null : {
        ...attempt.reconciliation, outcome: parseActionOutcome(attempt.reconciliation.outcome),
      },
    })),
  };
}
