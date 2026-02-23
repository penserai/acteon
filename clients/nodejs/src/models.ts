/**
 * Data models for the Acteon client.
 */

import { randomUUID } from "crypto";

/**
 * An attachment with explicit metadata and base64-encoded data.
 */
export interface Attachment {
  /** User-set identifier for referencing in chains. */
  id: string;
  /** Human-readable display name. */
  name: string;
  /** Filename with extension. */
  filename: string;
  /** MIME content type (e.g., "application/pdf"). */
  contentType: string;
  /** Base64-encoded file content. */
  dataBase64: string;
}

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
  /** Optional template name for payload rendering. */
  template?: string;
  /** Optional attachments. */
  attachments?: Attachment[];
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
    template?: string;
    attachments?: Attachment[];
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
    template: options?.template,
    attachments: options?.attachments,
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
  if (action.template) {
    result.template = action.template;
  }
  if (action.attachments && action.attachments.length > 0) {
    result.attachments = action.attachments.map(a => ({
      id: a.id,
      name: a.name,
      filename: a.filename,
      content_type: a.contentType,
      data_base64: a.dataBase64,
    }));
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
  /** SHA-256 hex digest of the canonicalized record content (compliance mode). */
  recordHash?: string;
  /** Hash of the previous record in the chain (compliance mode). */
  previousHash?: string;
  /** Monotonic sequence number within the (namespace, tenant) pair (compliance mode). */
  sequenceNumber?: number;
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
    recordHash: data.record_hash as string | undefined,
    previousHash: data.previous_hash as string | undefined,
    sequenceNumber: data.sequence_number as number | undefined,
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
  /** Parent chain ID if this is a sub-chain. */
  parentChainId?: string;
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
    parentChainId: data.parent_chain_id as string | undefined,
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
  /** Name of the sub-chain this step triggers, if any. */
  subChain?: string;
  /** ID of the child chain instance spawned by this step, if any. */
  childChainId?: string;
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
    subChain: data.sub_chain as string | undefined,
    childChainId: data.child_chain_id as string | undefined,
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
  /** Parent chain ID if this is a sub-chain. */
  parentChainId?: string;
  /** IDs of child chains spawned by sub-chain steps. */
  childChainIds?: string[];
}

/** Parse a ChainDetailResponse from API response. */
export function parseChainDetailResponse(data: Record<string, unknown>): ChainDetailResponse {
  const steps = (data.steps as Record<string, unknown>[]) ?? [];
  const executionPath = data.execution_path as string[] | undefined;
  const childChainIds = data.child_chain_ids as string[] | undefined;
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
    parentChainId: data.parent_chain_id as string | undefined,
    childChainIds: childChainIds && childChainIds.length > 0 ? childChainIds : undefined,
  };
}

// =============================================================================
// DAG Types (Chain Visualization)
// =============================================================================

/** A node in the chain DAG. */
export interface DagNode {
  /** Node name (step name or sub-chain name). */
  name: string;
  /** Node type: "step" or "sub_chain". */
  nodeType: string;
  /** Provider for this step, if applicable. */
  provider?: string;
  /** Action type for this step, if applicable. */
  actionType?: string;
  /** Name of the sub-chain, if this is a sub-chain node. */
  subChainName?: string;
  /** Current status of this node (for instance DAGs). */
  status?: string;
  /** ID of the child chain instance (for instance DAGs). */
  childChainId?: string;
  /** Nested DAG for sub-chain expansion. */
  children?: DagResponse;
}

/** An edge in the chain DAG. */
export interface DagEdge {
  /** Source node name. */
  source: string;
  /** Target node name. */
  target: string;
  /** Edge label (e.g., branch condition). */
  label?: string;
  /** Whether this edge is on the execution path. */
  onExecutionPath: boolean;
}

/** DAG representation of a chain (config or instance). */
export interface DagResponse {
  /** Chain configuration name. */
  chainName: string;
  /** Chain instance ID (only for instance DAGs). */
  chainId?: string;
  /** Chain status (only for instance DAGs). */
  status?: string;
  /** Nodes in the DAG. */
  nodes: DagNode[];
  /** Edges connecting the nodes. */
  edges: DagEdge[];
  /** Ordered list of step names on the execution path. */
  executionPath: string[];
}

/** Parse a DagNode from API response. */
export function parseDagNode(data: Record<string, unknown>): DagNode {
  const children = data.children as Record<string, unknown> | undefined;
  return {
    name: data.name as string,
    nodeType: data.node_type as string,
    provider: data.provider as string | undefined,
    actionType: data.action_type as string | undefined,
    subChainName: data.sub_chain_name as string | undefined,
    status: data.status as string | undefined,
    childChainId: data.child_chain_id as string | undefined,
    children: children ? parseDagResponse(children) : undefined,
  };
}

