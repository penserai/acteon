/**
 * HTTP client for the Acteon action gateway.
 */

import {
  Action,
  ActionOutcome,
  AuditPage,
  AuditQuery,
  AuditRecord,
  BatchResult,
  ReloadResult,
  RuleInfo,
  EvaluateRulesRequest,
  EvaluateRulesResponse,
  EventQuery,
  EventState,
  EventListResponse,
  TransitionResponse,
  GroupSummary,
  GroupListResponse,
  GroupDetail,
  FlushGroupResponse,
  ApprovalActionResponse,
  ApprovalStatus,
  ApprovalListResponse,
  ReplayResult,
  ReplaySummary,
  ReplayQuery,
  CreateRecurringAction,
  CreateRecurringResponse,
  RecurringFilter,
  RecurringSummary,
  ListRecurringResponse,
  RecurringDetail,
  UpdateRecurringAction,
  ListChainsResponse,
  ChainDetailResponse,
  DlqStatsResponse,
  DlqDrainResponse,
  SseEvent,
  SubscribeOptions,
  StreamOptions,
  actionToRequest,
  auditQueryToParams,
  eventQueryToParams,
  parseActionOutcome,
  parseAuditPage,
  parseAuditRecord,
  parseBatchResult,
  parseEventState,
  parseEventListResponse,
  parseTransitionResponse,
  parseGroupSummary,
  parseGroupListResponse,
  parseGroupDetail,
  parseFlushGroupResponse,
  parseApprovalActionResponse,
  parseApprovalStatus,
  parseApprovalListResponse,
  parseReplayResult,
  parseReplaySummary,
  replayQueryToParams,
  createRecurringActionToRequest,
  parseCreateRecurringResponse,
  recurringFilterToParams,
  parseListRecurringResponse,
  parseRecurringDetail,
  updateRecurringActionToRequest,
  parseListChainsResponse,
  parseChainDetailResponse,
  DagResponse,
  parseDagResponse,
  parseDlqStatsResponse,
  parseDlqDrainResponse,
  CreateQuotaRequest,
  createQuotaRequestToApi,
  UpdateQuotaRequest,
  updateQuotaRequestToApi,
  QuotaPolicy,
  parseQuotaPolicy,
  ListQuotasResponse,
  parseListQuotasResponse,
  QuotaUsage,
  parseQuotaUsage,
  CreateRetentionRequest,
  createRetentionRequestToApi,
  UpdateRetentionRequest,
  updateRetentionRequestToApi,
  RetentionPolicy,
  parseRetentionPolicy,
  ListRetentionResponse,
  parseListRetentionResponse,
  ProviderHealthStatus,
  ListProviderHealthResponse,
  parseListProviderHealthResponse,
  WasmPlugin,
  RegisterPluginRequest,
  registerPluginRequestToApi,
  ListPluginsResponse,
  parseListPluginsResponse,
  parseWasmPlugin,
  PluginInvocationRequest,
  PluginInvocationResponse,
  parsePluginInvocationResponse,
  ComplianceStatus,
  parseComplianceStatus,
  HashChainVerification,
  parseHashChainVerification,
  VerifyHashChainRequest,
  TemplateInfo,
  parseTemplateInfo,
  CreateTemplateRequest,
  createTemplateRequestToApi,
  UpdateTemplateRequest,
  updateTemplateRequestToApi,
  ListTemplatesResponse,
  parseListTemplatesResponse,
  TemplateProfileInfo,
  parseTemplateProfileInfo,
  CreateProfileRequest,
  createProfileRequestToApi,
  UpdateProfileRequest,
  updateProfileRequestToApi,
  ListProfilesResponse,
  parseListProfilesResponse,
  RenderPreviewRequest,
  RenderPreviewResponse,
  parseRenderPreviewResponse,
} from "./models.js";
import { ActeonError, ApiError, ConnectionError, HttpError } from "./errors.js";

/**
 * Configuration options for the Acteon client.
 */
export interface ActeonClientOptions {
  /** Request timeout in milliseconds. Default: 30000. */
  timeout?: number;
  /** Optional API key for authentication. */
  apiKey?: string;
}

/**
 * HTTP client for the Acteon action gateway.
 *
 * @example
 * ```typescript
 * const client = new ActeonClient("http://localhost:8080");
 *
 * if (await client.health()) {
 *   const action = createAction(
 *     "notifications",
 *     "tenant-1",
 *     "email",
 *     "send_notification",
 *     { to: "user@example.com", subject: "Hello" }
 *   );
 *   const outcome = await client.dispatch(action);
 *   console.log(`Outcome: ${outcome.type}`);
 * }
 * ```
 */
export class ActeonClient {
  private readonly baseUrl: string;
  private readonly timeout: number;
  private readonly apiKey?: string;

  constructor(baseUrl: string, options: ActeonClientOptions = {}) {
    this.baseUrl = baseUrl.replace(/\/$/, "");
    this.timeout = options.timeout ?? 30000;
    this.apiKey = options.apiKey;
  }

  private headers(): Record<string, string> {
    const headers: Record<string, string> = {
      "Content-Type": "application/json",
    };
    if (this.apiKey) {
      headers["Authorization"] = `Bearer ${this.apiKey}`;
    }
    return headers;
  }

