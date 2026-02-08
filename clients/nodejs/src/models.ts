/**
 * Data models for the Acteon client.
 */

import { randomUUID } from "crypto";

/**
 * An action to be dispatched through Acteon.
 */
export interface Action {
  /** Unique action identifier. */
  id: string;
  /** Logical grouping for the action. */
  namespace: string;
  /** Tenant identifier for multi-tenancy. */
  tenant: string;
  /** Target provider name (e.g., "email", "sms"). */
  provider: string;
  /** Type of action (e.g., "send_notification"). */
  actionType: string;
  /** Action-specific data. */
  payload: Record<string, unknown>;
  /** Optional deduplication key. */
  dedupKey?: string;
  /** Optional key-value metadata. */
  metadata?: Record<string, string>;
  /** Timestamp when the action was created. */
  createdAt: string;
}

/**
 * Create a new action with auto-generated ID.
 */
export function createAction(
  namespace: string,
  tenant: string,
  provider: string,
  actionType: string,
  payload: Record<string, unknown>,
  options?: {
    id?: string;
    dedupKey?: string;
    metadata?: Record<string, string>;
    createdAt?: string;
  }
): Action {
  return {
    id: options?.id ?? randomUUID(),
    namespace,
    tenant,
    provider,
    actionType,
    payload,
    dedupKey: options?.dedupKey,
    metadata: options?.metadata,
    createdAt: options?.createdAt ?? new Date().toISOString(),
  };
}

/**
 * Convert an Action to the API request format.
 */
export function actionToRequest(action: Action): Record<string, unknown> {
  const result: Record<string, unknown> = {
    id: action.id,
    namespace: action.namespace,
    tenant: action.tenant,
    provider: action.provider,
    action_type: action.actionType,
    payload: action.payload,
    created_at: action.createdAt,
  };
  if (action.dedupKey) {
    result.dedup_key = action.dedupKey;
  }
  if (action.metadata) {
    result.metadata = { labels: action.metadata };
  }
  return result;
}

/**
 * Response from a provider after executing an action.
 */
export interface ProviderResponse {
  status: "success" | "failure" | "partial";
  body: Record<string, unknown>;
  headers: Record<string, string>;
}

/**
 * Outcome of dispatching an action.
 */
export type ActionOutcome =
  | { type: "executed"; response: ProviderResponse }
  | { type: "deduplicated" }
  | { type: "suppressed"; rule: string }
  | {
      type: "rerouted";
      originalProvider: string;
      newProvider: string;
      response: ProviderResponse;
    }
  | { type: "throttled"; retryAfterSecs: number }
  | { type: "failed"; error: ActionError }
  | { type: "dry_run"; verdict: string; matchedRule?: string; wouldBeProvider: string }
  | { type: "scheduled"; actionId: string; scheduledFor: string };

/**
 * Error details when an action fails.
 */
export interface ActionError {
  code: string;
  message: string;
  retryable: boolean;
  attempts: number;
}

/**
 * Parse an ActionOutcome from API response.
 */
export function parseActionOutcome(data: unknown): ActionOutcome {
  // Handle string variant (e.g., "Deduplicated")
  if (data === "Deduplicated") {
    return { type: "deduplicated" };
  }

  if (typeof data !== "object" || data === null) {
    return { type: "failed", error: { code: "UNKNOWN", message: "Invalid response", retryable: false, attempts: 0 } };
  }

  const obj = data as Record<string, unknown>;

  if ("Executed" in obj) {
    const resp = obj.Executed as Record<string, unknown>;
    return {
      type: "executed",
      response: {
        status: (resp.status as "success" | "failure" | "partial") ?? "success",
        body: (resp.body as Record<string, unknown>) ?? {},
        headers: (resp.headers as Record<string, string>) ?? {},
      },
    };
  }

  if ("Deduplicated" in obj) {
    return { type: "deduplicated" };
  }

  if ("Suppressed" in obj) {
    const suppressed = obj.Suppressed as Record<string, unknown>;
    return { type: "suppressed", rule: (suppressed.rule as string) ?? "" };
  }

  if ("Rerouted" in obj) {
    const rerouted = obj.Rerouted as Record<string, unknown>;
    const resp = (rerouted.response as Record<string, unknown>) ?? {};
    return {
      type: "rerouted",
      originalProvider: (rerouted.original_provider as string) ?? "",
      newProvider: (rerouted.new_provider as string) ?? "",
      response: {
        status: (resp.status as "success" | "failure" | "partial") ?? "success",
        body: (resp.body as Record<string, unknown>) ?? {},
        headers: (resp.headers as Record<string, string>) ?? {},
      },
    };
  }

  if ("Throttled" in obj) {
    const throttled = obj.Throttled as Record<string, unknown>;
    const retryAfter = (throttled.retry_after as Record<string, number>) ?? {};
    const secs = (retryAfter.secs ?? 0) + (retryAfter.nanos ?? 0) / 1e9;
    return { type: "throttled", retryAfterSecs: secs };
  }

  if ("Failed" in obj) {
    const failed = obj.Failed as Record<string, unknown>;
    return {
      type: "failed",
      error: {
        code: (failed.code as string) ?? "UNKNOWN",
        message: (failed.message as string) ?? "Unknown error",
        retryable: (failed.retryable as boolean) ?? false,
        attempts: (failed.attempts as number) ?? 0,
      },
    };
  }

  if ("DryRun" in obj) {
    const dryRun = obj.DryRun as Record<string, unknown>;
    return {
      type: "dry_run",
      verdict: (dryRun.verdict as string) ?? "",
      matchedRule: dryRun.matched_rule as string | undefined,
      wouldBeProvider: (dryRun.would_be_provider as string) ?? "",
    };
  }

  if ("Scheduled" in obj) {
    const scheduled = obj.Scheduled as Record<string, unknown>;
    return {
      type: "scheduled",
      actionId: (scheduled.action_id as string) ?? "",
      scheduledFor: (scheduled.scheduled_for as string) ?? "",
    };
  }

  return { type: "failed", error: { code: "UNKNOWN", message: "Unknown outcome", retryable: false, attempts: 0 } };
}