/** Parse a DagEdge from API response. */
export function parseDagEdge(data: Record<string, unknown>): DagEdge {
  return {
    source: data.source as string,
    target: data.target as string,
    label: data.label as string | undefined,
    onExecutionPath: (data.on_execution_path as boolean) ?? false,
  };
}

/** Parse a DagResponse from API response. */
export function parseDagResponse(data: Record<string, unknown>): DagResponse {
  const nodes = (data.nodes as Record<string, unknown>[]) ?? [];
  const edges = (data.edges as Record<string, unknown>[]) ?? [];
  return {
    chainName: data.chain_name as string,
    chainId: data.chain_id as string | undefined,
    status: data.status as string | undefined,
    nodes: nodes.map(parseDagNode),
    edges: edges.map(parseDagEdge),
    executionPath: (data.execution_path as string[]) ?? [],
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

// =============================================================================
// WASM Plugin Types
// =============================================================================

/** Configuration for a WASM plugin. */
export interface WasmPluginConfig {
  /** Maximum memory in bytes the plugin can use. */
  memoryLimitBytes?: number;
  /** Maximum execution time in milliseconds. */
  timeoutMs?: number;
  /** List of host functions the plugin is allowed to call. */
  allowedHostFunctions?: string[];
}

/** Parse a WasmPluginConfig from API response. */
export function parseWasmPluginConfig(data: Record<string, unknown>): WasmPluginConfig {
  return {
    memoryLimitBytes: data.memory_limit_bytes as number | undefined,
    timeoutMs: data.timeout_ms as number | undefined,
    allowedHostFunctions: data.allowed_host_functions as string[] | undefined,
  };
}

/** A registered WASM plugin. */
export interface WasmPlugin {
  /** Plugin name (unique identifier). */
  name: string;
  /** Optional human-readable description. */
  description?: string;
  /** Plugin status (e.g., "active", "disabled"). */
  status: string;
  /** Whether the plugin is enabled. */
  enabled: boolean;
  /** Plugin resource configuration. */
  config?: WasmPluginConfig;
  /** When the plugin was registered. */
  createdAt: string;
  /** When the plugin was last updated. */
  updatedAt: string;
  /** Number of times the plugin has been invoked. */
  invocationCount: number;
}

/** Parse a WasmPlugin from API response. */
export function parseWasmPlugin(data: Record<string, unknown>): WasmPlugin {
  const configData = data.config as Record<string, unknown> | undefined;
  return {
    name: data.name as string,
    description: data.description as string | undefined,
    status: data.status as string,
    enabled: (data.enabled as boolean) ?? true,
    config: configData ? parseWasmPluginConfig(configData) : undefined,
    createdAt: data.created_at as string,
    updatedAt: data.updated_at as string,
    invocationCount: (data.invocation_count as number) ?? 0,
  };
}

/** Request to register a new WASM plugin. */
export interface RegisterPluginRequest {
  /** Plugin name (unique identifier). */
  name: string;
  /** Optional human-readable description. */
  description?: string;
  /** Base64-encoded WASM module bytes. */
  wasmBytes?: string;
  /** Path to the WASM file (server-side). */
  wasmPath?: string;
  /** Plugin resource configuration. */
  config?: WasmPluginConfig;
}

/** Convert a RegisterPluginRequest to the API request format. */
export function registerPluginRequestToApi(req: RegisterPluginRequest): Record<string, unknown> {
  const result: Record<string, unknown> = { name: req.name };
  if (req.description !== undefined) result.description = req.description;
  if (req.wasmBytes !== undefined) result.wasm_bytes = req.wasmBytes;
  if (req.wasmPath !== undefined) result.wasm_path = req.wasmPath;
  if (req.config !== undefined) {
    const config: Record<string, unknown> = {};
    if (req.config.memoryLimitBytes !== undefined) config.memory_limit_bytes = req.config.memoryLimitBytes;
    if (req.config.timeoutMs !== undefined) config.timeout_ms = req.config.timeoutMs;
    if (req.config.allowedHostFunctions !== undefined) config.allowed_host_functions = req.config.allowedHostFunctions;
    result.config = config;
  }
  return result;
}

/** Response from listing WASM plugins. */
export interface ListPluginsResponse {
  plugins: WasmPlugin[];
  count: number;
}

/** Parse a ListPluginsResponse from API response. */
export function parseListPluginsResponse(data: Record<string, unknown>): ListPluginsResponse {
  const items = data.plugins as Record<string, unknown>[];
  return {
    plugins: items.map(parseWasmPlugin),
    count: data.count as number,
  };
}

/** Request to test-invoke a WASM plugin. */
export interface PluginInvocationRequest {
  /** JSON input to pass to the plugin. */
  input: Record<string, unknown>;
  /** The function to invoke (default: "evaluate"). */
  function?: string;
}

/** Response from test-invoking a WASM plugin. */
export interface PluginInvocationResponse {
  /** Whether the plugin evaluation returned true or false. */
  verdict: boolean;
  /** Optional message from the plugin. */
  message?: string;
  /** Optional structured metadata from the plugin. */
  metadata?: Record<string, unknown>;
  /** Execution time in milliseconds. */
  durationMs?: number;
}

/** Parse a PluginInvocationResponse from API response. */
export function parsePluginInvocationResponse(data: Record<string, unknown>): PluginInvocationResponse {
  return {
    verdict: data.verdict as boolean,
    message: data.message as string | undefined,
    metadata: data.metadata as Record<string, unknown> | undefined,
    durationMs: data.duration_ms as number | undefined,
  };
}

// =============================================================================
// Compliance Types (SOC2/HIPAA)
// =============================================================================

/** Compliance mode: "none", "soc2", or "hipaa". */
export type ComplianceMode = "none" | "soc2" | "hipaa";

/** Current compliance configuration status. */
export interface ComplianceStatus {
  mode: ComplianceMode;
  syncAuditWrites: boolean;
  immutableAudit: boolean;
  hashChain: boolean;
}

/** Parse a ComplianceStatus from API response. */
export function parseComplianceStatus(data: Record<string, unknown>): ComplianceStatus {
  return {
    mode: data.mode as ComplianceMode,
    syncAuditWrites: data.sync_audit_writes as boolean,
    immutableAudit: data.immutable_audit as boolean,
    hashChain: data.hash_chain as boolean,
  };
}

/** Result of verifying the integrity of an audit hash chain. */
export interface HashChainVerification {
  valid: boolean;
  recordsChecked: number;
  firstBrokenAt?: string;
  firstRecordId?: string;
  lastRecordId?: string;
}

/** Parse a HashChainVerification from API response. */
export function parseHashChainVerification(data: Record<string, unknown>): HashChainVerification {
  return {
    valid: data.valid as boolean,
    recordsChecked: data.records_checked as number,
    firstBrokenAt: data.first_broken_at as string | undefined,
    firstRecordId: data.first_record_id as string | undefined,
    lastRecordId: data.last_record_id as string | undefined,
  };
}

/** Request body for hash chain verification. */
export interface VerifyHashChainRequest {
  namespace: string;
  tenant: string;
  from?: string;
  to?: string;
}

// =============================================================================
// Payload Template Types
// =============================================================================

/** A payload template. */
export interface TemplateInfo {
  id: string;
  name: string;
  namespace: string;
  tenant: string;
  content: string;
  createdAt: string;
  updatedAt: string;
  description?: string;
  labels?: Record<string, string>;
}

/** Parse a TemplateInfo from API response. */
export function parseTemplateInfo(data: Record<string, unknown>): TemplateInfo {
  return {
    id: data.id as string,
    name: data.name as string,
    namespace: data.namespace as string,
    tenant: data.tenant as string,
    content: data.content as string,
    createdAt: data.created_at as string,
    updatedAt: data.updated_at as string,
    description: data.description as string | undefined,
    labels: data.labels as Record<string, string> | undefined,
  };
}

/** Request to create a payload template. */
export interface CreateTemplateRequest {
  name: string;
  namespace: string;
  tenant: string;
  content: string;
  description?: string;
  labels?: Record<string, string>;
}

/** Convert a CreateTemplateRequest to the API request format. */
export function createTemplateRequestToApi(req: CreateTemplateRequest): Record<string, unknown> {
  const result: Record<string, unknown> = {
    name: req.name,
    namespace: req.namespace,
    tenant: req.tenant,
    content: req.content,
  };
  if (req.description !== undefined) result.description = req.description;
  if (req.labels !== undefined) result.labels = req.labels;
  return result;
}

/** Request to update a payload template. */
export interface UpdateTemplateRequest {
  content?: string;
  description?: string;
  labels?: Record<string, string>;
}

/** Convert an UpdateTemplateRequest to the API request format. */
export function updateTemplateRequestToApi(req: UpdateTemplateRequest): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  if (req.content !== undefined) result.content = req.content;
  if (req.description !== undefined) result.description = req.description;
  if (req.labels !== undefined) result.labels = req.labels;
  return result;
}

/** Response from listing templates. */
export interface ListTemplatesResponse {
  templates: TemplateInfo[];
  count: number;
}

/** Parse a ListTemplatesResponse from API response. */
export function parseListTemplatesResponse(data: Record<string, unknown>): ListTemplatesResponse {
  const items = data.templates as Record<string, unknown>[];
  return {
    templates: items.map(parseTemplateInfo),
    count: data.count as number,
  };
}

/**
 * A field in a template profile.
 * Either an inline string value or a reference to a template via $ref.
 */
export type TemplateProfileField = string | { $ref: string };

/** A template profile that groups multiple templates. */
export interface TemplateProfileInfo {
  id: string;
  name: string;
  namespace: string;
  tenant: string;
  fields: Record<string, TemplateProfileField>;
  createdAt: string;
  updatedAt: string;
  description?: string;
  labels?: Record<string, string>;
}

/** Parse a TemplateProfileInfo from API response. */
export function parseTemplateProfileInfo(data: Record<string, unknown>): TemplateProfileInfo {
  return {
    id: data.id as string,
    name: data.name as string,
    namespace: data.namespace as string,
    tenant: data.tenant as string,
    fields: (data.fields as Record<string, TemplateProfileField>) ?? {},
    createdAt: data.created_at as string,
    updatedAt: data.updated_at as string,
    description: data.description as string | undefined,
    labels: data.labels as Record<string, string> | undefined,
  };
}

/** Request to create a template profile. */
export interface CreateProfileRequest {
  name: string;
  namespace: string;
  tenant: string;
  fields: Record<string, TemplateProfileField>;
  description?: string;
  labels?: Record<string, string>;
}

/** Convert a CreateProfileRequest to the API request format. */
export function createProfileRequestToApi(req: CreateProfileRequest): Record<string, unknown> {
  const result: Record<string, unknown> = {
    name: req.name,
    namespace: req.namespace,
    tenant: req.tenant,
    fields: req.fields,
  };
  if (req.description !== undefined) result.description = req.description;
  if (req.labels !== undefined) result.labels = req.labels;
  return result;
}

/** Request to update a template profile. */
export interface UpdateProfileRequest {
  fields?: Record<string, TemplateProfileField>;
  description?: string;
  labels?: Record<string, string>;
}

/** Convert an UpdateProfileRequest to the API request format. */
export function updateProfileRequestToApi(req: UpdateProfileRequest): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  if (req.fields !== undefined) result.fields = req.fields;
  if (req.description !== undefined) result.description = req.description;
  if (req.labels !== undefined) result.labels = req.labels;
  return result;
}