  private async request(
    method: string,
    path: string,
    options?: {
      body?: unknown;
      params?: URLSearchParams;
    }
  ): Promise<Response> {
    let url = `${this.baseUrl}${path}`;
    if (options?.params) {
      url += `?${options.params.toString()}`;
    }

    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), this.timeout);

    try {
      const response = await fetch(url, {
        method,
        headers: this.headers(),
        body: options?.body ? JSON.stringify(options.body) : undefined,
        signal: controller.signal,
      });
      return response;
    } catch (error) {
      if (error instanceof Error) {
        if (error.name === "AbortError") {
          throw new ConnectionError("Request timed out");
        }
        throw new ConnectionError(error.message);
      }
      throw new ConnectionError("Unknown error");
    } finally {
      clearTimeout(timeoutId);
    }
  }

  // =========================================================================
  // Health
  // =========================================================================

  /**
   * Check if the server is healthy.
   */
  async health(): Promise<boolean> {
    try {
      const response = await this.request("GET", "/health");
      return response.ok;
    } catch {
      return false;
    }
  }

  // =========================================================================
  // Action Dispatch
  // =========================================================================

  /**
   * Dispatch a single action.
   *
   * @param options.dryRun - When true, evaluates rules without executing the action.
   */
  async dispatch(
    action: Action,
    options?: { dryRun?: boolean }
  ): Promise<ActionOutcome> {
    const params =
      options?.dryRun ? new URLSearchParams({ dry_run: "true" }) : undefined;
    const response = await this.request("POST", "/v1/dispatch", {
      body: actionToRequest(action),
      params,
    });

    const data = (await response.json()) as Record<string, unknown>;

    if (response.ok) {
      return parseActionOutcome(data);
    } else {
      throw new ApiError(
        (data.code as string) ?? "UNKNOWN",
        (data.message as string) ?? "Unknown error",
        (data.retryable as boolean) ?? false
      );
    }
  }

  /**
   * Dispatch a single action in dry-run mode.
   * Rules are evaluated but the action is not executed and no state is mutated.
   */
  async dispatchDryRun(action: Action): Promise<ActionOutcome> {
    return this.dispatch(action, { dryRun: true });
  }

  /**
   * Dispatch multiple actions in a single request.
   *
   * @param options.dryRun - When true, evaluates rules without executing any actions.
   */
  async dispatchBatch(
    actions: Action[],
    options?: { dryRun?: boolean }
  ): Promise<BatchResult[]> {
    const params =
      options?.dryRun ? new URLSearchParams({ dry_run: "true" }) : undefined;
    const response = await this.request("POST", "/v1/dispatch/batch", {
      body: actions.map(actionToRequest),
      params,
    });

    if (response.ok) {
      const data = (await response.json()) as unknown[];
      return data.map((item) => parseBatchResult(item as Record<string, unknown>));
    } else {
      const data = (await response.json()) as Record<string, unknown>;
      throw new ApiError(
        (data.code as string) ?? "UNKNOWN",
        (data.message as string) ?? "Unknown error",
        (data.retryable as boolean) ?? false
      );
    }
  }

  /**
   * Dispatch multiple actions in dry-run mode.
   * Rules are evaluated for each action but none are executed and no state is mutated.
   */
  async dispatchBatchDryRun(actions: Action[]): Promise<BatchResult[]> {
    return this.dispatchBatch(actions, { dryRun: true });
  }

  // =========================================================================
  // Rules Management
  // =========================================================================

  /**
   * List all loaded rules.
   */
  async listRules(): Promise<RuleInfo[]> {
    const response = await this.request("GET", "/v1/rules");

    if (response.ok) {
      return (await response.json()) as RuleInfo[];
    } else {
      throw new HttpError(response.status, "Failed to list rules");
    }
  }

  /**
   * Reload rules from the configured directory.
   */
  async reloadRules(): Promise<ReloadResult> {
    const response = await this.request("POST", "/v1/rules/reload");

    if (response.ok) {
      return (await response.json()) as ReloadResult;
    } else {
      throw new HttpError(response.status, "Failed to reload rules");
    }
  }

  /**
   * Enable or disable a specific rule.
   */
  async setRuleEnabled(ruleName: string, enabled: boolean): Promise<void> {
    const response = await this.request("PUT", `/v1/rules/${ruleName}/enabled`, {
      body: { enabled },
    });

    if (!response.ok) {
      throw new HttpError(response.status, "Failed to set rule enabled");
    }
  }

  /**
   * Evaluate rules against a test action without dispatching.
   *
   * This is the Rule Playground endpoint. It evaluates all matching rules
   * against the provided action parameters and returns a detailed trace
   * of each rule evaluation.
   */
  async evaluateRules(request: EvaluateRulesRequest): Promise<EvaluateRulesResponse> {
    const response = await this.request("POST", "/v1/rules/evaluate", {
      body: request,
    });

    if (response.ok) {
      return (await response.json()) as EvaluateRulesResponse;
    } else {
      throw new HttpError(response.status, "Failed to evaluate rules");
    }
  }

  // =========================================================================
  // Audit Trail
  // =========================================================================

  /**
   * Query audit records.
   */
  async queryAudit(query?: AuditQuery): Promise<AuditPage> {
    const params = query ? auditQueryToParams(query) : undefined;
    const response = await this.request("GET", "/v1/audit", { params });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseAuditPage(data);
    } else {
      throw new HttpError(response.status, "Failed to query audit");
    }
  }

  /**
   * Get a specific audit record by action ID.
   */
  async getAuditRecord(actionId: string): Promise<AuditRecord | null> {
    const response = await this.request("GET", `/v1/audit/${actionId}`);

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseAuditRecord(data);
    } else if (response.status === 404) {
      return null;
    } else {
      throw new HttpError(response.status, "Failed to get audit record");
    }
  }

  // =========================================================================
  // Audit Replay
  // =========================================================================

  /**
   * Replay a single action from the audit trail by its action ID.
   * The action is reconstructed from the stored payload and dispatched with a new ID.
   */
  async replayAction(actionId: string): Promise<ReplayResult> {
    const response = await this.request("POST", `/v1/audit/${actionId}/replay`);

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseReplayResult(data);
    } else if (response.status === 404) {
      throw new HttpError(404, `Audit record not found: ${actionId}`);
    } else if (response.status === 422) {
      throw new HttpError(422, "No stored payload available for replay");
    } else {
      throw new HttpError(response.status, "Failed to replay action");
    }
  }

  /**
   * Bulk replay actions from the audit trail matching the given query.
   */
  async replayAudit(query?: ReplayQuery): Promise<ReplaySummary> {
    const params = query ? replayQueryToParams(query) : undefined;
    const response = await this.request("POST", "/v1/audit/replay", { params });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseReplaySummary(data);
    } else {
      throw new HttpError(response.status, "Failed to replay audit");
    }
  }

  // =========================================================================
  // Events (State Machine Lifecycle)
  // =========================================================================

  /**
   * List events filtered by namespace, tenant, and optionally status.
   */
  async listEvents(query: EventQuery): Promise<EventListResponse> {
    const params = eventQueryToParams(query);
    const response = await this.request("GET", "/v1/events", { params });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseEventListResponse(data);
    } else {
      throw new HttpError(response.status, "Failed to list events");
    }
  }

  /**
   * Get the current state of an event by fingerprint.
   */
  async getEvent(
    fingerprint: string,
    namespace: string,
    tenant: string
  ): Promise<EventState | null> {
    const params = new URLSearchParams();
    params.set("namespace", namespace);
    params.set("tenant", tenant);
    const response = await this.request("GET", `/v1/events/${fingerprint}`, { params });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseEventState(data);
    } else if (response.status === 404) {
      return null;
    } else {
      throw new HttpError(response.status, "Failed to get event");
    }
  }

  /**
   * Transition an event to a new state.
   */
  async transitionEvent(
    fingerprint: string,
    toState: string,
    namespace: string,
    tenant: string
  ): Promise<TransitionResponse> {
    const response = await this.request("PUT", `/v1/events/${fingerprint}/transition`, {
      body: { to: toState, namespace, tenant },
    });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseTransitionResponse(data);
    } else if (response.status === 404) {
      throw new HttpError(404, `Event not found: ${fingerprint}`);
    } else {
      const data = (await response.json()) as Record<string, unknown>;
      throw new ApiError(
        (data.code as string) ?? "UNKNOWN",
        (data.message as string) ?? "Unknown error",
        (data.retryable as boolean) ?? false
      );
    }
  }

  // =========================================================================
  // Groups (Event Batching)
  // =========================================================================

  /**
   * List all active event groups.
   */
  async listGroups(): Promise<GroupListResponse> {
    const response = await this.request("GET", "/v1/groups");

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseGroupListResponse(data);
    } else {
      throw new HttpError(response.status, "Failed to list groups");
    }
  }

  /**
   * Get details of a specific group.
   */
  async getGroup(groupKey: string): Promise<GroupDetail | null> {
    const response = await this.request("GET", `/v1/groups/${groupKey}`);

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseGroupDetail(data);
    } else if (response.status === 404) {
      return null;
    } else {
      throw new HttpError(response.status, "Failed to get group");
    }
  }

  /**
   * Force flush a group, triggering immediate notification.
   */
  async flushGroup(groupKey: string): Promise<FlushGroupResponse> {
    const response = await this.request("DELETE", `/v1/groups/${groupKey}`);

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseFlushGroupResponse(data);
    } else if (response.status === 404) {
      throw new HttpError(404, `Group not found: ${groupKey}`);
    } else {
      const data = (await response.json()) as Record<string, unknown>;
      throw new ApiError(
        (data.code as string) ?? "UNKNOWN",
        (data.message as string) ?? "Unknown error",
        (data.retryable as boolean) ?? false
      );
    }
  }

  // =========================================================================
  // Approvals (Human-in-the-Loop)
  // =========================================================================

  /**
   * Approve a pending action by namespace, tenant, ID, and HMAC signature.
   * Does not require authentication -- the HMAC signature serves as proof of authorization.
   */
  async approve(namespace: string, tenant: string, id: string, sig: string, expiresAt: number, kid?: string): Promise<ApprovalActionResponse> {
    const params = new URLSearchParams();
    params.set("sig", sig);
    params.set("expires_at", expiresAt.toString());
    if (kid !== undefined) {
      params.set("kid", kid);
    }
    const response = await this.request("POST", `/v1/approvals/${namespace}/${tenant}/${id}/approve`, { params });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseApprovalActionResponse(data);
    } else if (response.status === 404) {
      throw new HttpError(404, "Approval not found or expired");
    } else if (response.status === 410) {
      throw new HttpError(410, "Approval already decided");
    } else {
      throw new HttpError(response.status, "Failed to approve");
    }
  }

  /**
   * Reject a pending action by namespace, tenant, ID, and HMAC signature.
   * Does not require authentication -- the HMAC signature serves as proof of authorization.
   */
  async reject(namespace: string, tenant: string, id: string, sig: string, expiresAt: number, kid?: string): Promise<ApprovalActionResponse> {
    const params = new URLSearchParams();
    params.set("sig", sig);
    params.set("expires_at", expiresAt.toString());
    if (kid !== undefined) {
      params.set("kid", kid);
    }
    const response = await this.request("POST", `/v1/approvals/${namespace}/${tenant}/${id}/reject`, { params });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseApprovalActionResponse(data);
    } else if (response.status === 404) {
      throw new HttpError(404, "Approval not found or expired");
    } else if (response.status === 410) {
      throw new HttpError(410, "Approval already decided");
    } else {
      throw new HttpError(response.status, "Failed to reject");
    }
  }

  /**
   * Get the status of an approval by namespace, tenant, ID, and HMAC signature.
   * Returns null if not found or expired.
   */
  async getApproval(namespace: string, tenant: string, id: string, sig: string, expiresAt: number, kid?: string): Promise<ApprovalStatus | null> {
    const params = new URLSearchParams();
    params.set("sig", sig);
    params.set("expires_at", expiresAt.toString());
    if (kid !== undefined) {
      params.set("kid", kid);
    }
    const response = await this.request("GET", `/v1/approvals/${namespace}/${tenant}/${id}`, { params });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseApprovalStatus(data);
    } else if (response.status === 404) {
      return null;
    } else {
      throw new HttpError(response.status, "Failed to get approval");
    }
  }

  /**
   * List pending approvals filtered by namespace and tenant.
   * Requires authentication.
   */
  async listApprovals(namespace: string, tenant: string): Promise<ApprovalListResponse> {
    const params = new URLSearchParams();
    params.set("namespace", namespace);
    params.set("tenant", tenant);
    const response = await this.request("GET", "/v1/approvals", { params });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseApprovalListResponse(data);
    } else {
      throw new HttpError(response.status, "Failed to list approvals");
    }
  }

  // =========================================================================
  // Recurring Actions
  // =========================================================================

  /**
   * Create a recurring action.
   */
  async createRecurring(recurring: CreateRecurringAction): Promise<CreateRecurringResponse> {
    const response = await this.request("POST", "/v1/recurring", {
      body: createRecurringActionToRequest(recurring),
    });

    if (response.status === 201) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseCreateRecurringResponse(data);
    } else {
      const data = (await response.json()) as Record<string, unknown>;
      throw new ApiError(
        (data.code as string) ?? "UNKNOWN",
        (data.message as string) ?? "Unknown error",
        (data.retryable as boolean) ?? false
      );
    }
  }

  /**
   * List recurring actions.
   */
  async listRecurring(filter?: RecurringFilter): Promise<ListRecurringResponse> {
    const params = filter ? recurringFilterToParams(filter) : undefined;
    const response = await this.request("GET", "/v1/recurring", { params });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseListRecurringResponse(data);
    } else {
      throw new HttpError(response.status, "Failed to list recurring actions");
    }
  }

  /**
   * Get details of a specific recurring action.
   */
  async getRecurring(recurringId: string, namespace: string, tenant: string): Promise<RecurringDetail | null> {
    const params = new URLSearchParams();
    params.set("namespace", namespace);
    params.set("tenant", tenant);
    const response = await this.request("GET", `/v1/recurring/${recurringId}`, { params });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseRecurringDetail(data);
    } else if (response.status === 404) {
      return null;
    } else {
      throw new HttpError(response.status, "Failed to get recurring action");
    }
  }

  /**
   * Update a recurring action.
   */
  async updateRecurring(recurringId: string, update: UpdateRecurringAction): Promise<RecurringDetail> {
    const response = await this.request("PUT", `/v1/recurring/${recurringId}`, {
      body: updateRecurringActionToRequest(update),
    });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseRecurringDetail(data);
    } else if (response.status === 404) {
      throw new HttpError(404, `Recurring action not found: ${recurringId}`);
    } else {
      const data = (await response.json()) as Record<string, unknown>;
      throw new ApiError(
        (data.code as string) ?? "UNKNOWN",
        (data.message as string) ?? "Unknown error",
        (data.retryable as boolean) ?? false
      );
    }
  }

  /**
   * Delete a recurring action.
   */
  async deleteRecurring(recurringId: string, namespace: string, tenant: string): Promise<void> {
    const params = new URLSearchParams();
    params.set("namespace", namespace);
    params.set("tenant", tenant);
    const response = await this.request("DELETE", `/v1/recurring/${recurringId}`, { params });

    if (response.status === 204) {
      return;
    } else if (response.status === 404) {
      throw new HttpError(404, `Recurring action not found: ${recurringId}`);
    } else {
      throw new HttpError(response.status, "Failed to delete recurring action");
    }
  }

  /**
   * Pause a recurring action.
   */
  async pauseRecurring(recurringId: string, namespace: string, tenant: string): Promise<RecurringDetail> {
    const response = await this.request("POST", `/v1/recurring/${recurringId}/pause`, {
      body: { namespace, tenant },
    });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseRecurringDetail(data);
    } else if (response.status === 404) {
      throw new HttpError(404, `Recurring action not found: ${recurringId}`);
    } else if (response.status === 409) {
      throw new HttpError(409, "Recurring action is already paused");
    } else {
      throw new HttpError(response.status, "Failed to pause recurring action");
    }
  }

  /**
   * Resume a paused recurring action.
   */
  async resumeRecurring(recurringId: string, namespace: string, tenant: string): Promise<RecurringDetail> {
    const response = await this.request("POST", `/v1/recurring/${recurringId}/resume`, {
      body: { namespace, tenant },
    });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseRecurringDetail(data);
    } else if (response.status === 404) {
      throw new HttpError(404, `Recurring action not found: ${recurringId}`);
    } else if (response.status === 409) {
      throw new HttpError(409, "Recurring action is already active");
    } else {
      throw new HttpError(response.status, "Failed to resume recurring action");
    }
  }

  // =========================================================================
  // Quotas
  // =========================================================================

  /**
   * Create a quota policy.
   */
  async createQuota(req: CreateQuotaRequest): Promise<QuotaPolicy> {
    const response = await this.request("POST", "/v1/quotas", {
      body: createQuotaRequestToApi(req),
    });

    if (response.status === 201) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseQuotaPolicy(data);
    } else {
      const data = (await response.json()) as Record<string, unknown>;
      throw new ApiError(
        (data.code as string) ?? "UNKNOWN",
        (data.message as string) ?? "Unknown error",
        (data.retryable as boolean) ?? false
      );
    }
  }

  /**
   * List quota policies.
   */
  async listQuotas(namespace?: string, tenant?: string): Promise<ListQuotasResponse> {
    const params = new URLSearchParams();
    if (namespace !== undefined) params.set("namespace", namespace);
    if (tenant !== undefined) params.set("tenant", tenant);
    const response = await this.request("GET", "/v1/quotas", { params: params.toString() ? params : undefined });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseListQuotasResponse(data);
    } else {
      throw new HttpError(response.status, "Failed to list quotas");
    }
  }

  /**
   * Get a single quota policy by ID.
   */
  async getQuota(quotaId: string): Promise<QuotaPolicy | null> {
    const response = await this.request("GET", `/v1/quotas/${quotaId}`);

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseQuotaPolicy(data);
    } else if (response.status === 404) {
      return null;
    } else {
      throw new HttpError(response.status, "Failed to get quota");
    }
  }

  /**
   * Update a quota policy.
   */
  async updateQuota(quotaId: string, update: UpdateQuotaRequest): Promise<QuotaPolicy> {
    const response = await this.request("PUT", `/v1/quotas/${quotaId}`, {
      body: updateQuotaRequestToApi(update),
    });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseQuotaPolicy(data);
    } else if (response.status === 404) {
      throw new HttpError(404, `Quota not found: ${quotaId}`);
    } else {
      const data = (await response.json()) as Record<string, unknown>;
      throw new ApiError(
        (data.code as string) ?? "UNKNOWN",
        (data.message as string) ?? "Unknown error",
        (data.retryable as boolean) ?? false
      );
    }
  }

  /**
   * Delete a quota policy.
   */
  async deleteQuota(quotaId: string, namespace: string, tenant: string): Promise<void> {
    const params = new URLSearchParams();
    params.set("namespace", namespace);
    params.set("tenant", tenant);
    const response = await this.request("DELETE", `/v1/quotas/${quotaId}`, { params });

    if (response.status === 204) {
      return;
    } else if (response.status === 404) {
      throw new HttpError(404, `Quota not found: ${quotaId}`);
    } else {
      throw new HttpError(response.status, "Failed to delete quota");
    }
  }

  /**
   * Get current usage statistics for a quota policy.
   */
  async getQuotaUsage(quotaId: string): Promise<QuotaUsage> {
    const response = await this.request("GET", `/v1/quotas/${quotaId}/usage`);

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseQuotaUsage(data);
    } else if (response.status === 404) {
      throw new HttpError(404, `Quota not found: ${quotaId}`);
    } else {
      throw new HttpError(response.status, "Failed to get quota usage");
    }
  }

  // =========================================================================
  // Retention Policies
  // =========================================================================

  /**
   * Create a retention policy.
   */
  async createRetention(req: CreateRetentionRequest): Promise<RetentionPolicy> {
    const response = await this.request("POST", "/v1/retention", {
      body: createRetentionRequestToApi(req),
    });

    if (response.status === 201) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseRetentionPolicy(data);
    } else {
      const data = (await response.json()) as Record<string, unknown>;
      throw new ApiError(
        (data.code as string) ?? "UNKNOWN",
        (data.message as string) ?? "Unknown error",
        (data.retryable as boolean) ?? false
      );
    }
  }

  /**
   * List retention policies.
   */
  async listRetention(namespace?: string, tenant?: string, limit?: number, offset?: number): Promise<ListRetentionResponse> {
    const params = new URLSearchParams();
    if (namespace !== undefined) params.set("namespace", namespace);
    if (tenant !== undefined) params.set("tenant", tenant);
    if (limit !== undefined) params.set("limit", limit.toString());
    if (offset !== undefined) params.set("offset", offset.toString());
    const response = await this.request("GET", "/v1/retention", { params: params.toString() ? params : undefined });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseListRetentionResponse(data);
    } else {
      throw new HttpError(response.status, "Failed to list retention policies");
    }
  }

  /**
   * Get a single retention policy by ID.
   */
  async getRetention(retentionId: string): Promise<RetentionPolicy | null> {
    const response = await this.request("GET", `/v1/retention/${retentionId}`);

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseRetentionPolicy(data);
    } else if (response.status === 404) {
      return null;
    } else {
      throw new HttpError(response.status, "Failed to get retention policy");
    }
  }

  /**
   * Update a retention policy.
   */
  async updateRetention(retentionId: string, update: UpdateRetentionRequest): Promise<RetentionPolicy> {
    const response = await this.request("PUT", `/v1/retention/${retentionId}`, {
      body: updateRetentionRequestToApi(update),
    });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseRetentionPolicy(data);
    } else if (response.status === 404) {
      throw new HttpError(404, `Retention policy not found: ${retentionId}`);
    } else {
      const data = (await response.json()) as Record<string, unknown>;
      throw new ApiError(
        (data.code as string) ?? "UNKNOWN",
        (data.message as string) ?? "Unknown error",
        (data.retryable as boolean) ?? false
      );
    }
  }

  /**
   * Delete a retention policy.
   */
  async deleteRetention(retentionId: string): Promise<void> {
    const response = await this.request("DELETE", `/v1/retention/${retentionId}`);

    if (response.status === 204) {
      return;
    } else if (response.status === 404) {
      throw new HttpError(404, `Retention policy not found: ${retentionId}`);
    } else {
      throw new HttpError(response.status, "Failed to delete retention policy");
    }
  }

  // =========================================================================
  // Payload Templates
  // =========================================================================

  /**
   * Create a payload template.
   */
  async createTemplate(req: CreateTemplateRequest): Promise<TemplateInfo> {
    const response = await this.request("POST", "/v1/templates", {
      body: createTemplateRequestToApi(req),
    });

    if (response.status === 201) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseTemplateInfo(data);
    } else {
      const data = (await response.json()) as Record<string, unknown>;
      throw new ApiError(
        (data.code as string) ?? "UNKNOWN",
        (data.message as string) ?? "Unknown error",
        (data.retryable as boolean) ?? false
      );
    }
  }

  /**
   * List payload templates.
   */
  async listTemplates(namespace?: string, tenant?: string): Promise<ListTemplatesResponse> {
    const params = new URLSearchParams();
    if (namespace !== undefined) params.set("namespace", namespace);
    if (tenant !== undefined) params.set("tenant", tenant);
    const response = await this.request("GET", "/v1/templates", { params: params.toString() ? params : undefined });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseListTemplatesResponse(data);
    } else {
      throw new HttpError(response.status, "Failed to list templates");
    }
  }

  /**
   * Get a single template by ID.
   */
  async getTemplate(templateId: string): Promise<TemplateInfo | null> {
    const response = await this.request("GET", `/v1/templates/${templateId}`);

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseTemplateInfo(data);
    } else if (response.status === 404) {
      return null;
    } else {
      throw new HttpError(response.status, "Failed to get template");
    }
  }

  /**
   * Update a payload template.
   */
  async updateTemplate(templateId: string, update: UpdateTemplateRequest): Promise<TemplateInfo> {
    const response = await this.request("PUT", `/v1/templates/${templateId}`, {
      body: updateTemplateRequestToApi(update),
    });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseTemplateInfo(data);
    } else if (response.status === 404) {
      throw new HttpError(404, `Template not found: ${templateId}`);
    } else {
      const data = (await response.json()) as Record<string, unknown>;
      throw new ApiError(
        (data.code as string) ?? "UNKNOWN",
        (data.message as string) ?? "Unknown error",
        (data.retryable as boolean) ?? false
      );
    }
  }

  /**
   * Delete a payload template.
   */
  async deleteTemplate(templateId: string): Promise<void> {
    const response = await this.request("DELETE", `/v1/templates/${templateId}`);

    if (response.status === 204) {
      return;
    } else if (response.status === 404) {
      throw new HttpError(404, `Template not found: ${templateId}`);
    } else {
      throw new HttpError(response.status, "Failed to delete template");
    }
  }

  /**
   * Create a template profile.
   */
  async createProfile(req: CreateProfileRequest): Promise<TemplateProfileInfo> {
    const response = await this.request("POST", "/v1/templates/profiles", {
      body: createProfileRequestToApi(req),
    });

    if (response.status === 201) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseTemplateProfileInfo(data);
    } else {
      const data = (await response.json()) as Record<string, unknown>;
      throw new ApiError(
        (data.code as string) ?? "UNKNOWN",
        (data.message as string) ?? "Unknown error",
        (data.retryable as boolean) ?? false
      );
    }
  }

  /**
   * List template profiles.
   */
  async listProfiles(namespace?: string, tenant?: string): Promise<ListProfilesResponse> {
    const params = new URLSearchParams();
    if (namespace !== undefined) params.set("namespace", namespace);
    if (tenant !== undefined) params.set("tenant", tenant);
    const response = await this.request("GET", "/v1/templates/profiles", { params: params.toString() ? params : undefined });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseListProfilesResponse(data);
    } else {
      throw new HttpError(response.status, "Failed to list profiles");
    }
  }

  /**
   * Get a single template profile by ID.
   */
  async getProfile(profileId: string): Promise<TemplateProfileInfo | null> {
    const response = await this.request("GET", `/v1/templates/profiles/${profileId}`);

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseTemplateProfileInfo(data);
    } else if (response.status === 404) {
      return null;
    } else {
      throw new HttpError(response.status, "Failed to get profile");
    }
  }

  /**
   * Update a template profile.
   */
  async updateProfile(profileId: string, update: UpdateProfileRequest): Promise<TemplateProfileInfo> {
    const response = await this.request("PUT", `/v1/templates/profiles/${profileId}`, {
      body: updateProfileRequestToApi(update),
    });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseTemplateProfileInfo(data);
    } else if (response.status === 404) {
      throw new HttpError(404, `Profile not found: ${profileId}`);
    } else {
      const data = (await response.json()) as Record<string, unknown>;
      throw new ApiError(
        (data.code as string) ?? "UNKNOWN",
        (data.message as string) ?? "Unknown error",
        (data.retryable as boolean) ?? false
      );
    }
  }

  /**
   * Delete a template profile.
   */
  async deleteProfile(profileId: string): Promise<void> {
    const response = await this.request("DELETE", `/v1/templates/profiles/${profileId}`);

    if (response.status === 204) {
      return;
    } else if (response.status === 404) {
      throw new HttpError(404, `Profile not found: ${profileId}`);
    } else {
      throw new HttpError(response.status, "Failed to delete profile");
    }
  }

  /**
   * Render a template profile with payload data.
   */
  async renderPreview(req: RenderPreviewRequest): Promise<RenderPreviewResponse> {
    const response = await this.request("POST", "/v1/templates/render", {
      body: req,
    });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseRenderPreviewResponse(data);
    } else {
      const data = (await response.json()) as Record<string, unknown>;
      throw new ApiError(
        (data.code as string) ?? "UNKNOWN",
        (data.message as string) ?? "Unknown error",
        (data.retryable as boolean) ?? false
      );
    }
  }

  // =========================================================================
  // Provider Health
  // =========================================================================

  /**
   * List health and metrics for all providers.
   */
  async listProviderHealth(): Promise<ListProviderHealthResponse> {
    const response = await this.request("GET", "/v1/providers/health");

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseListProviderHealthResponse(data);
    } else {
      throw new HttpError(response.status, "Failed to list provider health");
    }
  }

  // =========================================================================
  // WASM Plugins
  // =========================================================================

  /**
   * List all registered WASM plugins.
   */
  async listPlugins(): Promise<ListPluginsResponse> {
    const response = await this.request("GET", "/v1/plugins");

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseListPluginsResponse(data);
    } else {
      throw new HttpError(response.status, "Failed to list plugins");
    }
  }

  /**
   * Register a new WASM plugin.
   */
  async registerPlugin(req: RegisterPluginRequest): Promise<WasmPlugin> {
    const response = await this.request("POST", "/v1/plugins", {
      body: registerPluginRequestToApi(req),
    });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseWasmPlugin(data);
    } else {
      const data = (await response.json()) as Record<string, unknown>;
      throw new ApiError(
        (data.code as string) ?? "UNKNOWN",
        (data.message as string) ?? "Unknown error",
        (data.retryable as boolean) ?? false
      );
    }
  }

  /**
   * Get details of a registered WASM plugin.
   */
  async getPlugin(name: string): Promise<WasmPlugin | null> {
    const response = await this.request("GET", `/v1/plugins/${name}`);

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseWasmPlugin(data);
    } else if (response.status === 404) {
      return null;
    } else {
      throw new HttpError(response.status, "Failed to get plugin");
    }
  }

  /**
   * Unregister (delete) a WASM plugin.
   */
  async deletePlugin(name: string): Promise<void> {
    const response = await this.request("DELETE", `/v1/plugins/${name}`);

    if (response.status === 204) {
      return;
    } else if (response.status === 404) {
      throw new HttpError(404, `Plugin not found: ${name}`);
    } else {
      throw new HttpError(response.status, "Failed to delete plugin");
    }
  }

  /**
   * Test-invoke a WASM plugin.
   */
  async invokePlugin(name: string, req: PluginInvocationRequest): Promise<PluginInvocationResponse> {
    const body: Record<string, unknown> = { input: req.input };
    if (req.function !== undefined) body.function = req.function;

    const response = await this.request("POST", `/v1/plugins/${name}/invoke`, { body });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parsePluginInvocationResponse(data);
    } else if (response.status === 404) {
      throw new HttpError(404, `Plugin not found: ${name}`);
    } else {
      throw new HttpError(response.status, `Failed to invoke plugin: ${name}`);
    }
  }

  // =========================================================================
  // Compliance (SOC2/HIPAA)
  // =========================================================================

  /**
   * Get the current compliance configuration status.
   */
  async getComplianceStatus(): Promise<ComplianceStatus> {
    const response = await this.request("GET", "/v1/compliance/status");
    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseComplianceStatus(data);
    }
    throw new HttpError(response.status, "Failed to get compliance status");
  }

  /**
   * Verify the integrity of the audit hash chain for a namespace/tenant pair.
   */
  async verifyAuditChain(req: VerifyHashChainRequest): Promise<HashChainVerification> {
    const body: Record<string, unknown> = {
      namespace: req.namespace,
      tenant: req.tenant,
    };
    if (req.from !== undefined) body.from = req.from;
    if (req.to !== undefined) body.to = req.to;

    const response = await this.request("POST", "/v1/audit/verify", { body });
    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseHashChainVerification(data);
    }
    throw new HttpError(response.status, "Failed to verify audit chain");
  }

  // =========================================================================
  // Chains
  // =========================================================================

  /**
   * List chain executions filtered by namespace, tenant, and optional status.
   */
  async listChains(namespace: string, tenant: string, status?: string): Promise<ListChainsResponse> {
    const params = new URLSearchParams();
    params.set("namespace", namespace);
    params.set("tenant", tenant);
    if (status !== undefined) {
      params.set("status", status);
    }
    const response = await this.request("GET", "/v1/chains", { params });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseListChainsResponse(data);
    } else {
      throw new HttpError(response.status, "Failed to list chains");
    }
  }

  /**
   * Get full details of a chain execution including step results.
   */
  async getChain(chainId: string, namespace: string, tenant: string): Promise<ChainDetailResponse | null> {
    const params = new URLSearchParams();
    params.set("namespace", namespace);
    params.set("tenant", tenant);
    const response = await this.request("GET", `/v1/chains/${chainId}`, { params });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseChainDetailResponse(data);
    } else if (response.status === 404) {
      return null;
    } else {
      throw new HttpError(response.status, "Failed to get chain");
    }
  }

  /**
   * Cancel a running chain execution.
   */
  async cancelChain(
    chainId: string,
    namespace: string,
    tenant: string,
    reason?: string,
    cancelledBy?: string
  ): Promise<ChainDetailResponse> {
    const body: Record<string, unknown> = { namespace, tenant };
    if (reason !== undefined) {
      body.reason = reason;
    }
    if (cancelledBy !== undefined) {
      body.cancelled_by = cancelledBy;
    }
    const response = await this.request("POST", `/v1/chains/${chainId}/cancel`, { body });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseChainDetailResponse(data);
    } else if (response.status === 404) {
      throw new HttpError(404, `Chain not found: ${chainId}`);
    } else if (response.status === 409) {
      throw new HttpError(409, "Chain is not running");
    } else {
      throw new HttpError(response.status, "Failed to cancel chain");
    }
  }

  /**
   * Get the DAG representation for a running chain instance.
   */
  async getChainDag(chainId: string, namespace: string, tenant: string): Promise<DagResponse> {
    const params = new URLSearchParams();
    params.set("namespace", namespace);
    params.set("tenant", tenant);
    const response = await this.request("GET", `/v1/chains/${chainId}/dag`, { params });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseDagResponse(data);
    } else if (response.status === 404) {
      throw new HttpError(404, `Chain not found: ${chainId}`);
    } else {
      throw new HttpError(response.status, "Failed to get chain DAG");
    }
  }

  /**
   * Get the DAG representation for a chain definition (config only).
   */
  async getChainDefinitionDag(name: string): Promise<DagResponse> {
    const response = await this.request("GET", `/v1/chains/definitions/${name}/dag`);

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseDagResponse(data);
    } else if (response.status === 404) {
      throw new HttpError(404, `Chain definition not found: ${name}`);
    } else {
      throw new HttpError(response.status, "Failed to get chain definition DAG");
    }
  }

  // =========================================================================
  // DLQ (Dead-Letter Queue)
  // =========================================================================

  /**
   * Get dead-letter queue statistics.
   */
  async dlqStats(): Promise<DlqStatsResponse> {
    const response = await this.request("GET", "/v1/dlq/stats");

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseDlqStatsResponse(data);
    } else {
      throw new HttpError(response.status, "Failed to get DLQ stats");
    }
  }

  /**
   * Drain all entries from the dead-letter queue.
   * Removes and returns all entries for manual processing or resubmission.
   */
  async dlqDrain(): Promise<DlqDrainResponse> {
    const response = await this.request("POST", "/v1/dlq/drain");

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseDlqDrainResponse(data);
    } else if (response.status === 404) {
      throw new HttpError(404, "Dead-letter queue is not enabled");
    } else {
      throw new HttpError(response.status, "Failed to drain DLQ");
    }
  }

  // =========================================================================
  // Subscribe (SSE)
  // =========================================================================

  /**
   * Subscribe to events for a specific entity via Server-Sent Events.
   *
   * Returns an `AsyncGenerator` that yields parsed SSE events.
   * The generator completes when the connection is closed.
   *
   * @param entityType - The entity type: "chain", "group", or "action".
   * @param entityId - The entity ID to subscribe to.
   * @param options - Optional namespace, tenant, and includeHistory settings.
   *
   * @example
   * ```typescript
   * for await (const event of client.subscribe("chain", "chain-42", { namespace: "ns", tenant: "t1" })) {
   *   console.log(`${event.event}: ${JSON.stringify(event.data)}`);
   * }
   * ```
   */
  async *subscribe(
    entityType: string,
    entityId: string,
    options?: SubscribeOptions
  ): AsyncGenerator<SseEvent, void, undefined> {
    const params = new URLSearchParams();
    if (options?.namespace !== undefined) {
      params.set("namespace", options.namespace);
    }
    if (options?.tenant !== undefined) {
      params.set("tenant", options.tenant);
    }
    if (options?.includeHistory !== undefined) {
      params.set("include_history", options.includeHistory.toString());
    }

    yield* this.connectSse(`/v1/subscribe/${entityType}/${entityId}`, params);
  }

  // =========================================================================
  // Stream (SSE)
  // =========================================================================

  /**
   * Subscribe to the real-time event stream via Server-Sent Events.
   *
   * Returns an `AsyncGenerator` that yields parsed SSE events.
   * The generator completes when the connection is closed.
   *
   * @param options - Optional filters for namespace, actionType, outcome, eventType,
   *   chainId, groupId, actionId, and lastEventId for reconnection catch-up.
   *
   * @example
   * ```typescript
   * for await (const event of client.stream({ namespace: "alerts", eventType: "action_dispatched" })) {
   *   console.log(`${event.event}: ${JSON.stringify(event.data)}`);
   * }
   * ```
   */
  async *stream(options?: StreamOptions): AsyncGenerator<SseEvent, void, undefined> {
    const params = new URLSearchParams();
    if (options?.namespace !== undefined) {
      params.set("namespace", options.namespace);
    }
    if (options?.actionType !== undefined) {
      params.set("action_type", options.actionType);
    }
    if (options?.outcome !== undefined) {
      params.set("outcome", options.outcome);
    }
    if (options?.eventType !== undefined) {
      params.set("event_type", options.eventType);
    }
    if (options?.chainId !== undefined) {
      params.set("chain_id", options.chainId);
    }
    if (options?.groupId !== undefined) {
      params.set("group_id", options.groupId);
    }
    if (options?.actionId !== undefined) {
      params.set("action_id", options.actionId);
    }

    const headers: Record<string, string> = {};
    if (options?.lastEventId !== undefined) {
      headers["Last-Event-ID"] = options.lastEventId;
    }

    yield* this.connectSse("/v1/stream", params, headers);
  }

  // =========================================================================
  // SSE Helpers (private)
  // =========================================================================

  /**
   * Connect to an SSE endpoint and yield parsed events.
   */
  private async *connectSse(
    path: string,
    params?: URLSearchParams,
    extraHeaders?: Record<string, string>
  ): AsyncGenerator<SseEvent, void, undefined> {
    let url = `${this.baseUrl}${path}`;
    if (params && params.toString()) {
      url += `?${params.toString()}`;
    }

    const headers: Record<string, string> = {
      Accept: "text/event-stream",
    };
    if (this.apiKey) {
      headers["Authorization"] = `Bearer ${this.apiKey}`;
    }
    if (extraHeaders) {
      Object.assign(headers, extraHeaders);
    }

    const response = await fetch(url, {
      method: "GET",
      headers,
    });

    if (!response.ok) {
      throw new HttpError(response.status, `SSE connection failed: ${path}`);
    }

    if (!response.body) {
      throw new ConnectionError("Response body is null");
    }

    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    let currentEvent = "";
    let currentId = "";
    let currentData: string[] = [];

    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;

        buffer += decoder.decode(value, { stream: true });
        const lines = buffer.split("\n");
        // Keep the last incomplete line in the buffer.
        buffer = lines.pop() ?? "";

        for (const line of lines) {
          if (line === "") {
            // Empty line signals end of an event.
            if (currentData.length > 0) {
              const dataStr = currentData.join("\n");
              let parsedData: unknown;
              try {
                parsedData = JSON.parse(dataStr);
              } catch {
                parsedData = dataStr;
              }
              yield {
                event: currentEvent || "message",
                id: currentId,
                data: parsedData,
              };
            }
            currentEvent = "";
            currentId = "";
            currentData = [];
          } else if (line.startsWith("event:")) {
            currentEvent = line.slice(6).trim();
          } else if (line.startsWith("id:")) {
            currentId = line.slice(3).trim();
          } else if (line.startsWith("data:")) {
            currentData.push(line.slice(5).trim());
          }
          // Ignore comments (lines starting with ':') and unknown fields.
        }
      }
    } finally {
      reader.releaseLock();
    }
  }
}