/**
 * Error response from the API.
 */
export interface ErrorResponse {
  code: string;
  message: string;
  retryable: boolean;
}

/**
 * Result from a batch dispatch operation.
 */
export type BatchResult =
  | { success: true; outcome: ActionOutcome }
  | { success: false; error: ErrorResponse };

/**
 * Parse a BatchResult from API response.
 */
export function parseBatchResult(data: unknown): BatchResult {
  if (typeof data !== "object" || data === null) {
    return { success: false, error: { code: "UNKNOWN", message: "Invalid response", retryable: false } };
  }

  const obj = data as Record<string, unknown>;
  if ("error" in obj) {
    const err = obj.error as Record<string, unknown>;
    return {
      success: false,
      error: {
        code: (err.code as string) ?? "UNKNOWN",
        message: (err.message as string) ?? "Unknown error",
        retryable: (err.retryable as boolean) ?? false,
      },
    };
  }

  return { success: true, outcome: parseActionOutcome(data) };
}

/**
 * Information about a loaded rule.
 */
export interface RuleInfo {
  name: string;
  priority: number;
  enabled: boolean;
  description?: string;
}

/**
 * Result of reloading rules.
 */
export interface ReloadResult {
  loaded: number;
  errors: string[];
}

/**
 * Query parameters for audit search.
 */
export interface AuditQuery {
  namespace?: string;
  tenant?: string;
  provider?: string;
  actionType?: string;
  outcome?: string;
  limit?: number;
  offset?: number;
}

/**
 * Convert AuditQuery to URL search params.
 */
export function auditQueryToParams(query: AuditQuery): URLSearchParams {
  const params = new URLSearchParams();
  if (query.namespace) params.set("namespace", query.namespace);
  if (query.tenant) params.set("tenant", query.tenant);
  if (query.provider) params.set("provider", query.provider);
  if (query.actionType) params.set("action_type", query.actionType);
  if (query.outcome) params.set("outcome", query.outcome);
  if (query.limit !== undefined) params.set("limit", query.limit.toString());
  if (query.offset !== undefined) params.set("offset", query.offset.toString());
  return params;
}

/**
 * An audit record.
 */
export interface AuditRecord {
  id: string;
  actionId: string;
  namespace: string;
  tenant: string;
  provider: string;
  actionType: string;
  verdict: string;
  outcome: string;
  matchedRule?: string;
  durationMs: number;
  dispatchedAt: string;
}

/**
 * Parse an AuditRecord from API response.
 */
export function parseAuditRecord(data: Record<string, unknown>): AuditRecord {
  return {
    id: data.id as string,
    actionId: data.action_id as string,
    namespace: data.namespace as string,
    tenant: data.tenant as string,
    provider: data.provider as string,
    actionType: data.action_type as string,
    verdict: data.verdict as string,
    outcome: data.outcome as string,
    matchedRule: data.matched_rule as string | undefined,
    durationMs: data.duration_ms as number,
    dispatchedAt: data.dispatched_at as string,
  };
}

/**
 * Paginated audit results.
 */
export interface AuditPage {
  records: AuditRecord[];
  total: number;
  limit: number;
  offset: number;
}

/**
 * Parse an AuditPage from API response.
 */
export function parseAuditPage(data: Record<string, unknown>): AuditPage {
  const records = data.records as Record<string, unknown>[];
  return {
    records: records.map(parseAuditRecord),
    total: data.total as number,
    limit: data.limit as number,
    offset: data.offset as number,
  };
}