/** Response from listing template profiles. */
export interface ListProfilesResponse {
  profiles: TemplateProfileInfo[];
  count: number;
}

/** Parse a ListProfilesResponse from API response. */
export function parseListProfilesResponse(data: Record<string, unknown>): ListProfilesResponse {
  const items = data.profiles as Record<string, unknown>[];
  return {
    profiles: items.map(parseTemplateProfileInfo),
    count: data.count as number,
  };
}

/** Request to render a template profile with payload data. */
export interface RenderPreviewRequest {
  profile: string;
  namespace: string;
  tenant: string;
  payload: Record<string, unknown>;
}

/** Response from rendering a template profile. */
export interface RenderPreviewResponse {
  rendered: Record<string, string>;
}

/** Parse a RenderPreviewResponse from API response. */
export function parseRenderPreviewResponse(data: Record<string, unknown>): RenderPreviewResponse {
  return {
    rendered: (data.rendered as Record<string, string>) ?? {},
  };
}

// =============================================================================
// Analytics Types
// =============================================================================

/** Query parameters for the analytics endpoint. */
export interface AnalyticsQuery {
  /** The metric to query (required). */
  metric: "volume" | "outcome_breakdown" | "top_action_types" | "latency" | "error_rate";
  /** Optional namespace filter. */
  namespace?: string;
  /** Optional tenant filter. */
  tenant?: string;
  /** Optional provider filter. */
  provider?: string;
  /** Optional action type filter. */
  actionType?: string;
  /** Optional outcome filter. */
  outcome?: string;
  /** Time bucket interval (default "daily"). */
  interval?: "hourly" | "daily" | "weekly" | "monthly";
  /** Optional start of time range (RFC 3339 datetime string). */
  from?: string;
  /** Optional end of time range (RFC 3339 datetime string). */
  to?: string;
  /** Optional grouping dimension (e.g., "provider", "action_type", "outcome"). */
  groupBy?: string;
  /** Optional limit for top-N queries. */
  topN?: number;
}

