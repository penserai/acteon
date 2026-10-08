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
