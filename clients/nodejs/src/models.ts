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
  | { type: "scheduled"; actionId: string; scheduledFor: string }
  | { type: "quota_exceeded"; tenant: string; limit: number; used: number; overageBehavior: string };

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

  if ("QuotaExceeded" in obj) {
    const quota = obj.QuotaExceeded as Record<string, unknown>;
    return {
      type: "quota_exceeded",
      tenant: (quota.tenant as string) ?? "",
      limit: (quota.limit as number) ?? 0,
      used: (quota.used as number) ?? 0,
      overageBehavior: (quota.overage_behavior as string) ?? "",
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

// =============================================================================
// Rule Playground Types
// =============================================================================

/**
 * Request to evaluate rules against a test action without dispatching.
 */
export interface EvaluateRulesRequest {
  namespace: string;
  tenant: string;
  provider: string;
  action_type: string;
  payload: Record<string, unknown>;
  metadata?: Record<string, string>;
  include_disabled?: boolean;
  evaluate_all?: boolean;
  evaluate_at?: string | null;
  mock_state?: Record<string, string>;
}

/**
 * Details about a semantic match evaluation performed during rule evaluation.
 */
export interface SemanticMatchDetail {
  /** The text extracted from the action payload for semantic comparison. */
  extracted_text: string;
  /** The topic the rule is matching against. */
  topic: string;
  /** The computed similarity score between the extracted text and the topic. */
  similarity: number;
  /** The threshold the similarity must meet for a match. */
  threshold: number;
}

/**
 * Trace entry for a single rule evaluation.
 */
export interface RuleTraceEntry {
  rule_name: string;
  priority: number;
  enabled: boolean;
  condition_display: string;
  result: 'matched' | 'not_matched' | 'skipped' | 'error';
  evaluation_duration_us: number;
  action: string;
  source: string;
  description?: string;
  skip_reason?: string;
  error?: string;
  /** Details about semantic match evaluation, if the rule uses semantic matching. */
  semantic_details?: SemanticMatchDetail;
  /** JSON merge patch produced by a Modify rule in evaluate_all mode. */
  modify_patch?: Record<string, unknown>;
  /** Cumulative payload after applying this rule's merge patch. */
  modified_payload_preview?: Record<string, unknown>;
}

/**
 * Response from the rule evaluation playground.
 */
export interface EvaluateRulesResponse {
  verdict: string;
  matched_rule?: string;
  has_errors: boolean;
  total_rules_evaluated: number;
  total_rules_skipped: number;
  evaluation_duration_us: number;
  trace: RuleTraceEntry[];
  context: {
    time: Record<string, unknown>;
    environment_keys: string[];
    effective_timezone?: string;
    /** State keys that were actually accessed during rule evaluation. */
    accessed_state_keys?: string[];
  };
  modified_payload?: Record<string, unknown>;
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

// =============================================================================
// Recurring Action Types
// =============================================================================

/** Request to create a recurring action. */
export interface CreateRecurringAction {
  namespace: string;
  tenant: string;
  provider: string;
  actionType: string;
  payload: Record<string, unknown>;
  cronExpression: string;
  name?: string;
  metadata?: Record<string, string>;
  timezone?: string;
  endDate?: string;
  maxExecutions?: number;
  description?: string;
  dedupKey?: string;
  labels?: Record<string, string>;
}

/** Convert a CreateRecurringAction to the API request format. */
export function createRecurringActionToRequest(action: CreateRecurringAction): Record<string, unknown> {
  const result: Record<string, unknown> = {
    namespace: action.namespace,
    tenant: action.tenant,
    provider: action.provider,
    action_type: action.actionType,
    payload: action.payload,
    cron_expression: action.cronExpression,
  };
  if (action.name !== undefined) result.name = action.name;
  if (action.metadata !== undefined) result.metadata = action.metadata;
  if (action.timezone !== undefined) result.timezone = action.timezone;
  if (action.endDate !== undefined) result.end_date = action.endDate;
  if (action.maxExecutions !== undefined) result.max_executions = action.maxExecutions;
  if (action.description !== undefined) result.description = action.description;
  if (action.dedupKey !== undefined) result.dedup_key = action.dedupKey;
  if (action.labels !== undefined) result.labels = action.labels;
  return result;
}

/** Response from creating a recurring action. */
export interface CreateRecurringResponse {
  id: string;
  status: string;
  name?: string;
  nextExecutionAt?: string;
}

export function parseCreateRecurringResponse(data: Record<string, unknown>): CreateRecurringResponse {
  return {
    id: data.id as string,
    status: data.status as string,
    name: data.name as string | undefined,
    nextExecutionAt: data.next_execution_at as string | undefined,
  };
}

/** Query parameters for listing recurring actions. */
export interface RecurringFilter {
  namespace?: string;
  tenant?: string;
  status?: string;
  limit?: number;
  offset?: number;
}

export function recurringFilterToParams(filter: RecurringFilter): URLSearchParams {
  const params = new URLSearchParams();
  if (filter.namespace !== undefined) params.set("namespace", filter.namespace);
  if (filter.tenant !== undefined) params.set("tenant", filter.tenant);
  if (filter.status !== undefined) params.set("status", filter.status);
  if (filter.limit !== undefined) params.set("limit", filter.limit.toString());
  if (filter.offset !== undefined) params.set("offset", filter.offset.toString());
  return params;
}

/** Summary of a recurring action in list responses. */
export interface RecurringSummary {
  id: string;
  namespace: string;
  tenant: string;
  cronExpr: string;
  timezone: string;
  enabled: boolean;
  provider: string;
  actionType: string;
  executionCount: number;
  createdAt: string;
  nextExecutionAt?: string;
  description?: string;
}

export function parseRecurringSummary(data: Record<string, unknown>): RecurringSummary {
  return {
    id: data.id as string,
    namespace: data.namespace as string,
    tenant: data.tenant as string,
    cronExpr: data.cron_expr as string,
    timezone: data.timezone as string,
    enabled: data.enabled as boolean,
    provider: data.provider as string,
    actionType: data.action_type as string,
    executionCount: data.execution_count as number,
    createdAt: data.created_at as string,
    nextExecutionAt: data.next_execution_at as string | undefined,
    description: data.description as string | undefined,
  };
}

/** Response from listing recurring actions. */
export interface ListRecurringResponse {
  recurringActions: RecurringSummary[];
  count: number;
}

export function parseListRecurringResponse(data: Record<string, unknown>): ListRecurringResponse {
  const items = data.recurring_actions as Record<string, unknown>[];
  return {
    recurringActions: items.map(parseRecurringSummary),
    count: data.count as number,
  };
}

/** Detailed information about a recurring action. */
export interface RecurringDetail {
  id: string;
  namespace: string;
  tenant: string;
  cronExpr: string;
  timezone: string;
  enabled: boolean;
  provider: string;
  actionType: string;
  payload: Record<string, unknown>;
  metadata: Record<string, string>;
  executionCount: number;
  createdAt: string;
  updatedAt: string;
  labels: Record<string, string>;
  nextExecutionAt?: string;
  lastExecutedAt?: string;
  endsAt?: string;
  description?: string;
  dedupKey?: string;
}

export function parseRecurringDetail(data: Record<string, unknown>): RecurringDetail {
  return {
    id: data.id as string,
    namespace: data.namespace as string,
    tenant: data.tenant as string,
    cronExpr: data.cron_expr as string,
    timezone: data.timezone as string,
    enabled: data.enabled as boolean,
    provider: data.provider as string,
    actionType: data.action_type as string,
    payload: (data.payload as Record<string, unknown>) ?? {},
    metadata: (data.metadata as Record<string, string>) ?? {},
    executionCount: data.execution_count as number,
    createdAt: data.created_at as string,
    updatedAt: data.updated_at as string,
    labels: (data.labels as Record<string, string>) ?? {},
    nextExecutionAt: data.next_execution_at as string | undefined,
    lastExecutedAt: data.last_executed_at as string | undefined,
    endsAt: data.ends_at as string | undefined,
    description: data.description as string | undefined,
    dedupKey: data.dedup_key as string | undefined,
  };
}

/** Request to update a recurring action. */
export interface UpdateRecurringAction {
  namespace: string;
  tenant: string;
  name?: string;
  payload?: Record<string, unknown>;
  metadata?: Record<string, string>;
  cronExpression?: string;
  timezone?: string;
  endDate?: string;
  maxExecutions?: number;
  description?: string;
  dedupKey?: string;
  labels?: Record<string, string>;
}

export function updateRecurringActionToRequest(action: UpdateRecurringAction): Record<string, unknown> {
  const result: Record<string, unknown> = {
    namespace: action.namespace,
    tenant: action.tenant,
  };
  if (action.name !== undefined) result.name = action.name;
  if (action.payload !== undefined) result.payload = action.payload;
  if (action.metadata !== undefined) result.metadata = action.metadata;
  if (action.cronExpression !== undefined) result.cron_expression = action.cronExpression;
  if (action.timezone !== undefined) result.timezone = action.timezone;
  if (action.endDate !== undefined) result.end_date = action.endDate;
  if (action.maxExecutions !== undefined) result.max_executions = action.maxExecutions;
  if (action.description !== undefined) result.description = action.description;
  if (action.dedupKey !== undefined) result.dedup_key = action.dedupKey;
  if (action.labels !== undefined) result.labels = action.labels;
  return result;
}

// =============================================================================
// Quota Types
// =============================================================================

/** Request to create a quota policy. */
export interface CreateQuotaRequest {
  namespace: string;
  tenant: string;
  maxActions: number;
  window: string;
  overageBehavior: string;
  description?: string;
  labels?: Record<string, string>;
}

/** Convert a CreateQuotaRequest to the API request format. */
export function createQuotaRequestToApi(req: CreateQuotaRequest): Record<string, unknown> {
  const result: Record<string, unknown> = {
    namespace: req.namespace,
    tenant: req.tenant,
    max_actions: req.maxActions,
    window: req.window,
    overage_behavior: req.overageBehavior,
  };
  if (req.description !== undefined) result.description = req.description;
  if (req.labels !== undefined) result.labels = req.labels;
  return result;
}

/** Request to update a quota policy. */
export interface UpdateQuotaRequest {
  namespace: string;
  tenant: string;
  maxActions?: number;
  window?: string;
  overageBehavior?: string;
  description?: string;
  enabled?: boolean;
}

/** Convert an UpdateQuotaRequest to the API request format. */
export function updateQuotaRequestToApi(req: UpdateQuotaRequest): Record<string, unknown> {
  const result: Record<string, unknown> = {
    namespace: req.namespace,
    tenant: req.tenant,
  };
  if (req.maxActions !== undefined) result.max_actions = req.maxActions;
  if (req.window !== undefined) result.window = req.window;
  if (req.overageBehavior !== undefined) result.overage_behavior = req.overageBehavior;
  if (req.description !== undefined) result.description = req.description;
  if (req.enabled !== undefined) result.enabled = req.enabled;
  return result;
}

/** A quota policy. */
export interface QuotaPolicy {
  id: string;
  namespace: string;
  tenant: string;
  maxActions: number;
  window: string;
  overageBehavior: string;
  enabled: boolean;
  createdAt: string;
  updatedAt: string;
  description?: string;
  labels?: Record<string, string>;
}

/** Parse a QuotaPolicy from API response. */
export function parseQuotaPolicy(data: Record<string, unknown>): QuotaPolicy {
  return {
    id: data.id as string,
    namespace: data.namespace as string,
    tenant: data.tenant as string,
    maxActions: data.max_actions as number,
    window: data.window as string,
    overageBehavior: data.overage_behavior as string,
    enabled: data.enabled as boolean,
    createdAt: data.created_at as string,
    updatedAt: data.updated_at as string,
    description: data.description as string | undefined,
    labels: data.labels as Record<string, string> | undefined,
  };
}

/** Response from listing quota policies. */
export interface ListQuotasResponse {
  quotas: QuotaPolicy[];
  count: number;
}

/** Parse a ListQuotasResponse from API response. */
export function parseListQuotasResponse(data: Record<string, unknown>): ListQuotasResponse {
  const items = data.quotas as Record<string, unknown>[];
  return {
    quotas: items.map(parseQuotaPolicy),
    count: data.count as number,
  };
}

/** Current usage statistics for a quota. */
export interface QuotaUsage {
  tenant: string;
  namespace: string;
  used: number;
  limit: number;
  remaining: number;
  window: string;
  resetsAt: string;
  overageBehavior: string;
}

/** Parse a QuotaUsage from API response. */
export function parseQuotaUsage(data: Record<string, unknown>): QuotaUsage {
  return {
    tenant: data.tenant as string,
    namespace: data.namespace as string,
    used: data.used as number,
    limit: data.limit as number,
    remaining: data.remaining as number,
    window: data.window as string,
    resetsAt: data.resets_at as string,
    overageBehavior: data.overage_behavior as string,
  };
}

// =============================================================================
// Retention Policy Types
// =============================================================================

/** Request to create a retention policy. */
export interface CreateRetentionRequest {
  namespace: string;
  tenant: string;
  auditTtlSeconds: number;
  stateTtlSeconds: number;
  eventTtlSeconds: number;
  complianceHold?: boolean;
  description?: string;
  labels?: Record<string, string>;
}

/** Convert a CreateRetentionRequest to the API request format. */
export function createRetentionRequestToApi(req: CreateRetentionRequest): Record<string, unknown> {
  const result: Record<string, unknown> = {
    namespace: req.namespace,
    tenant: req.tenant,
    audit_ttl_seconds: req.auditTtlSeconds,
    state_ttl_seconds: req.stateTtlSeconds,
    event_ttl_seconds: req.eventTtlSeconds,
  };
  if (req.complianceHold !== undefined) result.compliance_hold = req.complianceHold;
  if (req.description !== undefined) result.description = req.description;
  if (req.labels !== undefined) result.labels = req.labels;
  return result;
}

/** Request to update a retention policy. */
export interface UpdateRetentionRequest {
  enabled?: boolean;
  auditTtlSeconds?: number;
  stateTtlSeconds?: number;
  eventTtlSeconds?: number;
  complianceHold?: boolean;
  description?: string;
  labels?: Record<string, string>;
}

/** Convert an UpdateRetentionRequest to the API request format. */
export function updateRetentionRequestToApi(req: UpdateRetentionRequest): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  if (req.enabled !== undefined) result.enabled = req.enabled;
  if (req.auditTtlSeconds !== undefined) result.audit_ttl_seconds = req.auditTtlSeconds;
  if (req.stateTtlSeconds !== undefined) result.state_ttl_seconds = req.stateTtlSeconds;
  if (req.eventTtlSeconds !== undefined) result.event_ttl_seconds = req.eventTtlSeconds;
  if (req.complianceHold !== undefined) result.compliance_hold = req.complianceHold;
  if (req.description !== undefined) result.description = req.description;
  if (req.labels !== undefined) result.labels = req.labels;
  return result;
}

/** A retention policy. */
export interface RetentionPolicy {
  id: string;
  namespace: string;
  tenant: string;
  enabled: boolean;
  auditTtlSeconds: number;
  stateTtlSeconds: number;
  eventTtlSeconds: number;
  complianceHold: boolean;
  createdAt: string;
  updatedAt: string;
  description?: string;
  labels?: Record<string, string>;
}

/** Parse a RetentionPolicy from API response. */
export function parseRetentionPolicy(data: Record<string, unknown>): RetentionPolicy {
  return {
    id: data.id as string,
    namespace: data.namespace as string,
    tenant: data.tenant as string,
    enabled: data.enabled as boolean,
    auditTtlSeconds: data.audit_ttl_seconds as number,
    stateTtlSeconds: data.state_ttl_seconds as number,
    eventTtlSeconds: data.event_ttl_seconds as number,
    complianceHold: data.compliance_hold as boolean,
    createdAt: data.created_at as string,
    updatedAt: data.updated_at as string,
    description: data.description as string | undefined,
    labels: data.labels as Record<string, string> | undefined,
  };
}

/** Response from listing retention policies. */
export interface ListRetentionResponse {
  policies: RetentionPolicy[];
  count: number;
}

/** Parse a ListRetentionResponse from API response. */
export function parseListRetentionResponse(data: Record<string, unknown>): ListRetentionResponse {
  const items = data.policies as Record<string, unknown>[];
  return {
    policies: items.map(parseRetentionPolicy),
    count: data.count as number,
  };
}

// =============================================================================
// Chain Types
// =============================================================================

/** Summary of a chain execution for list responses. */
export interface ChainSummary {
  /** Unique chain execution ID. */
  chainId: string;
  /** Name of the chain configuration. */
  chainName: string;
  /** Current status. */
  status: string;
  /** Current step index (0-based). */
  currentStep: number;
  /** Total number of steps. */
  totalSteps: number;
  /** When the chain started. */
  startedAt: string;
  /** When the chain was last updated. */
  updatedAt: string;
}

/** Parse a ChainSummary from API response. */
export function parseChainSummary(data: Record<string, unknown>): ChainSummary {
  return {
    chainId: data.chain_id as string,
    chainName: data.chain_name as string,
    status: data.status as string,
    currentStep: data.current_step as number,
    totalSteps: data.total_steps as number,
    startedAt: data.started_at as string,
    updatedAt: data.updated_at as string,
  };
}

/** Response for listing chain executions. */
export interface ListChainsResponse {
  /** List of chain execution summaries. */
  chains: ChainSummary[];
}

/** Parse a ListChainsResponse from API response. */
export function parseListChainsResponse(data: Record<string, unknown>): ListChainsResponse {
  const chains = data.chains as Record<string, unknown>[];
  return {
    chains: chains.map(parseChainSummary),
  };
}

/** Detailed status of a single chain step. */
export interface ChainStepStatus {
  /** Step name. */
  name: string;
  /** Provider used for this step. */
  provider: string;
  /** Step status: "pending", "completed", "failed", "skipped". */
  status: string;
  /** Response body from the provider (if completed). */
  responseBody?: unknown;
  /** Error message (if failed). */
  error?: string;
  /** When this step completed. */
  completedAt?: string;
}

/** Parse a ChainStepStatus from API response. */
export function parseChainStepStatus(data: Record<string, unknown>): ChainStepStatus {
  return {
    name: data.name as string,
    provider: data.provider as string,
    status: data.status as string,
    responseBody: data.response_body as unknown | undefined,
    error: data.error as string | undefined,
    completedAt: data.completed_at as string | undefined,
  };
}

/** Full detail response for a chain execution. */
export interface ChainDetailResponse {
  /** Unique chain execution ID. */
  chainId: string;
  /** Name of the chain configuration. */
  chainName: string;
  /** Current status. */
  status: string;
  /** Current step index (0-based). */
  currentStep: number;
  /** Total number of steps. */
  totalSteps: number;
  /** Per-step status details. */
  steps: ChainStepStatus[];
  /** When the chain started. */
  startedAt: string;
  /** When the chain was last updated. */
  updatedAt: string;
  /** When the chain will time out. */
  expiresAt?: string;
  /** Reason for cancellation (if cancelled). */
  cancelReason?: string;
  /** Who cancelled the chain (if cancelled). */
  cancelledBy?: string;
  /** The ordered list of step names that were executed (the branch path taken). */
  executionPath?: string[];
}

/** Parse a ChainDetailResponse from API response. */
export function parseChainDetailResponse(data: Record<string, unknown>): ChainDetailResponse {
  const steps = (data.steps as Record<string, unknown>[]) ?? [];
  const executionPath = data.execution_path as string[] | undefined;
  return {
    chainId: data.chain_id as string,
    chainName: data.chain_name as string,
    status: data.status as string,
    currentStep: data.current_step as number,
    totalSteps: data.total_steps as number,
    steps: steps.map(parseChainStepStatus),
    startedAt: data.started_at as string,
    updatedAt: data.updated_at as string,
    expiresAt: data.expires_at as string | undefined,
    cancelReason: data.cancel_reason as string | undefined,
    cancelledBy: data.cancelled_by as string | undefined,
    executionPath: executionPath && executionPath.length > 0 ? executionPath : undefined,
  };
}

// =============================================================================
// DLQ Types (Dead-Letter Queue)
// =============================================================================

/** Response for DLQ stats endpoint. */
export interface DlqStatsResponse {
  /** Whether the DLQ is enabled. */
  enabled: boolean;
  /** Number of entries in the DLQ. */
  count: number;
}

/** Parse a DlqStatsResponse from API response. */
export function parseDlqStatsResponse(data: Record<string, unknown>): DlqStatsResponse {
  return {
    enabled: data.enabled as boolean,
    count: data.count as number,
  };
}

/** A single dead-letter queue entry. */
export interface DlqEntry {
  /** The failed action's unique identifier. */
  actionId: string;
  /** Namespace the action belongs to. */
  namespace: string;
  /** Tenant that owns the action. */
  tenant: string;
  /** Target provider for the action. */
  provider: string;
  /** Action type discriminator. */
  actionType: string;
  /** Human-readable description of the final error. */
  error: string;
  /** Number of execution attempts made. */
  attempts: number;
  /** Unix timestamp (seconds) when the entry was created. */
  timestamp: number;
}

/** Parse a DlqEntry from API response. */
export function parseDlqEntry(data: Record<string, unknown>): DlqEntry {
  return {
    actionId: data.action_id as string,
    namespace: data.namespace as string,
    tenant: data.tenant as string,
    provider: data.provider as string,
    actionType: data.action_type as string,
    error: data.error as string,
    attempts: data.attempts as number,
    timestamp: data.timestamp as number,
  };
}

/** Response for DLQ drain endpoint. */
export interface DlqDrainResponse {
  /** Entries drained from the DLQ. */
  entries: DlqEntry[];
  /** Number of entries drained. */
  count: number;
}

/** Parse a DlqDrainResponse from API response. */
export function parseDlqDrainResponse(data: Record<string, unknown>): DlqDrainResponse {
  const entries = (data.entries as Record<string, unknown>[]) ?? [];
  return {
    entries: entries.map(parseDlqEntry),
    count: data.count as number,
  };
}

// =============================================================================
// SSE Event Types
// =============================================================================

/** Generic SSE event wrapper. */
export interface SseEvent {
  /** The SSE event type (e.g., "action_dispatched", "chain_step_completed"). */
  event: string;
  /** The SSE event ID. */
  id: string;
  /** The parsed JSON data payload. */
  data: unknown;
}

/** Options for the subscribe endpoint. */
export interface SubscribeOptions {
  /** Namespace for tenant isolation. */
  namespace?: string;
  /** Tenant for tenant isolation. */
  tenant?: string;
  /** Emit synthetic catch-up events for the entity's current state (default: true). */
  includeHistory?: boolean;
}

/** Options for the stream endpoint. */
export interface StreamOptions {
  /** Filter events by namespace. */
  namespace?: string;
  /** Filter events by action type. */
  actionType?: string;
  /** Filter events by outcome category (e.g., "executed", "suppressed", "failed"). */
  outcome?: string;
  /** Filter events by stream event type (e.g., "action_dispatched", "group_flushed"). */
  eventType?: string;
  /** Filter events by chain ID. */
  chainId?: string;
  /** Filter events by group ID. */
  groupId?: string;
  /** Filter events by action ID. */
  actionId?: string;
  /** Last event ID for reconnection catch-up. */
  lastEventId?: string;
}

// =============================================================================
// Provider Health Types
// =============================================================================

/** Health and metrics for a single provider. */
export interface ProviderHealthStatus {
  /** Provider name. */
  provider: string;
  /** Whether the provider is healthy (circuit breaker closed). */
  healthy: boolean;
  /** Error message from last health check (if any). */
  healthCheckError?: string;
  /** Current circuit breaker state (closed, open, half_open). */
  circuitBreakerState: string;
  /** Total number of requests to this provider. */
  totalRequests: number;
  /** Number of successful requests. */
  successes: number;
  /** Number of failed requests. */
  failures: number;
  /** Success rate as percentage (0-100). */
  successRate: number;
  /** Average request latency in milliseconds. */
  avgLatencyMs: number;
  /** 50th percentile latency in milliseconds. */
  p50LatencyMs: number;
  /** 95th percentile latency in milliseconds. */
  p95LatencyMs: number;
  /** 99th percentile latency in milliseconds. */
  p99LatencyMs: number;
  /** Timestamp of last request (milliseconds since epoch). */
  lastRequestAt?: number;
  /** Last error message (if any). */
  lastError?: string;
}

export function parseProviderHealthStatus(data: Record<string, unknown>): ProviderHealthStatus {
  return {
    provider: data.provider as string,
    healthy: data.healthy as boolean,
    healthCheckError: data.health_check_error as string | undefined,
    circuitBreakerState: data.circuit_breaker_state as string,
    totalRequests: data.total_requests as number,
    successes: data.successes as number,
    failures: data.failures as number,
    successRate: data.success_rate as number,
    avgLatencyMs: data.avg_latency_ms as number,
    p50LatencyMs: data.p50_latency_ms as number,
    p95LatencyMs: data.p95_latency_ms as number,
    p99LatencyMs: data.p99_latency_ms as number,
    lastRequestAt: data.last_request_at as number | undefined,
    lastError: data.last_error as string | undefined,
  };
}

/** Response from listing provider health. */
export interface ListProviderHealthResponse {
  providers: ProviderHealthStatus[];
}

export function parseListProviderHealthResponse(data: Record<string, unknown>): ListProviderHealthResponse {
  const items = data.providers as Record<string, unknown>[];
  return {
    providers: items.map(parseProviderHealthStatus),
  };
}
