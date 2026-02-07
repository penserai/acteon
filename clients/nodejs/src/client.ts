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
}