/** A single time bucket in an analytics response. */
export interface AnalyticsBucket {
  /** ISO 8601 timestamp for the bucket start. */
  timestamp: string;
  /** Number of actions in this bucket. */
  count: number;
  /** Optional group label when group_by is used. */
  group?: string | null;
  /** Average action duration in milliseconds. */
  avgDurationMs?: number | null;
  /** 50th percentile duration in milliseconds. */
  p50DurationMs?: number | null;
  /** 95th percentile duration in milliseconds. */
  p95DurationMs?: number | null;
  /** 99th percentile duration in milliseconds. */
  p99DurationMs?: number | null;
  /** Fraction of actions that failed (0.0 to 1.0). */
  errorRate?: number | null;
}

/** A single entry in a top-N analytics result. */
export interface AnalyticsTopEntry {
  /** The label for this entry (e.g., action type name). */
  label: string;
  /** Number of occurrences. */
  count: number;
  /** Percentage of total. */
  percentage: number;
}

/** Response from the analytics endpoint. */
export interface AnalyticsResponse {
  /** The metric that was queried. */
  metric: string;
  /** The time interval used for bucketing. */
  interval: string;
  /** Start of the queried time range. */
  from: string;
  /** End of the queried time range. */
  to: string;
  /** List of time-series data buckets. */
  buckets: AnalyticsBucket[];
  /** List of top-N entries (for top_action_types metric). */
  topEntries: AnalyticsTopEntry[];
  /** Total count across all buckets. */
  totalCount: number;
}

