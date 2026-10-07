/** Host-owned receipts; source context is never extracted from model data. */
export const AGENT_SOURCE_CONTEXT_HEADER = "x-acteon-agent-source-context";
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