// =============================================================================
// Event Types (State Machine Lifecycle)
// =============================================================================

/**
 * Query parameters for listing events.
 */
export interface EventQuery {
  namespace: string;
  tenant: string;
  status?: string;
  limit?: number;
}

/**
 * Convert EventQuery to URL search params.
 */
export function eventQueryToParams(query: EventQuery): URLSearchParams {
  const params = new URLSearchParams();
  params.set("namespace", query.namespace);
  params.set("tenant", query.tenant);
  if (query.status) params.set("status", query.status);
  if (query.limit !== undefined) params.set("limit", query.limit.toString());
  return params;
}

/**
 * Current state of an event.
 */
export interface EventState {
  fingerprint: string;
  state: string;
  actionType?: string;
  updatedAt?: string;
}

/**
 * Parse an EventState from API response.
 */
export function parseEventState(data: Record<string, unknown>): EventState {
  return {
    fingerprint: data.fingerprint as string,
    state: data.state as string,
    actionType: data.action_type as string | undefined,
    updatedAt: data.updated_at as string | undefined,
  };
}

/**
 * Response from listing events.
 */
export interface EventListResponse {
  events: EventState[];
  count: number;
}

/**
 * Parse an EventListResponse from API response.
 */
export function parseEventListResponse(data: Record<string, unknown>): EventListResponse {
  const events = data.events as Record<string, unknown>[];
  return {
    events: events.map(parseEventState),
    count: data.count as number,
  };
}

/**
 * Response from transitioning an event.
 */
export interface TransitionResponse {
  fingerprint: string;
  previousState: string;
  newState: string;
  notify: boolean;
}

/**
 * Parse a TransitionResponse from API response.
 */
export function parseTransitionResponse(data: Record<string, unknown>): TransitionResponse {
  return {
    fingerprint: data.fingerprint as string,
    previousState: data.previous_state as string,
    newState: data.new_state as string,
    notify: data.notify as boolean,
  };
}

// =============================================================================
// Group Types (Event Batching)
// =============================================================================

/**
 * Summary of an event group.
 */
export interface GroupSummary {
  groupId: string;
  groupKey: string;
  eventCount: number;
  state: string;
  notifyAt?: string;
  createdAt?: string;
}

/**
 * Parse a GroupSummary from API response.
 */
export function parseGroupSummary(data: Record<string, unknown>): GroupSummary {
  return {
    groupId: data.group_id as string,
    groupKey: data.group_key as string,
    eventCount: data.event_count as number,
    state: data.state as string,
    notifyAt: data.notify_at as string | undefined,
    createdAt: data.created_at as string | undefined,
  };
}

/**
 * Response from listing groups.
 */
export interface GroupListResponse {
  groups: GroupSummary[];
  total: number;
}

/**
 * Parse a GroupListResponse from API response.
 */
export function parseGroupListResponse(data: Record<string, unknown>): GroupListResponse {
  const groups = data.groups as Record<string, unknown>[];
  return {
    groups: groups.map(parseGroupSummary),
    total: data.total as number,
  };
}

/**
 * Detailed information about a group.
 */
export interface GroupDetail {
  group: GroupSummary;
  events: string[];
  labels: Record<string, string>;
}

/**
 * Parse a GroupDetail from API response.
 */
export function parseGroupDetail(data: Record<string, unknown>): GroupDetail {
  return {
    group: parseGroupSummary(data.group as Record<string, unknown>),
    events: (data.events as string[]) ?? [],
    labels: (data.labels as Record<string, string>) ?? {},
  };
}

/**
 * Response from flushing a group.
 */
export interface FlushGroupResponse {
  groupId: string;
  eventCount: number;
  notified: boolean;
}

/**
 * Parse a FlushGroupResponse from API response.
 */
export function parseFlushGroupResponse(data: Record<string, unknown>): FlushGroupResponse {
  return {
    groupId: data.group_id as string,
    eventCount: data.event_count as number,
    notified: data.notified as boolean,
  };
}

// =============================================================================
// Approval Types (Human-in-the-Loop)
// =============================================================================

/**
 * Response from approving or rejecting an action.
 */
export interface ApprovalActionResponse {
  id: string;
  status: string;
  outcome?: Record<string, unknown>;
}

/**
 * Parse an ApprovalActionResponse from API response.
 */
export function parseApprovalActionResponse(data: Record<string, unknown>): ApprovalActionResponse {
  return {
    id: data.id as string,
    status: data.status as string,
    outcome: data.outcome as Record<string, unknown> | undefined,
  };
}

/**
 * Public-facing approval status (no payload exposed).
 */