/** Convert AnalyticsQuery to URL search params. */
export function analyticsQueryToParams(query: AnalyticsQuery): URLSearchParams {
  const params = new URLSearchParams();
  params.set("metric", query.metric);
  if (query.namespace) params.set("namespace", query.namespace);
  if (query.tenant) params.set("tenant", query.tenant);
  if (query.provider) params.set("provider", query.provider);
  if (query.actionType) params.set("action_type", query.actionType);
  if (query.outcome) params.set("outcome", query.outcome);
  if (query.interval) params.set("interval", query.interval);
  if (query.from) params.set("from", query.from);
  if (query.to) params.set("to", query.to);
  if (query.groupBy) params.set("group_by", query.groupBy);
  if (query.topN !== undefined) params.set("top_n", query.topN.toString());
  return params;
}

/** Parse an AnalyticsBucket from a raw JSON object. */
export function parseAnalyticsBucket(data: Record<string, unknown>): AnalyticsBucket {
  return {
    timestamp: data.timestamp as string,
    count: data.count as number,
    group: (data.group as string | null) ?? null,
    avgDurationMs: (data.avg_duration_ms as number | null) ?? null,
    p50DurationMs: (data.p50_duration_ms as number | null) ?? null,
    p95DurationMs: (data.p95_duration_ms as number | null) ?? null,
    p99DurationMs: (data.p99_duration_ms as number | null) ?? null,
    errorRate: (data.error_rate as number | null) ?? null,
  };
}

/** Parse an AnalyticsTopEntry from a raw JSON object. */
export function parseAnalyticsTopEntry(data: Record<string, unknown>): AnalyticsTopEntry {
  return {
    label: data.label as string,
    count: data.count as number,
    percentage: data.percentage as number,
  };
}

/** Parse an AnalyticsResponse from a raw JSON object. */
export function parseAnalyticsResponse(data: Record<string, unknown>): AnalyticsResponse {
  const buckets = (data.buckets as Record<string, unknown>[]) ?? [];
  const topEntries = (data.top_entries as Record<string, unknown>[]) ?? [];
  return {
    metric: data.metric as string,
    interval: data.interval as string,
    from: data.from as string,
    to: data.to as string,
    buckets: buckets.map(parseAnalyticsBucket),
    topEntries: topEntries.map(parseAnalyticsTopEntry),
    totalCount: data.total_count as number,
  };
}

// =============================================================================
// Provider Payload Helpers
// =============================================================================

/** Payload for the Twilio SMS provider. */
export interface TwilioSmsPayload {
  to: string;
  body: string;
  from?: string;
  media_url?: string;
}

/** Create a payload for the Twilio SMS provider. */
export function createTwilioSmsPayload(
  to: string,
  body: string,
  options?: { from?: string; mediaUrl?: string }
): Record<string, unknown> {
  const payload: Record<string, unknown> = { to, body };
  if (options?.from) payload.from = options.from;
  if (options?.mediaUrl) payload.media_url = options.mediaUrl;
  return payload;
}

/** Payload for the Microsoft Teams provider. */
export interface TeamsMessagePayload {
  text?: string;
  title?: string;
  theme_color?: string;
  summary?: string;
  adaptive_card?: Record<string, unknown>;
}

/** Create a payload for the Teams provider (MessageCard). */
export function createTeamsMessagePayload(
  text: string,
  options?: { title?: string; themeColor?: string; summary?: string }
): Record<string, unknown> {
  const payload: Record<string, unknown> = { text };
  if (options?.title) payload.title = options.title;
  if (options?.themeColor) payload.theme_color = options.themeColor;
  if (options?.summary) payload.summary = options.summary;
  return payload;
}

/** Create a payload for the Teams provider (Adaptive Card). */
export function createTeamsAdaptiveCardPayload(
  card: Record<string, unknown>
): Record<string, unknown> {
  return { adaptive_card: card };
}

