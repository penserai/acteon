/** Host-owned receipts; source context is never extracted from model data. */
export const AGENT_SOURCE_CONTEXT_HEADER = "x-acteon-agent-source-context";
export const AGENT_EXECUTION_CONTEXT_HEADER = "x-acteon-execution-context";
export function agentExecutionContext(value: string): string {
  if (!value || value.length > 8192 || !/^[A-Za-z0-9_-]+$/.test(value)) throw new Error("agent service execution context malformed");
  return value;
}
export interface AgentServiceReceipt {
  readonly namespace: string;
  readonly tenant: string;
  readonly agent: string;
  readonly taskId: string;
  readonly sourceContext: string;
  readonly task: Record<string, unknown>;
}
export type AgentPeerSendStatus =
  | Readonly<{ state: "uncertain" }>
  | Readonly<{ state: "accepted"; task: Record<string, unknown> }>
  | Readonly<{ state: "rejected"; code: string }>;
export interface AgentPeerSendReceipt {
  readonly submissionId: string;
  readonly status: AgentPeerSendStatus;
}
export type AgentPeerCancelStatus =
  | Readonly<{ state: "unsupported" }>
  | Readonly<{ state: "rejected"; code: string }>
  | Readonly<{ state: "uncertain" }>
  | Readonly<{ state: "reconciled"; task: Record<string, unknown> }>;
export interface AgentPeerCancelReceipt {
  readonly submissionId: string;
  readonly cancellationId: string;
  readonly status: AgentPeerCancelStatus;
}
export function agentPeerReceipt(value: unknown, namespace: string, tenant: string): AgentPeerSendReceipt {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("agent peer receipt missing or malformed");
  const raw = value as Record<string, unknown>;
  if (Object.keys(raw).sort().join(",") !== "status,submission_id" || typeof raw.submission_id !== "string" || !/^[0-9a-f]{8}-[0-9a-f]{4}-5[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(raw.submission_id)) throw new Error("agent peer receipt missing or malformed");
  if (!raw.status || typeof raw.status !== "object" || Array.isArray(raw.status)) throw new Error("agent peer receipt missing or malformed");
  const status = raw.status as Record<string, unknown>;
  const keys = Object.keys(status).sort().join(",");
  if (status.state === "uncertain" && keys === "state") return Object.freeze({ submissionId: raw.submission_id, status: Object.freeze({ state: "uncertain" as const }) });
  if (status.state === "accepted" && keys === "state,task") return Object.freeze({ submissionId: raw.submission_id, status: Object.freeze({ state: "accepted" as const, task: agentTask(status.task, namespace, tenant) }) });
  if (status.state === "rejected" && keys === "code,state" && typeof status.code === "string" && status.code.length > 0 && status.code.length <= 1024 && status.code.trim() === status.code && !/\p{Cc}/u.test(status.code)) return Object.freeze({ submissionId: raw.submission_id, status: Object.freeze({ state: "rejected" as const, code: status.code }) });
  throw new Error("agent peer receipt missing or malformed");
}
export function agentPeerCancelReceipt(value: unknown, namespace: string, tenant: string, peer: AgentPeerSendReceipt): AgentPeerCancelReceipt {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("agent peer cancellation receipt missing or malformed");
  const raw = value as Record<string, unknown>;
  const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-5[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
  if (Object.keys(raw).sort().join(",") !== "cancellation_id,status,submission_id" || raw.submission_id !== peer.submissionId || typeof raw.cancellation_id !== "string" || !uuid.test(raw.cancellation_id)) throw new Error("agent peer cancellation receipt missing or malformed");
  if (peer.status.state !== "accepted" || !raw.status || typeof raw.status !== "object" || Array.isArray(raw.status)) throw new Error("agent peer cancellation requires an accepted peer receipt");
  const acceptedTask = agentTask(peer.status.task, namespace, tenant);
  const status = raw.status as Record<string, unknown>;
  const keys = Object.keys(status).sort().join(",");
  let parsed: AgentPeerCancelStatus;
  if (status.state === "unsupported" && keys === "state") parsed = Object.freeze({ state: "unsupported" });
  else if (status.state === "uncertain" && keys === "state") parsed = Object.freeze({ state: "uncertain" });
  else if (status.state === "rejected" && keys === "code,state" && typeof status.code === "string" && status.code.length > 0 && status.code.length <= 1024 && status.code.trim() === status.code && !/\p{Cc}/u.test(status.code)) parsed = Object.freeze({ state: "rejected", code: status.code });
  else if (status.state === "reconciled" && keys === "state,task") {
    const task = agentTask(status.task, namespace, tenant, acceptedTask.id as string);
    const taskStatus = task.status as Record<string, unknown> | undefined;
    if (!taskStatus || !["completed", "failed", "canceled", "rejected"].includes(String(taskStatus.state))) throw new Error("agent peer cancellation receipt missing or malformed");
    parsed = Object.freeze({ state: "reconciled", task });
  } else throw new Error("agent peer cancellation receipt missing or malformed");
  return Object.freeze({ submissionId: peer.submissionId, cancellationId: raw.cancellation_id, status: parsed });
}
export function agentSource(value: string | null): string {
  if (!value || value.length > 8192 || !/^[A-Za-z0-9_-]+$/.test(value)) throw new Error("agent service source context missing or malformed");
  return value;
}
export function agentTask(value: unknown, namespace: string, tenant: string, id?: string): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("agent service task identity mismatch");
  const task = value as Record<string, unknown>;
  if (typeof task.id !== "string" || !task.id || task.namespace !== namespace || task.tenant !== tenant || (id !== undefined && task.id !== id)) throw new Error("agent service task identity mismatch");
  return task;
}
export function agentServiceBase(namespace: string, tenant: string, agent: string): string {
  const segment = (value: string) => {
    if (!value || value === "." || value === "..") throw new Error("invalid agent service path segment");
    return encodeURIComponent(value);
  };
  return `/a2a/${segment(namespace)}/${segment(tenant)}/agents/${segment(agent)}/v1`;
}

export type AgentServiceProviderAbort =
  | Readonly<{ state: "restricted_only" }>
  | Readonly<{ state: "uncertain"; attemptId: string }>
  | Readonly<{ state: "reconciled"; proofDigest: string }>;

export function agentProviderAbort(value: unknown): AgentServiceProviderAbort | undefined {
  if (value === undefined || value === null) return undefined;
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("agent service provider abort status malformed");
  const raw = value as Record<string, unknown>;
  const keys = Object.keys(raw).sort().join(",");
  if (raw.state === "restricted_only" && keys === "state") return Object.freeze({ state: "restricted_only" });
  if (raw.state === "uncertain" && keys === "attempt_id,state" && typeof raw.attempt_id === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-5[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(raw.attempt_id)) {
    return Object.freeze({ state: "uncertain", attemptId: raw.attempt_id });
  }
  if (raw.state === "reconciled" && keys === "proof_digest,state" && typeof raw.proof_digest === "string" && /^[0-9a-f]{64}$/.test(raw.proof_digest)) {
    return Object.freeze({ state: "reconciled", proofDigest: raw.proof_digest });
  }
  throw new Error("agent service provider abort status malformed");
}

/** Acknowledges future-start restriction and preserves separate provider finality. */
export interface AgentServiceStopReceipt {
  readonly task: Record<string, unknown>;
  readonly futureStartsBlocked: true;
  readonly providerAbort?: AgentServiceProviderAbort;
}
