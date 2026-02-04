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
  actionToRequest,
  auditQueryToParams,
  parseActionOutcome,
  parseAuditPage,
  parseAuditRecord,
  parseBatchResult,
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
   */
  async dispatch(action: Action): Promise<ActionOutcome> {
    const response = await this.request("POST", "/v1/dispatch", {
      body: actionToRequest(action),
    });

    const data = await response.json();

    if (response.ok) {
      return parseActionOutcome(data);
    } else {
      throw new ApiError(
        data.code ?? "UNKNOWN",
        data.message ?? "Unknown error",
        data.retryable ?? false
      );
    }
  }

  /**
   * Dispatch multiple actions in a single request.
   */
  async dispatchBatch(actions: Action[]): Promise<BatchResult[]> {
    const response = await this.request("POST", "/v1/dispatch/batch", {
      body: actions.map(actionToRequest),
    });

    const data = await response.json();

    if (response.ok) {
      return (data as unknown[]).map(parseBatchResult);
    } else {
      throw new ApiError(
        data.code ?? "UNKNOWN",
        data.message ?? "Unknown error",
        data.retryable ?? false
      );
    }
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
      return parseAuditPage(await response.json());
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
      return parseAuditRecord(await response.json());
    } else if (response.status === 404) {
      return null;
    } else {
      throw new HttpError(response.status, "Failed to get audit record");
    }
  }
}