/** Payload for the Discord webhook provider. */
export interface DiscordMessagePayload {
  content?: string;
  embeds?: DiscordEmbed[];
  username?: string;
  avatar_url?: string;
  tts?: boolean;
}

/** A Discord embed object. */
export interface DiscordEmbed {
  title?: string;
  description?: string;
  color?: number;
  fields?: Array<{ name: string; value: string; inline?: boolean }>;
  footer?: { text: string };
  timestamp?: string;
}

/** Create a payload for the Discord webhook provider. */
export function createDiscordMessagePayload(
  options: {
    content?: string;
    embeds?: DiscordEmbed[];
    username?: string;
    avatarUrl?: string;
    tts?: boolean;
  }
): Record<string, unknown> {
  const payload: Record<string, unknown> = {};
  if (options.content !== undefined) payload.content = options.content;
  if (options.embeds !== undefined) payload.embeds = options.embeds;
  if (options.username !== undefined) payload.username = options.username;
  if (options.avatarUrl !== undefined) payload.avatar_url = options.avatarUrl;
  if (options.tts !== undefined) payload.tts = options.tts;
  return payload;
}

// =============================================================================
// AWS Provider Payload Helpers
// =============================================================================

/** Payload for the AWS SNS publish action. */
export interface SnsPublishPayload {
  message: string;
  subject?: string;
  topic_arn?: string;
  message_group_id?: string;
  message_dedup_id?: string;
}

/** Create a payload for the AWS SNS provider. */
export function createSnsPublishPayload(
  message: string,
  options?: {
    subject?: string;
    topicArn?: string;
    messageGroupId?: string;
    messageDedupId?: string;
  }
): Record<string, unknown> {
  const payload: Record<string, unknown> = { message };
  if (options?.subject) payload.subject = options.subject;
  if (options?.topicArn) payload.topic_arn = options.topicArn;
  if (options?.messageGroupId)
    payload.message_group_id = options.messageGroupId;
  if (options?.messageDedupId)
    payload.message_dedup_id = options.messageDedupId;
  return payload;
}

/** Payload for the AWS Lambda invoke action. */
export interface LambdaInvokePayload {
  payload?: unknown;
  function_name?: string;
  invocation_type?: "RequestResponse" | "Event" | "DryRun";
}

/** Create a payload for the AWS Lambda provider. */
export function createLambdaInvokePayload(
  payloadData?: unknown,
  options?: {
    functionName?: string;
    invocationType?: "RequestResponse" | "Event" | "DryRun";
  }
): Record<string, unknown> {
  const payload: Record<string, unknown> = {};
  if (payloadData !== undefined) payload.payload = payloadData;
  if (options?.functionName) payload.function_name = options.functionName;
  if (options?.invocationType)
    payload.invocation_type = options.invocationType;
  return payload;
}

/** Payload for the AWS EventBridge put-event action. */
export interface EventBridgePutEventPayload {
  source: string;
  detail_type: string;
  detail: unknown;
  event_bus_name?: string;
  resources?: string[];
}

/** Create a payload for the AWS EventBridge provider. */
export function createEventBridgePutEventPayload(
  source: string,
  detailType: string,
  detail: unknown,
  options?: { eventBusName?: string; resources?: string[] }
): Record<string, unknown> {
  const payload: Record<string, unknown> = {
    source,
    detail_type: detailType,
    detail,
  };
  if (options?.eventBusName) payload.event_bus_name = options.eventBusName;
  if (options?.resources) payload.resources = options.resources;
  return payload;
}

/** Payload for the AWS SQS send-message action. */
export interface SqsSendMessagePayload {
  message_body: string;
  queue_url?: string;
  delay_seconds?: number;
  message_group_id?: string;
  message_dedup_id?: string;
  message_attributes?: Record<string, string>;
}

/** Create a payload for the AWS SQS provider. */
export function createSqsSendMessagePayload(
  messageBody: string,
  options?: {
    queueUrl?: string;
    delaySeconds?: number;
    messageGroupId?: string;
    messageDedupId?: string;
    messageAttributes?: Record<string, string>;
  }
): Record<string, unknown> {
  const payload: Record<string, unknown> = { message_body: messageBody };
  if (options?.queueUrl) payload.queue_url = options.queueUrl;
  if (options?.delaySeconds !== undefined)
    payload.delay_seconds = options.delaySeconds;
  if (options?.messageGroupId)
    payload.message_group_id = options.messageGroupId;
  if (options?.messageDedupId)
    payload.message_dedup_id = options.messageDedupId;
  if (options?.messageAttributes)
    payload.message_attributes = options.messageAttributes;
  return payload;
}

/** Payload for the AWS S3 put-object action. */
export interface S3PutObjectPayload {
  key: string;
  bucket?: string;
  body?: string;
  body_base64?: string;
  content_type?: string;
  metadata?: Record<string, string>;
}