export interface ApprovalStatus {
  token: string;
  status: string;
  rule: string;
  createdAt: string;
  expiresAt: string;
  decidedAt?: string;
  message?: string;
}

/**
 * Parse an ApprovalStatus from API response.
 */
export function parseApprovalStatus(data: Record<string, unknown>): ApprovalStatus {
  return {
    token: data.token as string,
    status: data.status as string,
    rule: data.rule as string,
    createdAt: data.created_at as string,
    expiresAt: data.expires_at as string,
    decidedAt: data.decided_at as string | undefined,
    message: data.message as string | undefined,
  };
}

/**
 * Response from listing pending approvals.
 */
export interface ApprovalListResponse {
  approvals: ApprovalStatus[];
  count: number;
}

/**
 * Parse an ApprovalListResponse from API response.
 */
export function parseApprovalListResponse(data: Record<string, unknown>): ApprovalListResponse {
  const approvals = data.approvals as Record<string, unknown>[];
  return {
    approvals: approvals.map(parseApprovalStatus),
    count: data.count as number,
  };
}

// =============================================================================
// Webhook Helpers
// =============================================================================

/**
 * Payload for webhook actions.
 *
 * Use this to build the payload for an Action targeted at the webhook provider.
 */
export interface WebhookPayload {
  /** Target URL for the webhook request. */
  url: string;
  /** The JSON body to send to the webhook endpoint. */
  body: Record<string, unknown>;
  /** HTTP method (default: "POST"). */
  method?: string;
  /** Additional HTTP headers to include. */
  headers?: Record<string, string>;
}

/**
 * Create an Action targeting the webhook provider.
 *
 * This is a convenience function that constructs a properly formatted Action
 * for the webhook provider, wrapping the URL, method, headers, and body into
 * the payload.
 *
 * @example
 * ```typescript
 * const action = createWebhookAction(
 *   "notifications",
 *   "tenant-1",
 *   "https://hooks.example.com/alert",
 *   { message: "Server is down", severity: "critical" },
 *   { headers: { "X-Custom-Header": "value" } }
 * );
 * ```
 */
export function createWebhookAction(
  namespace: string,
  tenant: string,
  url: string,
  body: Record<string, unknown>,
  options?: {
    actionType?: string;
    method?: string;
    headers?: Record<string, string>;
    dedupKey?: string;
    metadata?: Record<string, string>;
  }
): Action {
  const payload: Record<string, unknown> = {
    url,
    method: options?.method ?? "POST",
    body,
  };
  if (options?.headers) {
    payload.headers = options.headers;
  }
  return createAction(namespace, tenant, "webhook", options?.actionType ?? "webhook", payload, {
    dedupKey: options?.dedupKey,
    metadata: options?.metadata,
  });
}

// =============================================================================
// Replay Types
// =============================================================================

/** Result of replaying a single action. */
export interface ReplayResult {
  originalActionId: string;
  newActionId: string;
  success: boolean;
  error?: string;
}

/** Summary of a bulk replay operation. */
export interface ReplaySummary {
  replayed: number;
  failed: number;
  skipped: number;
  results: ReplayResult[];
}

/** Query parameters for bulk audit replay. */
export interface ReplayQuery {
  namespace?: string;
  tenant?: string;
  provider?: string;
  actionType?: string;
  outcome?: string;
  verdict?: string;
  matchedRule?: string;
  from?: string;
  to?: string;
  limit?: number;
}

export function parseReplayResult(data: Record<string, unknown>): ReplayResult {
  return {
    originalActionId: data.original_action_id as string,
    newActionId: data.new_action_id as string,
    success: data.success as boolean,
    error: data.error as string | undefined,
  };
}

export function parseReplaySummary(data: Record<string, unknown>): ReplaySummary {
  const results = (data.results as Record<string, unknown>[]).map(parseReplayResult);
  return {
    replayed: data.replayed as number,
    failed: data.failed as number,
    skipped: data.skipped as number,
    results,
  };
}

export function replayQueryToParams(query: ReplayQuery): URLSearchParams {
  const params = new URLSearchParams();
  if (query.namespace !== undefined) params.set("namespace", query.namespace);
  if (query.tenant !== undefined) params.set("tenant", query.tenant);
  if (query.provider !== undefined) params.set("provider", query.provider);
  if (query.actionType !== undefined) params.set("action_type", query.actionType);
  if (query.outcome !== undefined) params.set("outcome", query.outcome);
  if (query.verdict !== undefined) params.set("verdict", query.verdict);
  if (query.matchedRule !== undefined) params.set("matched_rule", query.matchedRule);
  if (query.from !== undefined) params.set("from", query.from);
  if (query.to !== undefined) params.set("to", query.to);
  if (query.limit !== undefined) params.set("limit", query.limit.toString());
  return params;
}