/** Create a payload for the AWS S3 put-object action. */
export function createS3PutObjectPayload(
  key: string,
  options?: {
    bucket?: string;
    body?: string;
    bodyBase64?: string;
    contentType?: string;
    metadata?: Record<string, string>;
  }
): Record<string, unknown> {
  const payload: Record<string, unknown> = { key };
  if (options?.bucket) payload.bucket = options.bucket;
  if (options?.body !== undefined) payload.body = options.body;
  if (options?.bodyBase64) payload.body_base64 = options.bodyBase64;
  if (options?.contentType) payload.content_type = options.contentType;
  if (options?.metadata) payload.metadata = options.metadata;
  return payload;
}

/** Payload for the AWS S3 get-object action. */
export interface S3GetObjectPayload {
  key: string;
  bucket?: string;
}

/** Create a payload for the AWS S3 get-object action. */
export function createS3GetObjectPayload(
  key: string,
  options?: { bucket?: string }
): Record<string, unknown> {
  const payload: Record<string, unknown> = { key };
  if (options?.bucket) payload.bucket = options.bucket;
  return payload;
}

/** Payload for the AWS S3 delete-object action. */
export interface S3DeleteObjectPayload {
  key: string;
  bucket?: string;
}

/** Create a payload for the AWS S3 delete-object action. */
export function createS3DeleteObjectPayload(
  key: string,
  options?: { bucket?: string }
): Record<string, unknown> {
  const payload: Record<string, unknown> = { key };
  if (options?.bucket) payload.bucket = options.bucket;
  return payload;
}

// =============================================================================
// AWS EC2 Provider Payload Helpers
// =============================================================================

/** Payload for the AWS EC2 start-instances action. */
export interface Ec2StartInstancesPayload {
  instance_ids: string[];
}

/** Create a payload for the AWS EC2 start-instances action. */
export function createEc2StartInstancesPayload(
  instanceIds: string[]
): Record<string, unknown> {
  return { instance_ids: instanceIds };
}

/** Payload for the AWS EC2 stop-instances action. */
export interface Ec2StopInstancesPayload {
  instance_ids: string[];
  hibernate?: boolean;
  force?: boolean;
}

/** Create a payload for the AWS EC2 stop-instances action. */
export function createEc2StopInstancesPayload(
  instanceIds: string[],
  options?: { hibernate?: boolean; force?: boolean }
): Record<string, unknown> {
  const payload: Record<string, unknown> = { instance_ids: instanceIds };
  if (options?.hibernate !== undefined) payload.hibernate = options.hibernate;
  if (options?.force !== undefined) payload.force = options.force;
  return payload;
}

/** Payload for the AWS EC2 reboot-instances action. */
export interface Ec2RebootInstancesPayload {
  instance_ids: string[];
}

/** Create a payload for the AWS EC2 reboot-instances action. */
export function createEc2RebootInstancesPayload(
  instanceIds: string[]
): Record<string, unknown> {
  return { instance_ids: instanceIds };
}

/** Payload for the AWS EC2 terminate-instances action. */
export interface Ec2TerminateInstancesPayload {
  instance_ids: string[];
}

/** Create a payload for the AWS EC2 terminate-instances action. */
export function createEc2TerminateInstancesPayload(
  instanceIds: string[]
): Record<string, unknown> {
  return { instance_ids: instanceIds };
}

/** Create a payload for the AWS EC2 hibernate-instances action. */
export function createEc2HibernateInstancesPayload(
  instanceIds: string[]
): Record<string, unknown> {
  return { instance_ids: instanceIds };
}

/** Payload for the AWS EC2 run-instances action. */
export interface Ec2RunInstancesPayload {
  image_id: string;
  instance_type: string;
  min_count?: number;
  max_count?: number;
  key_name?: string;
  security_group_ids?: string[];
  subnet_id?: string;
  user_data?: string;
  tags?: Record<string, string>;
  iam_instance_profile?: string;
}

/** Create a payload for the AWS EC2 run-instances action. */
export function createEc2RunInstancesPayload(
  imageId: string,
  instanceType: string,
  options?: {
    minCount?: number;
    maxCount?: number;
    keyName?: string;
    securityGroupIds?: string[];
    subnetId?: string;
    userData?: string;
    tags?: Record<string, string>;
    iamInstanceProfile?: string;
  }
): Record<string, unknown> {
  const payload: Record<string, unknown> = {
    image_id: imageId,
    instance_type: instanceType,
  };
  if (options?.minCount !== undefined) payload.min_count = options.minCount;
  if (options?.maxCount !== undefined) payload.max_count = options.maxCount;
  if (options?.keyName !== undefined) payload.key_name = options.keyName;
  if (options?.securityGroupIds !== undefined)
    payload.security_group_ids = options.securityGroupIds;
  if (options?.subnetId !== undefined) payload.subnet_id = options.subnetId;
  if (options?.userData !== undefined) payload.user_data = options.userData;
  if (options?.tags !== undefined) payload.tags = options.tags;
  if (options?.iamInstanceProfile !== undefined)
    payload.iam_instance_profile = options.iamInstanceProfile;
  return payload;
}

/** Payload for the AWS EC2 attach-volume action. */
export interface Ec2AttachVolumePayload {
  volume_id: string;
  instance_id: string;
  device: string;
}

/** Create a payload for the AWS EC2 attach-volume action. */
export function createEc2AttachVolumePayload(
  volumeId: string,
  instanceId: string,
  device: string
): Record<string, unknown> {
  return {
    volume_id: volumeId,
    instance_id: instanceId,
    device,
  };
}

/** Payload for the AWS EC2 detach-volume action. */
export interface Ec2DetachVolumePayload {
  volume_id: string;
  instance_id?: string;
  device?: string;
  force?: boolean;
}

/** Create a payload for the AWS EC2 detach-volume action. */
export function createEc2DetachVolumePayload(
  volumeId: string,
  options?: { instanceId?: string; device?: string; force?: boolean }
): Record<string, unknown> {
  const payload: Record<string, unknown> = { volume_id: volumeId };
  if (options?.instanceId !== undefined)
    payload.instance_id = options.instanceId;
  if (options?.device !== undefined) payload.device = options.device;
  if (options?.force !== undefined) payload.force = options.force;
  return payload;
}

/** Payload for the AWS EC2 describe-instances action. */
export interface Ec2DescribeInstancesPayload {
  instance_ids?: string[];
}

/** Create a payload for the AWS EC2 describe-instances action. */
export function createEc2DescribeInstancesPayload(
  options?: { instanceIds?: string[] }
): Record<string, unknown> {
  const payload: Record<string, unknown> = {};
  if (options?.instanceIds !== undefined)
    payload.instance_ids = options.instanceIds;
  return payload;
}

// =============================================================================
// AWS Auto Scaling Provider Payload Helpers
// =============================================================================

/** Payload for the AWS Auto Scaling describe-groups action. */
export interface AsgDescribeGroupsPayload {
  auto_scaling_group_names?: string[];
}

/** Create a payload for the AWS Auto Scaling describe-groups action. */
export function createAsgDescribeGroupsPayload(
  options?: { groupNames?: string[] }
): Record<string, unknown> {
  const payload: Record<string, unknown> = {};
  if (options?.groupNames !== undefined)
    payload.auto_scaling_group_names = options.groupNames;
  return payload;
}

/** Payload for the AWS Auto Scaling set-desired-capacity action. */
export interface AsgSetDesiredCapacityPayload {
  auto_scaling_group_name: string;
  desired_capacity: number;
  honor_cooldown?: boolean;
}

/** Create a payload for the AWS Auto Scaling set-desired-capacity action. */
export function createAsgSetDesiredCapacityPayload(
  groupName: string,
  desiredCapacity: number,
  options?: { honorCooldown?: boolean }
): Record<string, unknown> {
  const payload: Record<string, unknown> = {
    auto_scaling_group_name: groupName,
    desired_capacity: desiredCapacity,
  };
  if (options?.honorCooldown !== undefined)
    payload.honor_cooldown = options.honorCooldown;
  return payload;
}

/** Payload for the AWS Auto Scaling update-group action. */
export interface AsgUpdateGroupPayload {
  auto_scaling_group_name: string;
  min_size?: number;
  max_size?: number;
  desired_capacity?: number;
  default_cooldown?: number;
  health_check_type?: string;
  health_check_grace_period?: number;
}

/** Create a payload for the AWS Auto Scaling update-group action. */
export function createAsgUpdateGroupPayload(
  groupName: string,
  options?: {
    minSize?: number;
    maxSize?: number;
    desiredCapacity?: number;
    defaultCooldown?: number;
    healthCheckType?: string;
    healthCheckGracePeriod?: number;
  }
): Record<string, unknown> {
  const payload: Record<string, unknown> = {
    auto_scaling_group_name: groupName,
  };
  if (options?.minSize !== undefined) payload.min_size = options.minSize;
  if (options?.maxSize !== undefined) payload.max_size = options.maxSize;
  if (options?.desiredCapacity !== undefined)
    payload.desired_capacity = options.desiredCapacity;
  if (options?.defaultCooldown !== undefined)
    payload.default_cooldown = options.defaultCooldown;
  if (options?.healthCheckType !== undefined)
    payload.health_check_type = options.healthCheckType;
  if (options?.healthCheckGracePeriod !== undefined)
    payload.health_check_grace_period = options.healthCheckGracePeriod;
  return payload;
}
