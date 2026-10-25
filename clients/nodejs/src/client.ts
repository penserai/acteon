import { parseRegistryProjection, parseRegistryMutationReceipt, type RegistryProjection, type GovernanceRegistryMutationRequest, type GovernanceRegistryProjectionView, type GovernanceRegistryMutationReceipt } from "./governance.js";
import { AGENT_EXECUTION_CONTEXT_HEADER, AGENT_SOURCE_CONTEXT_HEADER, agentExecutionContext, agentPeerAuthorizationReceipt, agentPeerCancelReceipt, agentPeerContinuationReceipt, agentPeerOptions, agentPeerReceipt, agentProviderAbort, agentSource, agentTask, agentServiceBase, agentServiceRpcBase, agentServicePushConfig, agentServicePushConfigPage, agentServiceTaskPage, assertUnboundPeerResponse, type AgentPeerAuthorizationReceipt, type AgentPeerCancelReceipt, type AgentPeerContinuationReceipt, type AgentPeerSelectionOption, type AgentPeerSendReceipt, type AgentServiceA2aFrame, type AgentServiceListOptions, type AgentServicePushConfig, type AgentServicePushConfigInput, type AgentServicePushConfigPage, type AgentServiceReceipt, type AgentServiceStopReceipt, type AgentServiceTaskPage } from "./agent_services.js";
import { parseProviderHistoryReceipt, type ProviderHistoryReceipt, type ProviderReconciliationCorrelation, type ProviderReconciliationRequest } from "./governance.js";
import { parseProviderExecutionHistory, type ProviderExecutionHistory, type ProviderExecutionHistoryWire } from "./governance.js";
import type { WorkforceScopeView, WorkforceChangeRequest } from "./workforce.js";
import type { AbandonFederationDeliveryRequest, FederationDeliveryEvidence, FederationDeliveryEvidencePage, FederationExportReceipt, FederationFinalityCorrelation, FederationImportReceipt, GovernanceScopeView, GovernanceChangeReceipt, PublishFederationTrustRequest, PublishGovernancePermitRequest, ReconcileFederationDeliveryRequest, RevokeFederationTrustRequest, SignedFederationCapabilityCard, SignedFederationEnvelopeDeclaration, SignedFederationInvocationDeclaration, GovernanceInterventionRequest } from "./governance.js";
import { platformRequestParts, type PlatformOperation, type PlatformRequestOptions } from "./platform.js";
/**
 * HTTP client for the Acteon action gateway.
 */

import {
  CredentialIdentity,
  parseCredentialIdentity,
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
  SwarmRunFilter,
  SwarmRunSnapshot,
  ListSwarmRunsResponse,
  swarmRunFilterToParams,
  parseSwarmRunSnapshot,
  parseListSwarmRunsResponse,
  parseListChainsResponse,
  parseChainDetailResponse,
  DagResponse,
  parseDagResponse,
  ChainHistoryResponse,
  parseChainHistoryResponse,
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
  CreateSilenceRequest,
  createSilenceRequestToApi,
  UpdateSilenceRequest,
  updateSilenceRequestToApi,
  Silence,
  parseSilence,
  ListSilencesResponse,
  parseListSilencesResponse,
  CreateTimeIntervalRequest,
  createTimeIntervalRequestToApi,
  UpdateTimeIntervalRequest,
  updateTimeIntervalRequestToApi,
  TimeInterval,
  parseTimeInterval,
  ListTimeIntervalsResponse,
  parseListTimeIntervalsResponse,
  CreateRetentionRequest,
  createRetentionRequestToApi,
  UpdateRetentionRequest,
  updateRetentionRequestToApi,
  RetentionPolicy,
  parseRetentionPolicy,
  ListRetentionResponse,
  parseListRetentionResponse,
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
  AnalyticsQuery,
  AnalyticsResponse,
  analyticsQueryToParams,
  parseAnalyticsResponse,
  CoverageQuery,
  CoverageReport,
  parseCoverageReport,
  SigningKeysResponse,
  parseSigningKeysResponse,
} from "./models.js";
import { ApiError, ConnectionError, HttpError } from "./errors.js";
import {
  A2A_HEADERS,
  A2A_PROTOCOL_VERSION,
  a2aSegment,
  unwrapJsonRpc,
  type JsonRpcReply,
} from "./a2a.js";
import {
  type EnqueueTaskOptions,
  type PollTasksOptions,
  type WorkerTask,
  type WorkerTaskStatus,
  completeTaskBody,
  enqueueTaskBody,
  failTaskBody,
  heartbeatTaskBody,
  parseTaskListResponse,
  parseWorkerTask,
  pollTasksBody,
} from "./queues.js";
import {
  type ExecutionHistory,
  type RecordCheckpointResponse,
  type StartChildOptions,
  type StartWorkflowOptions,
  type WorkflowExecution,
  type WorkflowStatus,
  parseExecutionHistory,
  parseRecordCheckpointResponse,
  parseWorkflowExecution,
  parseWorkflowExecutionListResponse,
  startWorkflowBody,
} from "./workflows.js";
import { readFileSync } from "node:fs";
import { Agent as HttpsAgent } from "node:https";
import {
  type AppendBusConversationMessage,
  type BusAgent,
  type BusApprovalDecision,
  type BusApprovalDecisionResponse,
  type BusApprovalStatus,
  type BusApprovalView,
  type BusConsumeItem,
  type ReconnectConfig,
  type BusConversation,
  type BusLag,
  type BusReplayResponse,
  type BusSchema,
  type BusStreamEnvelopeReceipt,
  type BusStreamItem,
  type BusSubscription,
  type BusToolEnvelopeReceipt,
  type BusToolResultLookup,
  type BusToolResultLookupParams,
  type BusTopic,
  type CreateBusConversation,
  type CreateBusSubscription,
  type CreateBusTopic,
  type PostBusStreamChunk,
  type PostBusStreamEnd,
  type PostBusToolCall,
  type PostBusToolCallOutcome,
  type PostBusToolResult,
  type PublishBusMessage,
  type PublishReceipt,
  type RegisterBusAgent,
  type RegisterBusSchema,
  type SetBusAgentAdminState,
  appendBusConversationMessageBody,
  busApprovalDecisionBody,
  busToolResultLookupParams,
  createBusConversationBody,
  createBusSubscriptionBody,
  createBusTopicBody,
  parseBusAgent,
  parseBusApprovalDecisionResponse,
  parseBusApprovalParkedReceipt,
  parseBusApprovalView,
  parseBusConsumedMessage,
  parseBusConversation,
  parseBusLag,
  parseBusReplayResponse,
  parseBusSchema,
  parseBusStreamEnvelopeReceipt,
  parseBusSubscription,
  parseBusToolEnvelopeReceipt,
  parseBusToolResultLookup,
  parseBusTopic,
  parsePublishReceipt,
  parseStreamChunkEnvelope,
  parseStreamEndEnvelope,
  postBusStreamChunkBody,
  postBusStreamEndBody,
  postBusToolCallBody,
  postBusToolResultBody,
  publishBusMessageBody,
  registerBusAgentBody,
  registerBusSchemaBody,
  setBusAgentAdminStateBody,
} from "./bus_models.js";

/**
 * Configuration options for the Acteon client.
 */
/** Explicit reference to an issued permit; the server validates current authority. */
export interface PermitReference {
  id: string;
  acceptedRevision: number;
}

/** Existing verified authority for one delegated agent-service invocation. */
export interface AgentServiceParentOptions {
  executionContext: string;
  permits: PermitReference[];
}

export interface DispatchOptions {
  dryRun?: boolean;
  permits?: PermitReference[];
}

async function dispatchError(response: Response): Promise<never> {
  const body = await response.text();
  let data: Record<string, unknown> | undefined;
  try { data = JSON.parse(body); } catch { /* preserve non-JSON HTTP errors */ }
  if (data && typeof data.code === "string" && typeof data.message === "string") {
    throw new ApiError(data.code, data.message, data.retryable === true);
  }
  throw new HttpError(response.status, body);
}

function permitHeaders(permits?: PermitReference[]): Record<string, string> | undefined {
  if (permits === undefined) return undefined;
  return { "x-acteon-execution-permits": JSON.stringify(permits.map(p => {
    if (!Number.isSafeInteger(p.acceptedRevision) || p.acceptedRevision <= 0) {
      throw new RangeError("Permit revision must be a positive safe integer");
    }
    return { id: p.id, accepted_revision: p.acceptedRevision };
  })) };
}

export interface ActeonClientOptions {
  /** Request timeout in milliseconds. Default: 30000. */
  timeout?: number;
  /** Optional API key for authentication. */
  apiKey?: string;
  /** Path to a custom CA certificate file (PEM) for server verification. */
  caCertPath?: string;
  /** Path to client certificate file (PEM) for mTLS. */
  clientCertPath?: string;
  /** Path to client private key file (PEM) for mTLS. */
  clientKeyPath?: string;
  /** Skip certificate verification (dev/test only). Default: true. */
  rejectUnauthorized?: boolean;
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
 *     "email",
 *     "send_notification",
 *     { to: "user@example.com", subject: "Hello" }
 *   );
 *   const outcome = await client.dispatch(action);
 *   console.log(`Outcome: ${outcome.type}`);
 * }
 * ```
 */
/**
 * Internal envelope yielded by `connectBusSse`. Either a complete
 * SSE frame or a keep-alive comment (which the bus consumers surface
 * to callers as a liveness signal).
 */
type BusSseEnvelope =
  | { kind: "frame"; event?: string; id?: string; data: string }
  | { kind: "keepAlive" };

function decodeJsonOrPassthrough(data: string): unknown {
  try {
    return JSON.parse(data);
  } catch {
    return data;
  }
}

/**
 * Exponential backoff capped at `maxBackoffMs`. The shift is bounded
 * at 20 so wild attempt counters can't overflow; matches the Rust
 * client's `backoff_for`.
 */
function reconnectBackoffMs(
  attempt: number,
  initialBackoffMs: number,
  maxBackoffMs: number,
): number {
  const shift = Math.min(attempt, 20);
  const exp = initialBackoffMs * Math.pow(2, shift);
  return Math.min(exp, maxBackoffMs);
}

function extractErrorMessage(data: string): string {
  const decoded = decodeJsonOrPassthrough(data);
  if (
    typeof decoded === "object" &&
    decoded !== null &&
    typeof (decoded as Record<string, unknown>).error === "string"
  ) {
    return (decoded as Record<string, unknown>).error as string;
  }
  return data;
}

export class ActeonClient {
  /** Persist a CDC change. Preserve changeId and revision across retries. */
  async ingestTimelineChange(change: Record<string, unknown>): Promise<unknown> {
    return this.platformRequest("timeline_ingest", { body: { change } });
  }
  /** Search current event versions; query.filter accepts the timeline CEL subset. */
  async queryTimeline(query: Record<string, unknown> = {}): Promise<unknown> {
    return this.platformRequest("timeline_query", { body: { query } });
  }
  async getTimelineEvent(source: string, eventId: string): Promise<unknown> {
    return this.platformRequest("timeline_get", { path: { source, eventId } });
  }

  /** Complete finite HTTP API, using wire field names. Never automatically retries. */
  async platformRequest(operation: PlatformOperation, options: PlatformRequestOptions = {}): Promise<unknown> {
    const parts = platformRequestParts(operation, options);
    const response = await this.request(parts.method, parts.path, { params: parts.params, body: options.body, redirect: operation === "governance_mutate_registry" || operation === "governance_registry_projection" ? "error" : "follow" });
    if (!response.ok) throw new HttpError(response.status, await response.text());
    if (response.status === 204) return null;
    return parts.response === "text" ? response.text() : response.json();
  }

  /** Submit once. Persist the receipt in host state; preserve message ID on response loss. */
  async agentServiceSendMessage(agent: string, message: Record<string, unknown>, parent?: AgentServiceParentOptions): Promise<AgentServiceReceipt> {
    const { namespace, tenant } = (await this.identity()).scope;
    const parentHeaders = parent === undefined ? undefined : permitHeaders(parent.permits);
    if (parent !== undefined && (!parentHeaders || parent.permits.length === 0)) throw new Error("delegated agent service invocation requires permits");
    const response = await this.request("POST", agentServiceBase(agent) + "/message:send", { body: { message }, extraHeaders: { ...A2A_HEADERS, ...(parentHeaders ?? {}), ...(parent ? { [AGENT_EXECUTION_CONTEXT_HEADER]: agentExecutionContext(parent.executionContext) } : {}) }, redirect: "error" });
    if (!response.ok) throw new HttpError(response.status, await response.text());
    if (response.headers.get("a2a-version") !== A2A_PROTOCOL_VERSION) throw new Error("agent service response version missing or unsupported");
    const task = agentTask(await response.json(), namespace, tenant);
    return Object.freeze({ namespace, tenant, agent, taskId: task.id as string, sourceContext: agentSource(response.headers.get(AGENT_SOURCE_CONTEXT_HEADER)), task });
  }
  /** Invoke one exact governed agent through official A2A streaming. */
  async *agentServiceSendStreamingMessage(agent: string, message: Record<string, unknown>, parent?: AgentServiceParentOptions): AsyncGenerator<AgentServiceA2aFrame, void, undefined> {
    if (typeof message.messageId !== "string" || message.messageId.length === 0) throw new Error("streaming message requires messageId");
    const parentHeaders = parent === undefined ? undefined : permitHeaders(parent.permits);
    if (parent !== undefined && (!parentHeaders || parent.permits.length === 0)) throw new Error("delegated agent service invocation requires permits");
    const requestId = `stream:${message.messageId}`;
    yield* this.connectAgentA2aSse(
      agentServiceRpcBase(agent),
      { jsonrpc: "2.0", id: requestId, method: "SendStreamingMessage", params: { message } },
      requestId,
      { ...(parentHeaders ?? {}), ...(parent ? { [AGENT_EXECUTION_CONTEXT_HEADER]: agentExecutionContext(parent.executionContext) } : {}) },
      true,
    );
  }
  /** Subscribe to one retained governed Task through official A2A streaming. */
  async *agentServiceSubscribeTask(receipt: AgentServiceReceipt): AsyncGenerator<AgentServiceA2aFrame, void, undefined> {
    const requestId = `subscribe:${receipt.taskId}`;
    yield* this.connectAgentA2aSse(
      agentServiceRpcBase(receipt.agent),
      { jsonrpc: "2.0", id: requestId, method: "SubscribeToTask", params: { id: receipt.taskId } },
      requestId,
      { [AGENT_SOURCE_CONTEXT_HEADER]: agentSource(receipt.sourceContext) },
      false,
    );
  }
  /** Request official A2A cancellation; nonterminal results retain pending finality metadata. */
  async agentServiceCancelTask(receipt: AgentServiceReceipt): Promise<Record<string, unknown>> {
    const requestId = `cancel:${receipt.taskId}`;
    const response = await this.request("POST", agentServiceRpcBase(receipt.agent), {
      body: { jsonrpc: "2.0", id: requestId, method: "CancelTask", params: { id: receipt.taskId } },
      extraHeaders: { ...A2A_HEADERS, [AGENT_SOURCE_CONTEXT_HEADER]: agentSource(receipt.sourceContext) },
      redirect: "error",
    });
    if (!response.ok) throw new HttpError(response.status, await response.text());
    if (response.headers.get("a2a-version") !== A2A_PROTOCOL_VERSION) throw new Error("agent service A2A response version missing or unsupported");
    const value = await response.json() as unknown;
    if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("invalid agent service A2A response");
    const rpc = value as Record<string, unknown>;
    if (rpc.jsonrpc !== "2.0" || rpc.id !== requestId) throw new Error("agent service A2A response request identity mismatch");
    if (rpc.error && typeof rpc.error === "object") {
      const error = rpc.error as Record<string, unknown>;
      throw new Error(`A2A error ${String(error.code ?? "A2A")}: ${String(error.message ?? "cancellation failed")}`);
    }
    if (!rpc.result || typeof rpc.result !== "object" || Array.isArray(rpc.result)) throw new Error("agent service A2A response has no object result");
    const task = rpc.result as Record<string, unknown>;
    if (task.id !== receipt.taskId || !task.status || typeof task.status !== "object" || Array.isArray(task.status)) throw new Error("agent service A2A cancellation task identity mismatch");
    return task;
  }
  /** List only Tasks visible to the current source through one exact governed agent. */
  async agentServiceListTasks(agent: string, options: AgentServiceListOptions = {}): Promise<AgentServiceTaskPage> {
    const { namespace, tenant } = (await this.identity()).scope;
    if (options.pageSize !== undefined && (!Number.isSafeInteger(options.pageSize) || options.pageSize < 1 || options.pageSize > 100)) throw new Error("pageSize must be between 1 and 100");
    if (options.historyLength !== undefined && (!Number.isSafeInteger(options.historyLength) || options.historyLength < 0 || options.historyLength > 2147483647)) throw new Error("historyLength must fit a signed 32-bit integer");
    if (options.pageToken !== undefined && options.pageToken.length > 8192) throw new Error("pageToken is too large");
    const source = options.source;
    if (source !== undefined && (source.namespace !== namespace || source.tenant !== tenant || source.agent !== agent)) throw new Error("agent service ListTasks receipt does not match the route");
    const params: Record<string, unknown> = {};
    if (options.contextId !== undefined) params.contextId = options.contextId;
    if (options.status !== undefined) params.status = options.status;
    if (options.pageSize !== undefined) params.pageSize = options.pageSize;
    if (options.pageToken !== undefined) params.pageToken = options.pageToken;
    if (options.historyLength !== undefined) params.historyLength = options.historyLength;
    if (options.statusTimestampAfter !== undefined) params.statusTimestampAfter = options.statusTimestampAfter;
    if (options.includeArtifacts === true) params.includeArtifacts = true;
    const requestId = `list:${agent}`;
    const response = await this.request("POST", agentServiceRpcBase(agent), {
      body: { jsonrpc: "2.0", id: requestId, method: "ListTasks", params },
      extraHeaders: { ...A2A_HEADERS, ...(source ? { [AGENT_SOURCE_CONTEXT_HEADER]: agentSource(source.sourceContext) } : {}) },
      redirect: "error",
    });
    if (!response.ok) throw new HttpError(response.status, await response.text());
    if (response.headers.get("a2a-version") !== A2A_PROTOCOL_VERSION) throw new Error("agent service A2A response version missing or unsupported");
    const value = await response.json() as unknown;
    if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("invalid agent service A2A response");
    const rpc = value as Record<string, unknown>;
    if (rpc.jsonrpc !== "2.0" || rpc.id !== requestId) throw new Error("agent service A2A response request identity mismatch");
    if (rpc.error && typeof rpc.error === "object") {
      const error = rpc.error as Record<string, unknown>;
      throw new Error(`A2A error ${String(error.code ?? "A2A")}: ${String(error.message ?? "list failed")}`);
    }
    return agentServiceTaskPage(rpc.result);
  }
  /** Create or update one official A2A push configuration under task authority. */
  async agentServiceCreatePushConfig(agent: string, taskId: string, config: AgentServicePushConfigInput, source?: AgentServiceReceipt): Promise<AgentServicePushConfig> {
    const params = this.agentServicePushInput(taskId, config);
    return agentServicePushConfig(await this.agentServicePushRpc(agent, taskId, "CreateTaskPushNotificationConfig", params, source), taskId, config.id);
  }
  /** Read one exact-agent push configuration, including management-only secrets. */
  async agentServiceGetPushConfig(agent: string, taskId: string, configId: string, source?: AgentServiceReceipt): Promise<AgentServicePushConfig> {
    return agentServicePushConfig(await this.agentServicePushRpc(agent, taskId, "GetTaskPushNotificationConfig", { taskId, id: configId }, source), taskId, configId);
  }
  /** List exact-agent push configurations under the original task authority. */
  async agentServiceListPushConfigs(agent: string, taskId: string, options: { readonly pageSize?: number; readonly pageToken?: string; readonly source?: AgentServiceReceipt } = {}): Promise<AgentServicePushConfigPage> {
    if (options.pageSize !== undefined && (!Number.isSafeInteger(options.pageSize) || options.pageSize < 1 || options.pageSize > 100)) throw new Error("pageSize must be between 1 and 100");
    if (options.pageToken !== undefined && options.pageToken.length > 8192) throw new Error("pageToken is too large");
    const params: Record<string, unknown> = { taskId };
    if (options.pageSize !== undefined) params.pageSize = options.pageSize;
    if (options.pageToken !== undefined) params.pageToken = options.pageToken;
    return agentServicePushConfigPage(await this.agentServicePushRpc(agent, taskId, "ListTaskPushNotificationConfigs", params, options.source), taskId);
  }
  /** Delete one exact-agent push configuration. Repeating deletion is safe. */
  async agentServiceDeletePushConfig(agent: string, taskId: string, configId: string, source?: AgentServiceReceipt): Promise<void> {
    const result = await this.agentServicePushRpc(agent, taskId, "DeleteTaskPushNotificationConfig", { taskId, id: configId }, source);
    if (Object.keys(result).length !== 0) throw new Error("agent service A2A delete result is invalid");
  }
  private agentServicePushInput(taskId: string, config: AgentServicePushConfigInput): Record<string, unknown> {
    if (!taskId || !config.url) throw new Error("taskId and push URL must not be empty");
    if (config.token !== undefined && config.token.length > 4096) throw new Error("push token is too large");
    if (config.token !== undefined && config.tokenRef !== undefined) throw new Error("push token and tokenRef are mutually exclusive");
    const params: Record<string, unknown> = { taskId, url: config.url };
    if (config.id !== undefined) params.id = config.id;
    if (config.token !== undefined) params.token = config.token;
    if (config.tokenRef !== undefined) params.tokenRef = config.tokenRef;
    if (config.authentication !== undefined) {
      const auth = config.authentication;
      if (!auth.scheme.trim() || auth.scheme.length > 256 || (auth.credentials !== undefined && auth.credentials.length > 4096) || (auth.credentials !== undefined && auth.credentialsRef !== undefined)) throw new Error("push authentication is invalid");
      params.authentication = { scheme: auth.scheme, ...(auth.credentials !== undefined ? { credentials: auth.credentials } : {}), ...(auth.credentialsRef !== undefined ? { credentialsRef: auth.credentialsRef } : {}) };
    }
    return params;
  }
  private async agentServicePushRpc(agent: string, taskId: string, method: string, params: Record<string, unknown>, source?: AgentServiceReceipt): Promise<Record<string, unknown>> {
    const { namespace, tenant } = (await this.identity()).scope;
    if (source !== undefined && (source.namespace !== namespace || source.tenant !== tenant || source.agent !== agent || source.taskId !== taskId)) throw new Error("agent service push receipt does not match the route or task");
    const requestId = `push:${method}:${taskId}`;
    const response = await this.request("POST", agentServiceRpcBase(agent), { body: { jsonrpc: "2.0", id: requestId, method, params }, extraHeaders: { ...A2A_HEADERS, ...(source ? { [AGENT_SOURCE_CONTEXT_HEADER]: agentSource(source.sourceContext) } : {}) }, redirect: "error" });
    if (!response.ok) throw new HttpError(response.status, await response.text());
    if (response.headers.get("a2a-version") !== A2A_PROTOCOL_VERSION) throw new Error("agent service A2A response version missing or unsupported");
    const value = await response.json() as unknown;
    if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("invalid agent service A2A response");
    const rpc = value as Record<string, unknown>;
    if (rpc.jsonrpc !== "2.0" || rpc.id !== requestId) throw new Error("agent service A2A response request identity mismatch");
    if (rpc.error && typeof rpc.error === "object") {
      const error = rpc.error as Record<string, unknown>;
      throw new Error(`A2A error ${String(error.code ?? "A2A")}: ${String(error.message ?? "push operation failed")}`);
    }
    if (!rpc.result || typeof rpc.result !== "object" || Array.isArray(rpc.result)) throw new Error("agent service A2A response has no object result");
    return rpc.result as Record<string, unknown>;
  }
  /** List current safe registry options. Descriptions are untrusted data. */
  async agentServiceDiscoverPeers(source: AgentServiceReceipt, skill: string): Promise<readonly AgentPeerSelectionOption[]> {
    if (!/^[A-Za-z0-9._-]{1,120}$/.test(skill) || skill === "*") throw new Error("invalid exact peer skill");
    const segment = (value: string) => {
      if (!value || value === "." || value === "..") throw new Error("invalid agent peer path segment");
      return encodeURIComponent(value);
    };
    const path = agentServiceBase(source.agent) + "/tasks/" + segment(source.taskId) + "/peers";
    const response = await this.request("GET", path, { params: new URLSearchParams({ skill }), extraHeaders: A2A_HEADERS, redirect: "error" });
    if (!response.ok) throw new HttpError(response.status, await response.text());
    if (response.headers.get("a2a-version") !== A2A_PROTOCOL_VERSION) throw new Error("agent peer discovery response version missing or unsupported");
    return agentPeerOptions(await response.json(), skill);
  }
  /** Submit from an accepted source task. Authority remains in server state. */
  async agentServiceSendPeer(source: AgentServiceReceipt, target: string, skill: string, message: Record<string, unknown>): Promise<AgentPeerSendReceipt> {
    const segment = (value: string) => {
      if (!value || value === "." || value === "..") throw new Error("invalid agent peer path segment");
      return encodeURIComponent(value);
    };
    const response = await this.request("POST", agentServiceBase(source.agent) + "/tasks/" + segment(source.taskId) + "/peers/" + segment(target) + "/" + segment(skill) + "/message:send", { body: { message }, extraHeaders: A2A_HEADERS, redirect: "error" });
    if (!response.ok) throw new HttpError(response.status, await response.text());
    if (response.headers.get("a2a-version") !== A2A_PROTOCOL_VERSION) throw new Error("agent peer response version missing or unsupported");
    return agentPeerReceipt(await response.json(), source.namespace, source.tenant);
  }
  /** Observe and journal one accepted remote task without resubmitting work. */
  async agentServiceRefreshPeer(source: AgentServiceReceipt, target: string, skill: string, peer: AgentPeerSendReceipt): Promise<AgentPeerSendReceipt> {
    const segment = (value: string) => {
      if (!value || value === "." || value === "..") throw new Error("invalid agent peer path segment");
      return encodeURIComponent(value);
    };
    if (!/^[0-9a-f]{8}-[0-9a-f]{4}-5[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(peer.submissionId)) throw new Error("invalid agent peer submission identity");
    const path = agentServiceBase(source.agent) + "/tasks/" + segment(source.taskId) + "/peers/" + segment(target) + "/" + segment(skill) + "/submissions/" + peer.submissionId + ":refresh";
    const response = await this.request("POST", path, { extraHeaders: A2A_HEADERS, redirect: "error" });
    if (!response.ok) throw new HttpError(response.status, await response.text());
    if (response.headers.get("a2a-version") !== A2A_PROTOCOL_VERSION) throw new Error("agent peer response version missing or unsupported");
    const refreshed = agentPeerReceipt(await response.json(), source.namespace, source.tenant);
    if (refreshed.submissionId !== peer.submissionId) throw new Error("agent peer submission identity mismatch");
    const sameDisposition =
      (peer.status.state === "uncertain" && refreshed.status.state === "uncertain") ||
      (peer.status.state === "accepted" && refreshed.status.state === "accepted" && peer.status.task.id === refreshed.status.task.id) ||
      (peer.status.state === "rejected" && refreshed.status.state === "rejected" && peer.status.code === refreshed.status.code);
    if (!sameDisposition) throw new Error("agent peer refresh changed durable disposition");
    return refreshed;
  }
  /** Persist and deliver at most one remote cancellation; never retries an ambiguous result. */
  async agentServiceCancelPeer(source: AgentServiceReceipt, target: string, skill: string, peer: AgentPeerSendReceipt): Promise<AgentPeerCancelReceipt> {
    const segment = (value: string) => {
      if (!value || value === "." || value === "..") throw new Error("invalid agent peer path segment");
      return encodeURIComponent(value);
    };
    if (peer.status.state !== "accepted" || !/^[0-9a-f]{8}-[0-9a-f]{4}-5[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(peer.submissionId)) throw new Error("agent peer cancellation requires an accepted peer receipt");
    agentTask(peer.status.task, source.namespace, source.tenant);
    const path = agentServiceBase(source.agent) + "/tasks/" + segment(source.taskId) + "/peers/" + segment(target) + "/" + segment(skill) + "/submissions/" + peer.submissionId + ":cancel";
    const response = await this.request("POST", path, { extraHeaders: A2A_HEADERS, redirect: "error" });
    if (!response.ok) throw new HttpError(response.status, await response.text());
    if (response.headers.get("a2a-version") !== A2A_PROTOCOL_VERSION) throw new Error("agent peer cancellation response version missing or unsupported");
    return agentPeerCancelReceipt(await response.json(), source.namespace, source.tenant, peer);
  }
  /** Deliver exactly one unbound user response to an accepted peer challenge. */
  async agentServiceContinuePeer(source: AgentServiceReceipt, target: string, skill: string, peer: AgentPeerSendReceipt, message: Record<string, unknown>): Promise<AgentPeerContinuationReceipt> {
    const segment = (value: string) => {
      if (!value || value === "." || value === "..") throw new Error("invalid agent peer path segment");
      return encodeURIComponent(value);
    };
    if (peer.status.state !== "accepted" || !/^[0-9a-f]{8}-[0-9a-f]{4}-5[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(peer.submissionId)) throw new Error("agent peer continuation requires an accepted peer receipt");
    agentTask(peer.status.task, source.namespace, source.tenant);
    assertUnboundPeerResponse(message);
    const path = agentServiceBase(source.agent) + "/tasks/" + segment(source.taskId) + "/peers/" + segment(target) + "/" + segment(skill) + "/submissions/" + peer.submissionId + "/message:send";
    const response = await this.request("POST", path, { body: { message }, extraHeaders: A2A_HEADERS, redirect: "error" });
    if (!response.ok) throw new HttpError(response.status, await response.text());
    if (response.headers.get("a2a-version") !== A2A_PROTOCOL_VERSION) throw new Error("agent peer continuation response version missing or unsupported");
    return agentPeerContinuationReceipt(await response.json(), source.namespace, source.tenant, peer);
  }
  /** Resolve one exact remote authorization challenge; credentials stay at the target. */
  async agentServiceAuthorizePeer(source: AgentServiceReceipt, target: string, skill: string, peer: AgentPeerSendReceipt, challengeId: string): Promise<AgentPeerAuthorizationReceipt> {
    const segment = (value: string) => {
      if (!value || value === "." || value === "..") throw new Error("invalid agent peer path segment");
      return encodeURIComponent(value);
    };
    if (peer.status.state !== "accepted" || !/^[0-9a-f]{8}-[0-9a-f]{4}-5[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(peer.submissionId) || !challengeId || challengeId.length > 1024 || challengeId.trim() !== challengeId || challengeId === "*" || /\p{Cc}/u.test(challengeId)) throw new Error("agent peer authorization requires an accepted peer and challenge");
    agentTask(peer.status.task, source.namespace, source.tenant);
    const path = agentServiceBase(source.agent) + "/tasks/" + segment(source.taskId) + "/peers/" + segment(target) + "/" + segment(skill) + "/submissions/" + peer.submissionId + "/authorization:resolve";
    const response = await this.request("POST", path, { body: { challengeId }, extraHeaders: A2A_HEADERS, redirect: "error" });
    if (!response.ok) throw new HttpError(response.status, await response.text());
    if (response.headers.get("a2a-version") !== A2A_PROTOCOL_VERSION) throw new Error("agent peer authorization response version missing or unsupported");
    return agentPeerAuthorizationReceipt(await response.json(), source.namespace, source.tenant, peer, challengeId);
  }
  /** Stop future starts for the original job; retry explicitly with the same receipt on response loss. */
  async agentServiceStopTask(receipt: AgentServiceReceipt): Promise<AgentServiceStopReceipt> {
    const taskId = receipt.taskId;
    if (!taskId || taskId === "." || taskId === "..") throw new Error("invalid agent service task ID");
    const response = await this.request("POST", agentServiceBase(receipt.agent) + "/tasks/" + encodeURIComponent(taskId) + "/stop", { extraHeaders: { ...A2A_HEADERS, [AGENT_SOURCE_CONTEXT_HEADER]: agentSource(receipt.sourceContext) }, redirect: "error" });
    if (!response.ok) throw new HttpError(response.status, await response.text());
    if (response.headers.get("a2a-version") !== A2A_PROTOCOL_VERSION) throw new Error("agent service response version missing or unsupported");
    const value = await response.json() as { task?: unknown; future_starts_blocked?: unknown; cancellation_confirmed?: unknown; provider_abort?: unknown } | null;
    if (!value || value.future_starts_blocked !== true || typeof value.cancellation_confirmed !== "boolean") throw new Error("agent service stop acknowledgement missing or malformed");
    const providerAbort = agentProviderAbort(value.provider_abort);
    const task = agentTask(value.task, receipt.namespace, receipt.tenant, taskId);
    if (value.cancellation_confirmed && (task.status as { state?: unknown } | undefined)?.state !== "canceled") throw new Error("agent service stop acknowledgement missing or malformed");
    return Object.freeze({ task, futureStartsBlocked: true as const, cancellationConfirmed: value.cancellation_confirmed, ...(providerAbort ? { providerAbort } : {}) });
  }

  /** Observe one retained job; never starts work or changes global headers. */
  async agentServiceGetTask(receipt: AgentServiceReceipt): Promise<Record<string, unknown>> {
    const taskId = receipt.taskId;
    if (!taskId || taskId === "." || taskId === "..") throw new Error("invalid agent service task ID");
    const response = await this.request("GET", agentServiceBase(receipt.agent) + "/tasks/" + encodeURIComponent(taskId), { extraHeaders: { ...A2A_HEADERS, [AGENT_SOURCE_CONTEXT_HEADER]: agentSource(receipt.sourceContext) }, redirect: "error" });
    if (!response.ok) throw new HttpError(response.status, await response.text());
    if (response.headers.get("a2a-version") !== A2A_PROTOCOL_VERSION) throw new Error("agent service response version missing or unsupported");
    return agentTask(await response.json(), receipt.namespace, receipt.tenant, taskId);
  }
  /** Continue the original retained task using its host-owned source context. */
  async agentServiceContinueTask(receipt: AgentServiceReceipt, message: Record<string, unknown>): Promise<Record<string, unknown>> {
    const taskId = receipt.taskId;
    if (!taskId || taskId === "." || taskId === "..") throw new Error("invalid agent service task ID");
    const response = await this.request("POST", agentServiceBase(receipt.agent) + "/tasks/" + encodeURIComponent(taskId) + "/message:send", { body: { message }, extraHeaders: { ...A2A_HEADERS, [AGENT_SOURCE_CONTEXT_HEADER]: agentSource(receipt.sourceContext) }, redirect: "error" });
    if (!response.ok) throw new HttpError(response.status, await response.text());
    if (response.headers.get("a2a-version") !== A2A_PROTOCOL_VERSION) throw new Error("agent service response version missing or unsupported");
    return agentTask(await response.json(), receipt.namespace, receipt.tenant, taskId);
  }
  /** Open the configured authorization profile with an opaque verifier handle. */
  async agentServiceRequestAuthorization(agent: string, taskId: string, authorizationRequestId: string): Promise<Record<string, unknown>> {
    const { namespace, tenant } = (await this.identity()).scope;
    if (!taskId || taskId === "." || taskId === "..") throw new Error("invalid agent service task ID");
    const response = await this.request("POST", agentServiceBase(agent) + "/tasks/" + encodeURIComponent(taskId) + "/authorization:request", { body: { authorizationRequestId }, extraHeaders: A2A_HEADERS, redirect: "error" });
    if (!response.ok) throw new HttpError(response.status, await response.text());
    if (response.headers.get("a2a-version") !== A2A_PROTOCOL_VERSION) throw new Error("agent service response version missing or unsupported");
    return agentTask(await response.json(), namespace, tenant, taskId);
  }
  /** Resolve one challenge without sending credential material through Acteon. */
  async agentServiceResolveAuthorization(receipt: AgentServiceReceipt, challengeId: string): Promise<Record<string, unknown>> {
    const taskId = receipt.taskId;
    if (!taskId || taskId === "." || taskId === "..") throw new Error("invalid agent service task ID");
    const response = await this.request("POST", agentServiceBase(receipt.agent) + "/tasks/" + encodeURIComponent(taskId) + "/authorization:resolve", { body: { challengeId }, extraHeaders: { ...A2A_HEADERS, [AGENT_SOURCE_CONTEXT_HEADER]: agentSource(receipt.sourceContext) }, redirect: "error" });
    if (!response.ok) throw new HttpError(response.status, await response.text());
    if (response.headers.get("a2a-version") !== A2A_PROTOCOL_VERSION) throw new Error("agent service response version missing or unsupported");
    return agentTask(await response.json(), receipt.namespace, receipt.tenant, taskId);
  }

  /** Inspect only the routes and permits within current independent management policy. */
  async workforce(): Promise<WorkforceScopeView> {
    return await this.platformRequest("workforce_inspect", { query: { } }) as WorkforceScopeView;
  }
  async changeWorkforce(request: WorkforceChangeRequest): Promise<GovernanceChangeReceipt> {
    return await this.platformRequest("workforce_change", { body: request }) as GovernanceChangeReceipt;
  }

  async registryProjection(agentId: string, projection: RegistryProjection): Promise<GovernanceRegistryProjectionView> {
    const { namespace, tenant } = (await this.identity()).scope;
    const data = await this.platformRequest("governance_registry_projection", { path: { agent_id: agentId }, query: {projection } });
    return parseRegistryProjection(data, namespace, tenant, agentId, projection);
  }
  /** Send once; retain the same request and change ID for explicit recovery. */
  async mutateRegistry(request: GovernanceRegistryMutationRequest): Promise<GovernanceRegistryMutationReceipt> {
    const scope = (await this.identity()).scope;
    const sent = structuredClone(request);
    const data = await this.platformRequest("governance_mutate_registry", { body: sent });
    return parseRegistryMutationReceipt(data, sent, scope);
  }
  async governance(): Promise<GovernanceScopeView> {
    return await this.platformRequest("governance_inspect", { query: { } }) as GovernanceScopeView;
  }
  /** Read retained evidence under independent history management authority. */
  async providerExecutionHistory(executionId: string): Promise<ProviderExecutionHistory> {
    const data = await this.platformRequest("governance_provider_history", {
      path: { execution_id: executionId }, query: { },
    });
    return parseProviderExecutionHistory(data as ProviderExecutionHistoryWire);
  }
  async providerReconciliationCorrelation(executionId: string, ordinal: number): Promise<ProviderReconciliationCorrelation> {
    return await this.platformRequest("governance_reconciliation_correlation", {
      path: { execution_id: executionId, ordinal: String(ordinal) }, query: { },
    }) as ProviderReconciliationCorrelation;
  }
  /** Accept independently qualified finality; never automatically retries. */
  async acceptProviderReconciliation(executionId: string, ordinal: number, request: ProviderReconciliationRequest): Promise<ProviderHistoryReceipt> {
    const data = await this.platformRequest("governance_accept_reconciliation", {
      path: { execution_id: executionId, ordinal: String(ordinal) }, query: { }, body: request,
    });
    return parseProviderHistoryReceipt(data as ProviderExecutionHistoryWire["receipt"]);
  }
  async publishGovernancePermit(request: PublishGovernancePermitRequest): Promise<GovernanceChangeReceipt> {
    return await this.platformRequest("governance_publish_permit", { body: request }) as GovernanceChangeReceipt;
  }
  async publishFederationTrust(request: PublishFederationTrustRequest): Promise<GovernanceChangeReceipt> {
    return await this.platformRequest("governance_publish_federation_trust", { body: request }) as GovernanceChangeReceipt;
  }
  async revokeFederationTrust(request: RevokeFederationTrustRequest): Promise<GovernanceChangeReceipt> {
    return await this.platformRequest("governance_revoke_federation_trust", { body: request }) as GovernanceChangeReceipt;
  }
  /** List sanitized outbound delivery evidence under reconciliation authority. */
  async federationDeliveryEvidence(after?: string, limit?: number): Promise<FederationDeliveryEvidencePage> {
    const page = await this.platformRequest("governance_federation_delivery_evidence", {
      query: {...(after ? { after } : {}), ...(limit === undefined ? {} : { limit: String(limit) }) },
    }) as FederationDeliveryEvidencePage;
    if (!Array.isArray(page.deliveries) || page.deliveries.some((delivery) => !delivery.delivery_id || !delivery.parent_execution_id)) {
      throw new Error("federation delivery evidence identity mismatch");
    }
    return page;
  }
  /** Stop local recovery without claiming peer success or rejection. */
  async abandonFederationDelivery(deliveryId: string, request: AbandonFederationDeliveryRequest): Promise<FederationDeliveryEvidence> {
    return await this.platformRequest("governance_abandon_federation_delivery", {
      path: { delivery_id: deliveryId }, body: request,
    }) as FederationDeliveryEvidence;
  }
  /** Obtain immutable correlation for the currently ambiguous peer stage. */
  async federationFinalityCorrelation(deliveryId: string): Promise<FederationFinalityCorrelation> {
    return await this.platformRequest("governance_federation_finality_correlation", {
      path: { delivery_id: deliveryId }, query: { },
    }) as FederationFinalityCorrelation;
  }
  /** Accept qualified peer finality without sending or replaying the delivery. */
  async reconcileFederationDelivery(deliveryId: string, request: ReconcileFederationDeliveryRequest): Promise<FederationDeliveryEvidence> {
    return await this.platformRequest("governance_reconcile_federation_delivery", {
      path: { delivery_id: deliveryId }, body: request,
    }) as FederationDeliveryEvidence;
  }
  /** Authenticate as the configured peer and retain authority without starting execution. */
  async federationCapabilities(agent: string): Promise<SignedFederationCapabilityCard> {
    const card = await this.platformRequest("federation_capabilities", {
      path: {agent },
    }) as SignedFederationCapabilityCard;
    if (!card.payload || card.signature.length !== 128) {
      throw new Error("invalid signed federation capability card");
    }
    return card;
  }

  /** Authenticate as the configured peer and retain authority without starting execution. */
  async importFederationEnvelope(envelope: SignedFederationEnvelopeDeclaration): Promise<FederationImportReceipt> {
    const { namespace, tenant } = (await this.identity()).scope;
    const receipt = await this.platformRequest("federation_import_envelope", {
      path: { }, body: { envelope },
    }) as FederationImportReceipt;
    if (receipt.namespace !== namespace || receipt.tenant !== tenant || receipt.execution_started !== false) {
      throw new Error("federation import receipt identity or execution state mismatch");
    }
    return receipt;
  }
  /** Execute or replay one exact message under signed imported federation authority. */
  async invokeFederatedAgent(agent: string, invocation: SignedFederationInvocationDeclaration, message: Record<string, unknown>): Promise<Record<string, unknown>> {
    const { namespace, tenant } = (await this.identity()).scope;
    const task = await this.platformRequest("federation_invoke_agent", {
      path: {agent }, body: { invocation, message },
    }) as Record<string, unknown>;
    if (task.namespace !== namespace || task.tenant !== tenant || typeof task.id !== "string" || task.id.length === 0) {
      throw new Error("federated task receipt identity mismatch");
    }
    return task;
  }
  /** Submit one host-qualified cross-domain export from an accepted agent task. */
  async exportFederatedMessage(agent: string, taskId: string, profile: string, message: Record<string, unknown>): Promise<FederationExportReceipt> {
    return await this.federationExportRequest("federation_export_message", agent, taskId, profile, message);
  }
  /** Observe retained delivery evidence without issuing network traffic. */
  async observeFederatedExport(agent: string, taskId: string, profile: string, message: Record<string, unknown>): Promise<FederationExportReceipt> {
    return await this.federationExportRequest("federation_observe_export_message", agent, taskId, profile, message);
  }
  /** Explicitly replay the exact request through a qualified idempotent destination. */
  async replayFederatedExport(agent: string, taskId: string, profile: string, message: Record<string, unknown>): Promise<FederationExportReceipt> {
    return await this.federationExportRequest("federation_replay_export_message", agent, taskId, profile, message);
  }
  private async federationExportRequest(operation: "federation_export_message" | "federation_observe_export_message" | "federation_replay_export_message", agent: string, taskId: string, profile: string, message: Record<string, unknown>): Promise<FederationExportReceipt> {
    const receipt = await this.platformRequest(operation, {
      path: {agent, task_id: taskId, profile }, body: { message },
    }) as FederationExportReceipt;
    if (!receipt.attempt_id || !receipt.delivery_id || !["in_flight", "settled", "uncertain"].includes(receipt.attempt_status) || !receipt.delivery || typeof receipt.delivery.status !== "string") {
      throw new Error("federation export receipt identity mismatch");
    }
    return receipt;
  }
  async interveneGovernance(request: GovernanceInterventionRequest): Promise<GovernanceChangeReceipt> {
    return await this.platformRequest("governance_intervene", { body: request }) as GovernanceChangeReceipt;
  }

  private readonly baseUrl: string;
  private readonly timeout: number;
  private readonly apiKey?: string;
  private readonly dispatcher?: any;

  constructor(baseUrl: string, options: ActeonClientOptions = {}) {
    this.baseUrl = baseUrl.replace(/\/$/, "");
    this.timeout = options.timeout ?? 30000;
    this.apiKey = options.apiKey;

    if (options.caCertPath || options.clientCertPath || options.rejectUnauthorized === false) {
      const agentOptions: Record<string, unknown> = { keepAlive: true };
      if (options.caCertPath) {
        agentOptions.ca = readFileSync(options.caCertPath);
      }
      if (options.clientCertPath && options.clientKeyPath) {
        agentOptions.cert = readFileSync(options.clientCertPath);
        agentOptions.key = readFileSync(options.clientKeyPath);
      }
      if (options.rejectUnauthorized === false) {
        agentOptions.rejectUnauthorized = false;
      }
      this.dispatcher = new HttpsAgent(agentOptions);
    }
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
      extraHeaders?: Record<string, string>;
      redirect?: "error" | "follow" | "manual";
      /**
       * When true, suppress the `Authorization` header. Used by the
       * A2A unauthenticated discovery endpoint
       * (`/.well-known/agent.json`), which per spec is anonymous.
       */
      skipAuth?: boolean;
    }
  ): Promise<Response> {
    let url = `${this.baseUrl}${path}`;
    if (options?.params?.size) {
      url += `?${options.params.toString()}`;
    }

    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), this.timeout);

    try {
      const baseHeaders = options?.skipAuth
        ? { "Content-Type": "application/json" }
        : this.headers();
      const headers: Record<string, string> = {
        ...baseHeaders,
        ...(options?.extraHeaders ?? {}),
      };
      const fetchOptions: RequestInit & { dispatcher?: unknown } = {
        method,
        headers,
        redirect: options?.redirect ?? "follow",
        body: options?.body !== undefined ? JSON.stringify(options.body) : undefined,
        signal: controller.signal,
      };
      if (this.dispatcher) {
        (fetchOptions as Record<string, unknown>).dispatcher = this.dispatcher;
      }
      const response = await fetch(url, fetchOptions);
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

  /** Inspect this credential's stable principal binding. */
  async identity(): Promise<CredentialIdentity> {
    return parseCredentialIdentity(await this.platformRequest("auth_identity") as Record<string, unknown>);
  }

  /** Check if the server is healthy. */
  async health(): Promise<boolean> {
    try {
      const response = await this.request("GET", "/health");
      return response.ok;
    } catch {
      return false;
    }
  }

  // =========================================================================
  // Signing key discovery (JWKS-style)
  // =========================================================================

  /**
   * Fetch the server's active signing keyring.
   *
   * Calls `GET /.well-known/acteon-signing-keys`, a public,
   * unauthenticated endpoint that publishes the public half of
   * every `(signer_id, kid)` pair the server will accept
   * signatures from. Useful for:
   *
   * - verifying dispatched actions independently without pinning
   *   public keys at deploy time
   * - detecting a rotation in progress — a signer with more than
   *   one entry means the operator is staging a rotation and the
   *   client should start sending the new `kid`
   *
   * Returns a response with an empty `keys` array when signing is
   * disabled on the server.
   *
   * @throws {HttpError} If the server returns a non-2xx status.
   */
  async fetchSigningKeys(): Promise<SigningKeysResponse> {
    const response = await this.request(
      "GET",
      "/.well-known/acteon-signing-keys",
    );
    if (!response.ok) {
      throw new HttpError(
        response.status,
        `Failed to fetch signing keys: ${response.status}`,
      );
    }
    // Wrap both JSON decode failures (proxy returns 200 OK with an
    // HTML body) and shape validation failures from
    // parseSigningKeysResponse in a typed ConnectionError so
    // callers see "malformed signing keys response" instead of a
    // raw SyntaxError.
    let data: Record<string, unknown>;
    try {
      data = (await response.json()) as Record<string, unknown>;
    } catch (e) {
      throw new ConnectionError(
        `malformed signing keys response: ${(e as Error).message}`,
      );
    }
    try {
      return parseSigningKeysResponse(data);
    } catch (e) {
      // parseSigningKeysResponse throws plain Errors with a
      // "malformed signing keys response:" prefix; rewrap as a
      // typed ConnectionError so `catch (e: ConnectionError)`
      // works on the caller side.
      throw new ConnectionError((e as Error).message);
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
    options?: DispatchOptions
  ): Promise<ActionOutcome> {
    const params =
      options?.dryRun ? new URLSearchParams({ dry_run: "true" }) : undefined;
    const response = await this.request("POST", "/v1/dispatch", {
      body: actionToRequest(action),
      params,
      extraHeaders: permitHeaders(options?.permits),
    });

    if (response.ok) {
      return parseActionOutcome((await response.json()) as Record<string, unknown>);
    } else {
      return dispatchError(response);
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
    options?: DispatchOptions
  ): Promise<BatchResult[]> {
    const params =
      options?.dryRun ? new URLSearchParams({ dry_run: "true" }) : undefined;
    const response = await this.request("POST", "/v1/dispatch/batch", {
      body: actions.map(actionToRequest),
      params,
      extraHeaders: permitHeaders(options?.permits),
    });

    if (response.ok) {
      const data = (await response.json()) as unknown[];
      return data.map((item) => parseBatchResult(item as Record<string, unknown>));
    } else {
      return dispatchError(response);
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
   * List events filtered by optional status within the credential context.
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
  async getEvent(fingerprint: string
  ): Promise<EventState | null> {
    const params = new URLSearchParams();

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
  async transitionEvent(fingerprint: string, toState: string
  ): Promise<TransitionResponse> {
    const response = await this.request("PUT", `/v1/events/${fingerprint}/transition`, {
      body: {to: toState },
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
   * Approve a pending action by ID in the authenticated credential context.
   * Requires an authenticated operator or admin credential.
   */
  async approve(id: string): Promise<ApprovalActionResponse> {
    const response = await this.request("POST", `/v1/approvals/${id}/approve`, {});

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
   * Reject a pending action by ID in the authenticated credential context.
   * Requires an authenticated operator or admin credential.
   */
  async reject(id: string): Promise<ApprovalActionResponse> {
    const response = await this.request("POST", `/v1/approvals/${id}/reject`, {});

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
   * Get the status of an approval by ID in the authenticated credential context.
   * Returns null if not found or expired.
   */
  async getApproval(id: string): Promise<ApprovalStatus | null> {
    const response = await this.request("GET", `/v1/approvals/${id}`, {});

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
   * List pending approvals within the credential context.
   * Requires authentication.
   */
  async listApprovals(): Promise<ApprovalListResponse> {
    const params = new URLSearchParams();

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
  async getRecurring(recurringId: string): Promise<RecurringDetail | null> {
    const params = new URLSearchParams();

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
  async deleteRecurring(recurringId: string): Promise<void> {
    const params = new URLSearchParams();

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
  async pauseRecurring(recurringId: string): Promise<RecurringDetail> {
    const response = await this.request("POST", `/v1/recurring/${recurringId}/pause`, {
      body: { },
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
  async resumeRecurring(recurringId: string): Promise<RecurringDetail> {
    const response = await this.request("POST", `/v1/recurring/${recurringId}/resume`, {
      body: { },
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
  // Swarm runs
  // =========================================================================

  /**
   * List swarm runs tracked by the server-side registry.
   */
  async listSwarmRuns(filter?: SwarmRunFilter): Promise<ListSwarmRunsResponse> {
    const params = filter ? swarmRunFilterToParams(filter) : undefined;
    const response = await this.request("GET", "/v1/swarm/runs", { params });
    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseListSwarmRunsResponse(data);
    }
    throw new HttpError(response.status, "Failed to list swarm runs");
  }

  /**
   * Fetch a single swarm run snapshot. Returns `null` if unknown.
   */
  async getSwarmRun(runId: string): Promise<SwarmRunSnapshot | null> {
    // Encode so a maliciously crafted runId containing '/', '?', or '#'
    // can't escape the path segment into query/fragment territory.
    const encoded = encodeURIComponent(runId);
    const response = await this.request("GET", `/v1/swarm/runs/${encoded}`);
    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseSwarmRunSnapshot(data);
    }
    if (response.status === 404) {
      return null;
    }
    throw new HttpError(response.status, "Failed to fetch swarm run");
  }

  /**
   * Request cancellation of an inflight swarm run.
   */
  async cancelSwarmRun(runId: string): Promise<SwarmRunSnapshot | null> {
    const encoded = encodeURIComponent(runId);
    const response = await this.request("POST", `/v1/swarm/runs/${encoded}/cancel`);
    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseSwarmRunSnapshot(data);
    }
    if (response.status === 404) {
      return null;
    }
    throw new HttpError(response.status, "Failed to cancel swarm run");
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
   * List quota policies, filtered within the credential context by
   * and provider scope. Pass ``"generic"`` for ``provider`` to match
   * only policies without a provider scope, or a provider name
   * (e.g. ``"slack"``) to match only per-provider policies.
   */
  async listQuotas(provider?: string, principal?: string
  ): Promise<ListQuotasResponse> {
    const params = new URLSearchParams();

    if (provider !== undefined) params.set("provider", provider);
    if (principal !== undefined) params.set("principal", principal);
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
  async deleteQuota(quotaId: string): Promise<void> {
    const params = new URLSearchParams();

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
  // Silences
  // =========================================================================

  /**
   * Create a silence. Supply either `endsAt` or `durationSeconds`.
   */
  async createSilence(req: CreateSilenceRequest): Promise<Silence> {
    const response = await this.request("POST", "/v1/silences", {
      body: createSilenceRequestToApi(req),
    });

    if (response.status === 201) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseSilence(data);
    } else {
      const data = (await response.json()) as Record<string, unknown>;
      throw new ApiError(
        (data.code as string) ?? "UNKNOWN",
        ((data.message ?? data.error) as string) ?? "Unknown error",
        (data.retryable as boolean) ?? false
      );
    }
  }

  /**
   * List silences, filtered within the credential context by or expiry.
   */
  async listSilences(includeExpired = false
  ): Promise<ListSilencesResponse> {
    const params = new URLSearchParams();

    if (includeExpired) params.set("include_expired", "true");
    const response = await this.request("GET", "/v1/silences", {
      params: params.toString() ? params : undefined,
    });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseListSilencesResponse(data);
    } else {
      throw new HttpError(response.status, "Failed to list silences");
    }
  }

  /**
   * Fetch a single silence by ID. Returns `null` if not found.
   */
  async getSilence(silenceId: string): Promise<Silence | null> {
    const response = await this.request(
      "GET",
      `/v1/silences/${encodeURIComponent(silenceId)}`,
    );

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseSilence(data);
    } else if (response.status === 404) {
      return null;
    } else {
      throw new HttpError(response.status, "Failed to get silence");
    }
  }

  /**
   * Extend a silence or edit its comment. Matchers are immutable —
   * to change them, expire the silence and create a new one.
   */
  async updateSilence(
    silenceId: string,
    update: UpdateSilenceRequest,
  ): Promise<Silence> {
    const response = await this.request(
      "PUT",
      `/v1/silences/${encodeURIComponent(silenceId)}`,
      { body: updateSilenceRequestToApi(update) },
    );

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseSilence(data);
    } else if (response.status === 404) {
      throw new HttpError(404, `Silence not found: ${silenceId}`);
    } else {
      const data = (await response.json()) as Record<string, unknown>;
      throw new ApiError(
        (data.code as string) ?? "UNKNOWN",
        ((data.message ?? data.error) as string) ?? "Unknown error",
        (data.retryable as boolean) ?? false
      );
    }
  }

  /**
   * Expire a silence immediately. Soft-expires by setting
   * `endsAt = now`; the record stays queryable for audit purposes.
   */
  async deleteSilence(silenceId: string): Promise<void> {
    const response = await this.request(
      "DELETE",
      `/v1/silences/${encodeURIComponent(silenceId)}`,
    );

    if (response.status === 204) {
      return;
    } else if (response.status === 404) {
      throw new HttpError(404, `Silence not found: ${silenceId}`);
    } else {
      throw new HttpError(response.status, "Failed to delete silence");
    }
  }

  // =========================================================================
  // Time Intervals
  // =========================================================================

  async createTimeInterval(
    req: CreateTimeIntervalRequest,
  ): Promise<TimeInterval> {
    const response = await this.request("POST", "/v1/time-intervals", {
      body: createTimeIntervalRequestToApi(req),
    });
    if (response.status === 201) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseTimeInterval(data);
    }
    const data = (await response.json()) as Record<string, unknown>;
    throw new ApiError(
      (data.code as string) ?? "UNKNOWN",
      ((data.message ?? data.error) as string) ?? "Unknown error",
      (data.retryable as boolean) ?? false,
    );
  }

  async listTimeIntervals(
  ): Promise<ListTimeIntervalsResponse> {
    const params = new URLSearchParams();

    const response = await this.request("GET", "/v1/time-intervals", {
      params: params.toString() ? params : undefined,
    });
    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseListTimeIntervalsResponse(data);
    }
    throw new HttpError(response.status, "Failed to list time intervals");
  }

  async getTimeInterval(name: string
  ): Promise<TimeInterval | null> {
    const response = await this.request(
      "GET",
      `/v1/time-intervals/${encodeURIComponent(name)}`,
    );
    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseTimeInterval(data);
    } else if (response.status === 404) {
      return null;
    } else {
      throw new HttpError(response.status, "Failed to get time interval");
    }
  }

  async updateTimeInterval(name: string, update: UpdateTimeIntervalRequest
  ): Promise<TimeInterval> {
    const response = await this.request(
      "PUT",
      `/v1/time-intervals/${encodeURIComponent(name)}`,
      { body: updateTimeIntervalRequestToApi(update) },
    );
    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseTimeInterval(data);
    } else if (response.status === 404) {
      throw new HttpError(404, `Time interval not found: ${name}`);
    } else {
      const data = (await response.json()) as Record<string, unknown>;
      throw new ApiError(
        (data.code as string) ?? "UNKNOWN",
        ((data.message ?? data.error) as string) ?? "Unknown error",
        (data.retryable as boolean) ?? false,
      );
    }
  }

  async deleteTimeInterval(name: string
  ): Promise<void> {
    const response = await this.request(
      "DELETE",
      `/v1/time-intervals/${encodeURIComponent(name)}`,
    );
    if (response.status === 204) {
      return;
    } else if (response.status === 404) {
      throw new HttpError(404, `Time interval not found: ${name}`);
    } else {
      throw new HttpError(response.status, "Failed to delete time interval");
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
  async listRetention(limit?: number, offset?: number): Promise<ListRetentionResponse> {
    const params = new URLSearchParams();

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
  async listTemplates(): Promise<ListTemplatesResponse> {
    const params = new URLSearchParams();

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
  async listProfiles(): Promise<ListProfilesResponse> {
    const params = new URLSearchParams();

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
   * List chain executions filtered by optional status within the credential context.
   */
  async listChains(status?: string): Promise<ListChainsResponse> {
    const params = new URLSearchParams();

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
  async getChain(chainId: string): Promise<ChainDetailResponse | null> {
    const params = new URLSearchParams();

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
  async cancelChain(chainId: string, reason?: string, cancelledBy?: string
  ): Promise<ChainDetailResponse> {
    const body: Record<string, unknown> = { };
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
  async getChainDag(chainId: string): Promise<DagResponse> {
    const params = new URLSearchParams();

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

  /**
   * Get the retry history for a chain execution.
   */
  async getChainHistory(chainId: string): Promise<ChainHistoryResponse> {
    const params = new URLSearchParams();

    const response = await this.request("GET", `/v1/chains/${chainId}/history`, { params });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseChainHistoryResponse(data);
    } else if (response.status === 404) {
      throw new HttpError(404, `Chain not found: ${chainId}`);
    } else {
      throw new HttpError(response.status, "Failed to get chain history");
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
  // Analytics
  // =========================================================================

  /**
   * Query analytics data.
   *
   * @param query - Analytics query parameters including the required metric.
   * @returns Analytics response with time-series buckets and/or top entries.
   */
  async queryAnalytics(query: AnalyticsQuery): Promise<AnalyticsResponse> {
    const params = analyticsQueryToParams(query);
    const response = await this.request("GET", "/v1/analytics", { params });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseAnalyticsResponse(data);
    } else {
      throw new HttpError(response.status, "Failed to query analytics");
    }
  }

  // =========================================================================
  // Rule Coverage
  // =========================================================================

  /**
   * Analyze rule coverage by querying the server's aggregation endpoint.
   *
   * The server groups audit records by (namespace, tenant, provider,
   * action_type, matched_rule) and cross-references the result with the
   * currently-loaded rule set. No raw audit records are transferred over
   * the wire.
   */
  async rulesCoverage(query?: CoverageQuery): Promise<CoverageReport> {
    const params = new URLSearchParams();

    if (query?.from) params.set("from", query.from);
    if (query?.to) params.set("to", query.to);

    const response = await this.request("GET", "/v1/rules/coverage", { params });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseCoverageReport(data);
    } else {
      throw new HttpError(response.status, "Failed to get rule coverage");
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
   * @param options - Optional includeHistory setting.
   *
   * @example
   * ```typescript
   * for await (const event of client.subscribe("chain", "chain-42")) {
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
   * @param options - Optional filters for actionType, outcome, eventType,
   *   chainId, groupId, actionId, and lastEventId for reconnection catch-up.
   *
   * @example
   * ```typescript
   * for await (const event of client.stream({ eventType: "action_dispatched" })) {
   *   console.log(`${event.event}: ${JSON.stringify(event.data)}`);
   * }
   * ```
   */
  async *stream(options?: StreamOptions): AsyncGenerator<SseEvent, void, undefined> {
    const params = new URLSearchParams();
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

        for (const raw of lines) {
          // Strip a trailing CR — splitting on `\n` alone leaves bare
          // `\r` on every line when an intermediary normalises to CRLF.
          const line = raw.endsWith("\r") ? raw.slice(0, -1) : raw;
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

  /** POST one official JSON-RPC request and validate every SSE response frame. */
  private async *connectAgentA2aSse(
    path: string,
    envelope: Record<string, unknown>,
    requestId: string,
    extraHeaders: Record<string, string>,
    requireSource: boolean,
  ): AsyncGenerator<AgentServiceA2aFrame, void, undefined> {
    const headers = { ...this.headers(), ...A2A_HEADERS, ...extraHeaders, Accept: "text/event-stream" };
    const fetchOptions: RequestInit & { dispatcher?: unknown } = {
      method: "POST",
      headers,
      redirect: "error",
      body: JSON.stringify(envelope),
    };
    if (this.dispatcher) fetchOptions.dispatcher = this.dispatcher;
    let response: Response;
    try {
      response = await fetch(`${this.baseUrl}${path}`, fetchOptions);
    } catch (error) {
      throw new ConnectionError(error instanceof Error ? error.message : "A2A stream connection failed");
    }
    if (!response.ok) throw new HttpError(response.status, await response.text());
    if (response.headers.get("a2a-version") !== A2A_PROTOCOL_VERSION) throw new Error("agent service A2A stream version missing or unsupported");
    if (!response.headers.get("content-type")?.toLowerCase().startsWith("text/event-stream")) throw new Error("agent service did not return an A2A SSE stream");
    if (!response.body) throw new ConnectionError("A2A SSE response body is null");
    const sourceContext = requireSource ? agentSource(response.headers.get(AGENT_SOURCE_CONTEXT_HEADER)) : undefined;
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    let data: string[] = [];
    let dataBytes = 0;
    const parse = (): AgentServiceA2aFrame => {
      let value: unknown;
      try { value = JSON.parse(data.join("\n")); } catch { throw new Error("invalid A2A SSE frame"); }
      if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("invalid A2A SSE frame");
      const rpc = value as Record<string, unknown>;
      if (rpc.jsonrpc !== "2.0" || rpc.id !== requestId) throw new Error("A2A SSE frame request identity mismatch");
      if (rpc.error && typeof rpc.error === "object") {
        const error = rpc.error as Record<string, unknown>;
        throw new Error(`A2A error ${String(error.code ?? "A2A")}: ${String(error.message ?? "stream failed")}`);
      }
      if (!rpc.result || typeof rpc.result !== "object" || Array.isArray(rpc.result)) throw new Error("A2A SSE frame has no object result");
      return Object.freeze({ result: rpc.result as Record<string, unknown>, ...(sourceContext ? { sourceContext } : {}) });
    };
    const consume = (raw: string): AgentServiceA2aFrame | undefined => {
      const line = raw.endsWith("\r") ? raw.slice(0, -1) : raw;
      if (line === "") {
        if (data.length === 0) return undefined;
        const frame = parse();
        data = [];
        dataBytes = 0;
        return frame;
      }
      if (line.startsWith("data:")) {
        const value = line.slice(5).replace(/^ /, "");
        dataBytes += new TextEncoder().encode(value).byteLength;
        if (dataBytes > 2 * 1024 * 1024) throw new Error("A2A SSE frame exceeds 2 MiB");
        data.push(value);
      }
      return undefined;
    };
    try {
      while (true) {
        const { done, value } = await reader.read();
        buffer += decoder.decode(value, { stream: !done });
        const lines = buffer.split("\n");
        buffer = lines.pop() ?? "";
        for (const line of lines) {
          const frame = consume(line);
          if (frame) yield frame;
        }
        if (buffer.length > 2 * 1024 * 1024) throw new Error("A2A SSE line exceeds 2 MiB");
        if (done) break;
      }
      if (buffer) consume(buffer);
      if (data.length > 0) yield parse();
    } finally {
      await reader.cancel().catch(() => undefined);
      reader.releaseLock();
    }
  }

  /**
   * Connect to a bus SSE endpoint and yield raw envelopes (frame or
   * keep-alive). The dispatch `connectSse` silently swallows comments;
   * bus consumers want them as liveness signals, so this private helper
   * reimplements the line protocol with a slightly wider yield type.
   */
  private async *connectBusSse(
    path: string,
    params?: URLSearchParams,
  ): AsyncGenerator<BusSseEnvelope, void, undefined> {
    let url = `${this.baseUrl}${path}`;
    if (params && params.toString()) url += `?${params.toString()}`;
    const headers: Record<string, string> = { Accept: "text/event-stream" };
    if (this.apiKey) headers["Authorization"] = `Bearer ${this.apiKey}`;

    const response = await fetch(url, { method: "GET", headers });
    if (!response.ok) {
      throw new HttpError(response.status, `bus SSE connection failed: ${path}`);
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
        buffer = lines.pop() ?? "";
        for (const raw of lines) {
          // Strip a trailing CR. The SSE spec accepts `\r\n`, `\r`, or
          // `\n` as line terminators; splitting on `\n` alone leaves a
          // bare `\r` on every line when an intermediary normalises to
          // CRLF — and that breaks the empty-line frame trigger below.
          const line = raw.endsWith("\r") ? raw.slice(0, -1) : raw;
          if (line.startsWith(":")) {
            yield { kind: "keepAlive" };
            continue;
          }
          if (line === "") {
            if (currentData.length > 0 || currentEvent !== "" || currentId !== "") {
              yield {
                kind: "frame",
                event: currentEvent || undefined,
                id: currentId || undefined,
                data: currentData.join("\n"),
              };
            }
            currentEvent = "";
            currentId = "";
            currentData = [];
            continue;
          }
          if (line.startsWith("event:")) currentEvent = line.slice(6).trim();
          else if (line.startsWith("id:")) currentId = line.slice(3).trim();
          else if (line.startsWith("data:")) currentData.push(line.slice(5).trim());
        }
      }
    } finally {
      reader.releaseLock();
    }
  }

  private envelopeToConsumeItem(env: BusSseEnvelope): BusConsumeItem {
    if (env.kind === "keepAlive") return { kind: "keepAlive" };
    const name = env.event ?? "message";
    if (name === "bus.message" || name === "message") {
      const decoded = decodeJsonOrPassthrough(env.data);
      if (typeof decoded !== "object" || decoded === null) {
        throw new ApiError("BUS", `invalid bus.message payload: ${env.data}`, false);
      }
      return {
        kind: "message",
        message: parseBusConsumedMessage(decoded as Record<string, unknown>),
      };
    }
    if (name === "bus.error") {
      return { kind: "error", error: extractErrorMessage(env.data) };
    }
    throw new ApiError(
      "BUS",
      `unexpected SSE event '${name}' on bus subscribe stream`,
      false,
    );
  }

  private envelopeToStreamItem(env: BusSseEnvelope): BusStreamItem {
    if (env.kind === "keepAlive") return { kind: "keepAlive" };
    const name = env.event ?? "message";
    if (name === "bus.stream.chunk") {
      const decoded = decodeJsonOrPassthrough(env.data);
      if (typeof decoded !== "object" || decoded === null) {
        throw new ApiError("BUS", `invalid stream chunk payload: ${env.data}`, false);
      }
      return {
        kind: "chunk",
        chunk: parseStreamChunkEnvelope(decoded as Record<string, unknown>),
      };
    }
    if (name === "bus.stream.end") {
      const decoded = decodeJsonOrPassthrough(env.data);
      if (typeof decoded !== "object" || decoded === null) {
        throw new ApiError("BUS", `invalid stream end payload: ${env.data}`, false);
      }
      return {
        kind: "end",
        end: parseStreamEndEnvelope(decoded as Record<string, unknown>),
      };
    }
    if (name === "bus.stream.error") {
      return { kind: "error", error: extractErrorMessage(env.data) };
    }
    throw new ApiError(
      "BUS",
      `unexpected SSE event '${name}' on bus stream consumer`,
      false,
    );
  }

  // ===========================================================================
  // Phase 8b: Agentic bus surface (Phases 1-6c)
  // ===========================================================================

  /** Helper: percent-encode a single path segment. The bus REST
   *  surface treats namespace / tenant / name slots as opaque
   *  strings, so reserved characters like `/` need encoding rather
   *  than slipping through into the URL grammar.
   */
  private busSeg(s: string): string {
    return encodeURIComponent(s);
  }

  /** Helper: read an Acteon-shaped error body, fall back to a plain
   *  HttpError if the body isn't structured.
   */
  private async busThrowFromResponse(response: Response): Promise<never> {
    try {
      const data = (await response.json()) as Record<string, unknown>;
      throw new ApiError(
        (data.code as string) ?? "BUS",
        (data.error as string) ?? (data.message as string) ?? "bus error",
        false,
      );
    } catch (err) {
      if (err instanceof ApiError) throw err;
      throw new HttpError(response.status, response.statusText || "bus error");
    }
  }

  // --------------- Phase 1: Topics + publish ---------------

  async createBusTopic(req: CreateBusTopic): Promise<BusTopic> {
    const response = await this.request("POST", "/v1/bus/topics", {
      body: createBusTopicBody(req),
    });
    if (!response.ok) await this.busThrowFromResponse(response);
    return parseBusTopic((await response.json()) as Record<string, unknown>);
  }

  async listBusTopics(): Promise<BusTopic[]> {
    const params = new URLSearchParams();

    const response = await this.request("GET", "/v1/bus/topics", {
      params: params.toString() ? params : undefined,
    });
    if (!response.ok) await this.busThrowFromResponse(response);
    const body = (await response.json()) as { topics?: Record<string, unknown>[] };
    return (body.topics ?? []).map(parseBusTopic);
  }

  async getBusTopic(name: string): Promise<BusTopic> {
    const { namespace, tenant } = (await this.identity()).scope;
    const topics = await this.listBusTopics();
    const topic = topics.find(t => t.namespace === namespace && t.tenant === tenant && t.name === name);
    if (!topic) throw new HttpError(404, "Bus topic not found");
    return topic;
  }

  async deleteBusTopic(name: string): Promise<void> {
    const topic = await this.getBusTopic(name);
    const response = await this.request("DELETE", `/v1/bus/topics/${this.busSeg(topic.kafkaName)}`);
    if (!response.ok) await this.busThrowFromResponse(response);
  }

  async publishBusMessage(req: PublishBusMessage): Promise<PublishReceipt> {
    const response = await this.request("POST", "/v1/bus/publish", {
      body: publishBusMessageBody(req),
    });
    if (!response.ok) await this.busThrowFromResponse(response);
    return parsePublishReceipt((await response.json()) as Record<string, unknown>);
  }

  // --------------- Phase 2: Subscriptions + lag ---------------

  async createBusSubscription(req: CreateBusSubscription): Promise<BusSubscription> {
    const response = await this.request("POST", "/v1/bus/subscriptions", {
      body: createBusSubscriptionBody(req),
    });
    if (!response.ok) await this.busThrowFromResponse(response);
    return parseBusSubscription((await response.json()) as Record<string, unknown>);
  }

  async listBusSubscriptions(
    filter: { topic?: string } = {},
  ): Promise<BusSubscription[]> {
    const params = new URLSearchParams();

    if (filter.topic) params.set("topic", filter.topic);
    const response = await this.request("GET", "/v1/bus/subscriptions", {
      params: params.toString() ? params : undefined,
    });
    if (!response.ok) await this.busThrowFromResponse(response);
    const body = (await response.json()) as { subscriptions?: Record<string, unknown>[] };
    return (body.subscriptions ?? []).map(parseBusSubscription);
  }

  async getBusSubscription(subId: string): Promise<BusSubscription> {
    const { namespace, tenant } = (await this.identity()).scope;
    const subscriptions = await this.listBusSubscriptions({ });
    const subscription = subscriptions.find(s => s.namespace === namespace && s.tenant === tenant && s.id === subId);
    if (!subscription) throw new HttpError(404, "Bus subscription not found");
    return subscription;
  }

  async deleteBusSubscription(subId: string
  ): Promise<void> {
    const response = await this.request(
      "DELETE",
      `/v1/bus/subscriptions/${this.busSeg(subId)}`,
    );
    if (!response.ok) await this.busThrowFromResponse(response);
  }

  async getBusSubscriptionLag(subId: string
  ): Promise<BusLag> {
    const response = await this.request(
      "GET",
      `/v1/bus/subscriptions/${this.busSeg(subId)}/lag`,
    );
    if (!response.ok) await this.busThrowFromResponse(response);
    return parseBusLag((await response.json()) as Record<string, unknown>);
  }

  // --------------- Phase 3: Schemas ---------------

  async registerBusSchema(req: RegisterBusSchema): Promise<BusSchema> {
    const response = await this.request("POST", "/v1/bus/schemas", {
      body: registerBusSchemaBody(req),
    });
    if (!response.ok) await this.busThrowFromResponse(response);
    return parseBusSchema((await response.json()) as Record<string, unknown>);
  }

  async listBusSchemas(
    filter: {
      subject?: string;
      latestOnly?: boolean;
    } = {},
  ): Promise<BusSchema[]> {
    const params = new URLSearchParams();

    if (filter.subject) params.set("subject", filter.subject);
    if (filter.latestOnly) params.set("latest_only", "true");
    const response = await this.request("GET", "/v1/bus/schemas", {
      params: params.toString() ? params : undefined,
    });
    if (!response.ok) await this.busThrowFromResponse(response);
    const body = (await response.json()) as { schemas?: Record<string, unknown>[] };
    return (body.schemas ?? []).map(parseBusSchema);
  }

  async getBusSchema(subject: string, version: number
  ): Promise<BusSchema> {
    const response = await this.request(
      "GET",
      `/v1/bus/schemas/${this.busSeg(subject)}/${version}`,
    );
    if (!response.ok) await this.busThrowFromResponse(response);
    return parseBusSchema((await response.json()) as Record<string, unknown>);
  }

  async deleteBusSchema(subject: string, version: number
  ): Promise<void> {
    const response = await this.request(
      "DELETE",
      `/v1/bus/schemas/${this.busSeg(subject)}/${version}`,
    );
    if (!response.ok) await this.busThrowFromResponse(response);
  }

  // --------------- Phase 4: Agents + heartbeat ---------------

  async registerBusAgent(req: RegisterBusAgent): Promise<BusAgent> {
    const response = await this.request("POST", "/v1/bus/agents", {
      body: registerBusAgentBody(req),
    });
    if (!response.ok) await this.busThrowFromResponse(response);
    return parseBusAgent((await response.json()) as Record<string, unknown>);
  }

  async listBusAgents(): Promise<BusAgent[]> {
    const params = new URLSearchParams();

    const response = await this.request("GET", "/v1/bus/agents", {
      params: params.toString() ? params : undefined,
    });
    if (!response.ok) await this.busThrowFromResponse(response);
    const body = (await response.json()) as { agents?: Record<string, unknown>[] };
    return (body.agents ?? []).map(parseBusAgent);
  }

  async getBusAgent(agentId: string): Promise<BusAgent> {
    const response = await this.request(
      "GET",
      `/v1/bus/agents/${this.busSeg(agentId)}`,
    );
    if (!response.ok) await this.busThrowFromResponse(response);
    return parseBusAgent((await response.json()) as Record<string, unknown>);
  }

  async deleteBusAgent(agentId: string): Promise<void> {
    const response = await this.request(
      "DELETE",
      `/v1/bus/agents/${this.busSeg(agentId)}`,
    );
    if (!response.ok) await this.busThrowFromResponse(response);
  }

  async heartbeatBusAgent(agentId: string
  ): Promise<BusAgent> {
    const response = await this.request(
      "PATCH",
      `/v1/bus/agents/${this.busSeg(agentId)}/heartbeat`,
    );
    if (!response.ok) await this.busThrowFromResponse(response);
    return parseBusAgent((await response.json()) as Record<string, unknown>);
  }

  /**
   * Set the operator admin state on an agent (active / suspended /
   * banned). Requires the standard `ManageAgent` permission. The
   * server returns 400 if `req.expiresAt` is set on anything other
   * than `"suspended"`.
   */
  async setBusAgentAdminState(agentId: string, req: SetBusAgentAdminState
  ): Promise<BusAgent> {
    const response = await this.request(
      "PUT",
      `/v1/bus/agents/${this.busSeg(agentId)}/admin-state`,
      { body: setBusAgentAdminStateBody(req) },
    );
    if (!response.ok) await this.busThrowFromResponse(response);
    return parseBusAgent((await response.json()) as Record<string, unknown>);
  }

  // --------------- Phase 5: Conversations ---------------

  async createBusConversation(req: CreateBusConversation): Promise<BusConversation> {
    const response = await this.request("POST", "/v1/bus/conversations", {
      body: createBusConversationBody(req),
    });
    if (!response.ok) await this.busThrowFromResponse(response);
    return parseBusConversation((await response.json()) as Record<string, unknown>);
  }

  async listBusConversations(
    filter: { state?: string; participant?: string } = {},
  ): Promise<BusConversation[]> {
    const params = new URLSearchParams();

    if (filter.state) params.set("state", filter.state);
    if (filter.participant) params.set("participant", filter.participant);
    const response = await this.request("GET", "/v1/bus/conversations", {
      params: params.toString() ? params : undefined,
    });
    if (!response.ok) await this.busThrowFromResponse(response);
    const body = (await response.json()) as { conversations?: Record<string, unknown>[] };
    return (body.conversations ?? []).map(parseBusConversation);
  }

  async getBusConversation(conversationId: string
  ): Promise<BusConversation> {
    const response = await this.request(
      "GET",
      `/v1/bus/conversations/${this.busSeg(conversationId)}`,
    );
    if (!response.ok) await this.busThrowFromResponse(response);
    return parseBusConversation((await response.json()) as Record<string, unknown>);
  }

  async deleteBusConversation(conversationId: string
  ): Promise<void> {
    const response = await this.request(
      "DELETE",
      `/v1/bus/conversations/${this.busSeg(conversationId)}`,
    );
    if (!response.ok) await this.busThrowFromResponse(response);
  }

  async transitionBusConversation(conversationId: string, targetState: string
  ): Promise<BusConversation> {
    const response = await this.request(
      "POST",
      `/v1/bus/conversations/${this.busSeg(conversationId)}/transition`,
      { body: { target_state: targetState } },
    );
    if (!response.ok) await this.busThrowFromResponse(response);
    return parseBusConversation((await response.json()) as Record<string, unknown>);
  }

  async appendBusConversationMessage(conversationId: string, req: AppendBusConversationMessage
  ): Promise<Record<string, unknown>> {
    const response = await this.request(
      "POST",
      `/v1/bus/conversations/${this.busSeg(conversationId)}/messages`,
      { body: appendBusConversationMessageBody(req) },
    );
    if (!response.ok) await this.busThrowFromResponse(response);
    return (await response.json()) as Record<string, unknown>;
  }

  async replayBusConversationMessages(conversationId: string, options: { limit?: number; cursor?: string } = {}
  ): Promise<BusReplayResponse> {
    const params = new URLSearchParams();
    if (options.limit !== undefined) params.set("limit", String(options.limit));
    if (options.cursor) params.set("cursor", options.cursor);
    const response = await this.request(
      "GET",
      `/v1/bus/conversations/${this.busSeg(conversationId)}/messages`,
      { params: params.toString() ? params : undefined },
    );
    if (!response.ok) await this.busThrowFromResponse(response);
    return parseBusReplayResponse((await response.json()) as Record<string, unknown>);
  }

  // --------------- Phase 6a: Tool envelopes ---------------

  /**
   * Append a tool-call envelope.
   *
   * Returns a discriminated union: `kind === "produced"` when the
   * call landed on Kafka, `kind === "parked"` when the server
   * parked it under a Phase 6c HITL approval (driven by
   * `req.requireApproval`).
   */
  async postBusToolCall(conversationId: string, req: PostBusToolCall
  ): Promise<PostBusToolCallOutcome> {
    const response = await this.request(
      "POST",
      `/v1/bus/conversations/${this.busSeg(conversationId)}/tool-calls`,
      { body: postBusToolCallBody(req) },
    );
    if (!response.ok) await this.busThrowFromResponse(response);
    const body = (await response.json()) as Record<string, unknown>;
    if (response.status === 202) {
      return { kind: "parked", receipt: parseBusApprovalParkedReceipt(body) };
    }
    return { kind: "produced", receipt: parseBusToolEnvelopeReceipt(body) };
  }

  async postBusToolResult(conversationId: string, req: PostBusToolResult
  ): Promise<BusToolEnvelopeReceipt> {
    const response = await this.request(
      "POST",
      `/v1/bus/conversations/${this.busSeg(conversationId)}/tool-results`,
      { body: postBusToolResultBody(req) },
    );
    if (!response.ok) await this.busThrowFromResponse(response);
    return parseBusToolEnvelopeReceipt((await response.json()) as Record<string, unknown>);
  }

  async lookupBusToolResult(callId: string, params: BusToolResultLookupParams
  ): Promise<BusToolResultLookup> {
    const response = await this.request(
      "GET",
      `/v1/bus/tool-calls/${this.busSeg(callId)}/result`,
      { params: busToolResultLookupParams(params) },
    );
    if (!response.ok) await this.busThrowFromResponse(response);
    return parseBusToolResultLookup((await response.json()) as Record<string, unknown>);
  }

  // --------------- Phase 6b: Stream envelopes ---------------

  async postBusStreamChunk(conversationId: string, req: PostBusStreamChunk
  ): Promise<BusStreamEnvelopeReceipt> {
    const response = await this.request(
      "POST",
      `/v1/bus/conversations/${this.busSeg(conversationId)}/stream-chunks`,
      { body: postBusStreamChunkBody(req) },
    );
    if (!response.ok) await this.busThrowFromResponse(response);
    return parseBusStreamEnvelopeReceipt((await response.json()) as Record<string, unknown>);
  }

  async postBusStreamEnd(conversationId: string, req: PostBusStreamEnd
  ): Promise<BusStreamEnvelopeReceipt> {
    const response = await this.request(
      "POST",
      `/v1/bus/conversations/${this.busSeg(conversationId)}/stream-end`,
      { body: postBusStreamEndBody(req) },
    );
    if (!response.ok) await this.busThrowFromResponse(response);
    return parseBusStreamEnvelopeReceipt((await response.json()) as Record<string, unknown>);
  }

  /**
   * Build the SSE consume URL for a stream. Plug it into your
   * preferred SSE client (`EventSource` in the browser, `eventsource`
   * package on the server, etc.). Path segments are encoded the
   * same way the Rust + Python SDKs encode them.
   */
  busStreamConsumeUrl(conversationId: string, streamId: string
  ): string {
    return (
      `${this.baseUrl}/v1/bus/streams/` +
      `` +
      `${this.busSeg(conversationId)}/${this.busSeg(streamId)}`
    );
  }

  /**
   * Consume a bus subscription via SSE
   * (`GET /v1/bus/subscribe/{subscriptionId}`). Yields one item per
   * Kafka record on the underlying topic. Server-side errors surface
   * as `kind: "error"`; SSE keep-alive comments surface as
   * `kind: "keepAlive"` so callers can use them as a liveness signal.
   *
   * When `reconnect` is set, a clean disconnect triggers exponential
   * backoff and a fresh subscribe call from `latest` — yielding a
   * `kind: "reconnected"` item so callers can resync state. Note that
   * resume from `latest` means messages produced during the disconnect
   * window are dropped; use Phase 2 durable subscriptions with manual
   * ack for lossless delivery.
   *
   * @example
   * ```typescript
   * for await (const item of client.consumeBusSubscription("agent-A", {
   *   topic: "agents.demo.events",
   *   from: "earliest",
   *   reconnect: {},  // 500ms initial, 30s cap, infinite retries
   * })) {
   *   if (item.kind === "message") console.log(item.message.offset);
   *   else if (item.kind === "reconnected") {
   *     console.warn(`reconnected after ${item.backoffMs}ms (attempt ${item.attempt})`);
   *   }
   * }
   * ```
   */
  async *consumeBusSubscription(
    subscriptionId: string,
    options: {
      topic: string;
      from?: "earliest" | "latest";
      reconnect?: ReconnectConfig;
    },
  ): AsyncGenerator<BusConsumeItem, void, undefined> {
    const path = `/v1/bus/subscribe/${this.busSeg(subscriptionId)}`;
    if (options.reconnect === undefined) {
      const params = new URLSearchParams();
      params.set("topic", options.topic);
      if (options.from !== undefined) params.set("from", options.from);
      for await (const env of this.connectBusSse(path, params)) {
        yield this.envelopeToConsumeItem(env);
      }
      return;
    }

    // Reconnect path: open the first stream with the caller's `from`,
    // resume from `latest` after every disconnect. Counter resets on
    // successful read so a long-stable connection isn't penalised
    // for a single later blip.
    const cfg = options.reconnect;
    const initialBackoffMs = cfg.initialBackoffMs ?? 500;
    const maxBackoffMs = cfg.maxBackoffMs ?? 30_000;
    const maxAttempts = cfg.maxAttempts;
    let attempt = 0;
    let firstOpen = true;
    while (true) {
      const params = new URLSearchParams();
      params.set("topic", options.topic);
      params.set("from", firstOpen ? options.from ?? "latest" : "latest");
      try {
        for await (const env of this.connectBusSse(path, params)) {
          attempt = 0;
          yield this.envelopeToConsumeItem(env);
        }
      } catch (err) {
        // Connection-level errors are the disconnect signal we're
        // meant to recover from; let them flow through to the sleep+
        // retry path. Anything else (parse error etc.) re-throws.
        if (err instanceof HttpError || err instanceof ConnectionError) {
          // expected — fall through to backoff
        } else {
          throw err;
        }
      }
      firstOpen = false;
      if (maxAttempts !== undefined && attempt >= maxAttempts) {
        return;
      }
      const backoffMs = reconnectBackoffMs(attempt, initialBackoffMs, maxBackoffMs);
      await new Promise((resolve) => setTimeout(resolve, backoffMs));
      attempt += 1;
      yield { kind: "reconnected", backoffMs, attempt };
    }
  }

  /**
   * Consume a typed stream via SSE
   * (`GET /v1/bus/streams/{conversationId}/{streamId}`).
   * The server filters records by `(envelope_kind, conversation_id,
   * stream_id)`, so this generator only emits chunks for the requested
   * stream id and closes after the terminal `kind: "end"`.
   *
   * @example
   * ```typescript
   * for await (const item of client.consumeBusStream("agents", "demo", "thread-1", "stream-42")) {
   *   if (item.kind === "chunk") process.stdout.write(JSON.stringify(item.chunk.body));
   *   else if (item.kind === "end") break;
   * }
   * ```
   */
  async *consumeBusStream(conversationId: string, streamId: string
  ): AsyncGenerator<BusStreamItem, void, undefined> {
    const path =
      `/v1/bus/streams/` +
      `${this.busSeg(conversationId)}/${this.busSeg(streamId)}`;
    for await (const env of this.connectBusSse(path)) {
      const item = this.envelopeToStreamItem(env);
      yield item;
      if (item.kind === "end") return;
    }
  }

  // --------------- Phase 6c: HITL approvals ---------------

  async listBusApprovals(filter: { status?: BusApprovalStatus; conversationId?: string } = {}
  ): Promise<BusApprovalView[]> {
    const params = new URLSearchParams();
    if (filter.status) params.set("status", filter.status);
    if (filter.conversationId) params.set("conversation_id", filter.conversationId);
    const response = await this.request(
      "GET",
      "/v1/bus/approvals",
      { params: params.toString() ? params : undefined },
    );
    if (!response.ok) await this.busThrowFromResponse(response);
    const body = (await response.json()) as { approvals?: Record<string, unknown>[] };
    return (body.approvals ?? []).map(parseBusApprovalView);
  }

  async getBusApproval(approvalId: string
  ): Promise<BusApprovalView> {
    const response = await this.request(
      "GET",
      `/v1/bus/approvals/${this.busSeg(approvalId)}`,
    );
    if (!response.ok) await this.busThrowFromResponse(response);
    return parseBusApprovalView((await response.json()) as Record<string, unknown>);
  }

  async approveBusApproval(approvalId: string, decision: BusApprovalDecision
  ): Promise<BusApprovalDecisionResponse> {
    const response = await this.request(
      "POST",
      `/v1/bus/approvals/${this.busSeg(approvalId)}/approve`,
      { body: busApprovalDecisionBody(decision) },
    );
    if (!response.ok) await this.busThrowFromResponse(response);
    return parseBusApprovalDecisionResponse((await response.json()) as Record<string, unknown>);
  }

  async rejectBusApproval(approvalId: string, decision: BusApprovalDecision
  ): Promise<BusApprovalDecisionResponse> {
    const response = await this.request(
      "POST",
      `/v1/bus/approvals/${this.busSeg(approvalId)}/reject`,
      { body: busApprovalDecisionBody(decision) },
    );
    if (!response.ok) await this.busThrowFromResponse(response);
    return parseBusApprovalDecisionResponse((await response.json()) as Record<string, unknown>);
  }

  // ===========================================================================
  // Worker Task Queues
  //
  // Lease protocol driven by external workers: poll a named queue for
  // tasks, execute them, and settle each one via complete/fail using the
  // lease token from the poll response. The `Worker` class in `worker.ts`
  // wraps these methods in a poll loop.
  // ===========================================================================

  /** Translate a queue/workflow error response into an `HttpError`.
   *  Both API surfaces use the `{ "error": "..." }` envelope. */
  private async taskThrowFromResponse(
    response: Response,
    context: string,
  ): Promise<never> {
    let payload: Record<string, unknown> = {};
    try {
      payload = (await response.json()) as Record<string, unknown>;
    } catch {
      /* fall through with empty payload */
    }
    const message =
      (payload.error as string | undefined) ??
      (payload.message as string | undefined) ??
      context;
    throw new HttpError(response.status, message);
  }

  /**
   * Poll a queue for tasks, leasing up to `maxTasks` of them.
   * Returned tasks carry the `leaseToken` required by heartbeat /
   * complete / fail.
   */
  async pollTasks(queue: string, options?: PollTasksOptions
  ): Promise<WorkerTask[]> {
    const response = await this.request(
      "POST",
      `/v1/queues/${encodeURIComponent(queue)}/poll`,
      { body: pollTasksBody(options) },
    );

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseTaskListResponse(data);
    }
    return this.taskThrowFromResponse(response, "Failed to poll tasks");
  }

  /**
   * Enqueue a task on a queue.
   */
  async enqueueTask(queue: string, actionType: string, payload: unknown, options?: EnqueueTaskOptions
  ): Promise<WorkerTask> {
    const response = await this.request(
      "POST",
      `/v1/queues/${encodeURIComponent(queue)}/tasks`,
      { body: enqueueTaskBody(actionType, payload, options) },
    );

    if (response.status === 201) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseWorkerTask(data);
    }
    return this.taskThrowFromResponse(response, "Failed to enqueue task");
  }

  /**
   * Extend the lease on a task while a handler is still working on it.
   */
  async heartbeatTask(taskId: string, leaseToken: string, extendSeconds?: number
  ): Promise<WorkerTask> {
    const response = await this.request(
      "POST",
      `/v1/queues/tasks/${encodeURIComponent(taskId)}/heartbeat`,
      { body: heartbeatTaskBody(leaseToken, extendSeconds) },
    );

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseWorkerTask(data);
    }
    return this.taskThrowFromResponse(response, "Failed to heartbeat task");
  }

  /**
   * Settle a leased task as completed, recording `result`.
   */
  async completeTask(taskId: string, leaseToken: string, result: unknown
  ): Promise<WorkerTask> {
    const response = await this.request(
      "POST",
      `/v1/queues/tasks/${encodeURIComponent(taskId)}/complete`,
      { body: completeTaskBody(leaseToken, result) },
    );

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseWorkerTask(data);
    }
    return this.taskThrowFromResponse(response, "Failed to complete task");
  }

  /**
   * Settle a leased task as failed. With `retryable: true` (the
   * default worker behaviour) the server redelivers while attempts
   * remain; with `retryable: false` the task fails terminally.
   */
  async failTask(taskId: string, leaseToken: string, error: string, retryable: boolean = true
  ): Promise<WorkerTask> {
    const response = await this.request(
      "POST",
      `/v1/queues/tasks/${encodeURIComponent(taskId)}/fail`,
      { body: failTaskBody(leaseToken, error, retryable) },
    );

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseWorkerTask(data);
    }
    return this.taskThrowFromResponse(response, "Failed to fail task");
  }

  /**
   * Get a single task by ID. Returns `null` if not found.
   */
  async getTask(taskId: string
  ): Promise<WorkerTask | null> {
    const params = new URLSearchParams();

    const response = await this.request(
      "GET",
      `/v1/queues/tasks/${encodeURIComponent(taskId)}`,
      { params },
    );

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseWorkerTask(data);
    } else if (response.status === 404) {
      return null;
    }
    return this.taskThrowFromResponse(response, "Failed to get task");
  }

  /**
   * List tasks on a queue, optionally filtered by status.
   */
  async listTasks(queue: string, status?: WorkerTaskStatus
  ): Promise<WorkerTask[]> {
    const params = new URLSearchParams();

    if (status !== undefined) params.set("status", status);
    const response = await this.request(
      "GET",
      `/v1/queues/${encodeURIComponent(queue)}/tasks`,
      { params },
    );

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseTaskListResponse(data);
    }
    return this.taskThrowFromResponse(response, "Failed to list tasks");
  }

  // ===========================================================================
  // Workflow Executions
  //
  // Checkpoint-based durable workflows-as-code: the server persists
  // checkpoints and schedules continuation tasks on worker queues; workflow
  // logic runs on workers (see `WorkflowContext` in `workflows.ts`).
  // ===========================================================================

  /**
   * Start a workflow execution. Continuation tasks are routed through
   * `queue`; a `Worker` polling that queue with the workflow
   * registered drives the execution.
   */
  async startWorkflow(workflow: string, queue: string, input: unknown, options?: StartWorkflowOptions
  ): Promise<WorkflowExecution> {
    const response = await this.request("POST", "/v1/workflows/start", {
      body: startWorkflowBody(workflow, queue, input, options),
    });

    if (response.status === 201) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseWorkflowExecution(data);
    }
    return this.taskThrowFromResponse(response, "Failed to start workflow");
  }

  /**
   * List workflow executions, optionally filtered by workflow name
   * and status.
   */
  async listWorkflowExecutions(options?: { workflow?: string; status?: WorkflowStatus; limit?: number }
  ): Promise<WorkflowExecution[]> {
    const params = new URLSearchParams();

    if (options?.workflow !== undefined) params.set("workflow", options.workflow);
    if (options?.status !== undefined) params.set("status", options.status);
    if (options?.limit !== undefined) params.set("limit", options.limit.toString());
    const response = await this.request("GET", "/v1/workflows/executions", {
      params,
    });

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseWorkflowExecutionListResponse(data);
    }
    return this.taskThrowFromResponse(
      response,
      "Failed to list workflow executions",
    );
  }

  /**
   * Get a workflow execution by ID. Returns `null` if not found.
   */
  async getWorkflowExecution(executionId: string
  ): Promise<WorkflowExecution | null> {
    const params = new URLSearchParams();

    const response = await this.request(
      "GET",
      `/v1/workflows/executions/${encodeURIComponent(executionId)}`,
      { params },
    );

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseWorkflowExecution(data);
    } else if (response.status === 404) {
      return null;
    }
    return this.taskThrowFromResponse(
      response,
      "Failed to get workflow execution",
    );
  }

  /**
   * Deliver an external signal to a workflow execution. The payload
   * is returned by the awaiting `ctx.waitForSignal(name)` call (or
   * buffered until the workflow awaits it).
   */
  async signalWorkflow(executionId: string, signalName: string, payload?: unknown
  ): Promise<void> {
    const body: Record<string, unknown> = { };
    if (payload !== undefined) body.payload = payload;
    const response = await this.request(
      "POST",
      `/v1/workflows/executions/${encodeURIComponent(executionId)}/signal/${encodeURIComponent(signalName)}`,
      { body },
    );

    if (!response.ok) {
      await this.taskThrowFromResponse(response, "Failed to signal workflow");
    }
  }

  /**
   * Cancel a workflow execution.
   */
  async cancelWorkflow(executionId: string, reason?: string
  ): Promise<void> {
    const body: Record<string, unknown> = { };
    if (reason !== undefined) body.reason = reason;
    const response = await this.request(
      "POST",
      `/v1/workflows/executions/${encodeURIComponent(executionId)}/cancel`,
      { body },
    );

    if (!response.ok) {
      await this.taskThrowFromResponse(response, "Failed to cancel workflow");
    }
  }

  /**
   * Record a named checkpoint on a workflow execution. Idempotent by
   * name: replays return the originally-recorded data. Normally
   * called through `ctx.step(...)` rather than directly.
   */
  async recordWorkflowCheckpoint(executionId: string, name: string, data: unknown
  ): Promise<RecordCheckpointResponse> {
    const response = await this.request(
      "POST",
      `/v1/workflows/executions/${encodeURIComponent(executionId)}/checkpoints`,
      { body: {name, data } },
    );

    if (response.ok) {
      const body = (await response.json()) as Record<string, unknown>;
      return parseRecordCheckpointResponse(body);
    }
    return this.taskThrowFromResponse(response, "Failed to record checkpoint");
  }

  /**
   * Start a child workflow from a parent execution, idempotently
   * keyed by `checkpoint`. Returns the child execution ID (the
   * recorded one on replay). Normally called through
   * `ctx.startChild(...)` rather than directly.
   */
  async startChildWorkflow(executionId: string, checkpoint: string, workflow: string, input: unknown, options?: StartChildOptions
  ): Promise<string> {
    const body: Record<string, unknown> = {checkpoint, workflow, input
    };
    if (options?.queue !== undefined) body.queue = options.queue;
    if (options?.parentClosePolicy !== undefined) {
      body.parent_close_policy = options.parentClosePolicy;
    }
    const response = await this.request(
      "POST",
      `/v1/workflows/executions/${encodeURIComponent(executionId)}/children`,
      { body },
    );

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return data.child_execution_id as string;
    }
    return this.taskThrowFromResponse(response, "Failed to start child workflow");
  }

  /**
   * Get the ordered event history of an execution.
   */
  async getExecutionHistory(executionId: string
  ): Promise<ExecutionHistory> {
    const params = new URLSearchParams();

    const response = await this.request(
      "GET",
      `/v1/executions/${encodeURIComponent(executionId)}/history`,
      { params },
    );

    if (response.ok) {
      const data = (await response.json()) as Record<string, unknown>;
      return parseExecutionHistory(data);
    }
    return this.taskThrowFromResponse(
      response,
      "Failed to get execution history",
    );
  }

  // ===========================================================================
  // A2A protocol surface
  //
  // Mirrors the Rust + Python SDKs method-for-method. Every authenticated
  // call sends `A2A-Version: 1.0`; the discovery call is intentionally
  // unauthenticated per spec.
  // ===========================================================================

  /** Translate an A2A error response into an `ApiError`. The A2A
   *  surface uses the same `{ "error": "..." }` envelope as the rest
   *  of the Acteon REST API. */
  private async a2aThrowFromResponse(response: Response): Promise<never> {
    let payload: Record<string, unknown> = {};
    try {
      payload = (await response.json()) as Record<string, unknown>;
    } catch {
      /* fall through with empty payload */
    }
    const message =
      (payload.error as string | undefined) ??
      (payload.message as string | undefined) ??
      `a2a error (status ${response.status})`;
    const retryable =
      response.status === 408 ||
      response.status === 425 ||
      response.status === 429 ||
      response.status >= 500;
    throw new ApiError(
      (payload.code as string | undefined) ?? "A2A",
      message,
      retryable,
    );
  }

  /** `POST /a2a/v1/message:send`. */
  async a2aSendMessage(message: Record<string, unknown>
  ): Promise<Record<string, unknown>> {
    const response = await this.request(
      "POST",
      `/a2a/v1/message:send`,
      { body: { message }, extraHeaders: { ...A2A_HEADERS } },
    );
    if (!response.ok) await this.a2aThrowFromResponse(response);
    return (await response.json()) as Record<string, unknown>;
  }

  /** `GET /a2a/v1/tasks/{id}`. */
  async a2aGetTask(taskId: string
  ): Promise<Record<string, unknown>> {
    const response = await this.request(
      "GET",
      `/a2a/v1/tasks/${a2aSegment(taskId)}`,
      { extraHeaders: { ...A2A_HEADERS } },
    );
    if (!response.ok) await this.a2aThrowFromResponse(response);
    return (await response.json()) as Record<string, unknown>;
  }

  /** `POST /a2a/v1/tasks/{id}:cancel`. The
   *  `:cancel` verb is part of the URL (spec §11) — the server
   *  splits it off in-handler. */
  async a2aCancelTask(taskId: string
  ): Promise<Record<string, unknown>> {
    const response = await this.request(
      "POST",
      `/a2a/v1/tasks/${a2aSegment(taskId)}:cancel`,
      { extraHeaders: { ...A2A_HEADERS } },
    );
    if (!response.ok) await this.a2aThrowFromResponse(response);
    return (await response.json()) as Record<string, unknown>;
  }

  /** `POST .../v1/tasks/{id}/pushNotificationConfigs` — register or
   *  upsert a push-notification webhook for a Task. Use
   *  {@link makePushConfig} to build `config`. */
  async a2aSetPushConfig(taskId: string, config: Record<string, unknown>
  ): Promise<Record<string, unknown>> {
    const response = await this.request(
      "POST",
      `/a2a/v1/tasks/${a2aSegment(taskId)}/pushNotificationConfigs`,
      { body: config, extraHeaders: { ...A2A_HEADERS } },
    );
    if (!response.ok) await this.a2aThrowFromResponse(response);
    return (await response.json()) as Record<string, unknown>;
  }

  /** `GET .../v1/tasks/{id}/pushNotificationConfigs` — list every
   *  config registered for the task. */
  async a2aListPushConfigs(taskId: string
  ): Promise<Record<string, unknown>[]> {
    const response = await this.request(
      "GET",
      `/a2a/v1/tasks/${a2aSegment(taskId)}/pushNotificationConfigs`,
      { extraHeaders: { ...A2A_HEADERS } },
    );
    if (!response.ok) await this.a2aThrowFromResponse(response);
    return (await response.json()) as Record<string, unknown>[];
  }

  /** `GET …/pushNotificationConfigs/{cfgId}` — read one config. */
  async a2aGetPushConfig(taskId: string, configId: string
  ): Promise<Record<string, unknown>> {
    const response = await this.request(
      "GET",
      `/a2a/v1/tasks/${a2aSegment(taskId)}/pushNotificationConfigs/${a2aSegment(configId)}`,
      { extraHeaders: { ...A2A_HEADERS } },
    );
    if (!response.ok) await this.a2aThrowFromResponse(response);
    return (await response.json()) as Record<string, unknown>;
  }

  /** `DELETE …/pushNotificationConfigs/{cfgId}`. Throws an
   *  `ApiError` with HTTP 404 when the config doesn't exist — the
   *  server never silently no-ops. */
  async a2aDeletePushConfig(taskId: string, configId: string
  ): Promise<void> {
    const response = await this.request(
      "DELETE",
      `/a2a/v1/tasks/${a2aSegment(taskId)}/pushNotificationConfigs/${a2aSegment(configId)}`,
      { extraHeaders: { ...A2A_HEADERS } },
    );
    if (!response.ok) await this.a2aThrowFromResponse(response);
  }

  /** `GET /a2a/.well-known/agent.json` — the
   *  unauthenticated discovery endpoint. Issued **without** the
   *  Authorization header per A2A spec. */
  async a2aDiscoverAgent(
  ): Promise<Record<string, unknown>> {
    const response = await this.request(
      "GET",
      `/a2a/.well-known/agent.json`,
      { skipAuth: false },
    );
    if (!response.ok) await this.a2aThrowFromResponse(response);
    return (await response.json()) as Record<string, unknown>;
  }

  /** JSON-RPC `agent/getAuthenticatedExtendedCard` — authenticated
   *  discovery variant. Issued through the JSON-RPC envelope against
   *  `POST /a2a` (the A2A spec defines no REST
   *  counterpart). The returned card has
   *  `capabilities.extendedAgentCard = true`. */
  async a2aGetAuthenticatedExtendedCard(
  ): Promise<Record<string, unknown>> {
    const envelope = {
      jsonrpc: "2.0",
      id: 1,
      method: "agent/getAuthenticatedExtendedCard",
    };
    const response = await this.request(
      "POST",
      `/a2a`,
      { body: envelope, extraHeaders: { ...A2A_HEADERS } },
    );
    if (!response.ok) await this.a2aThrowFromResponse(response);
    const body = (await response.json()) as JsonRpcReply<Record<string, unknown>>;
    return unwrapJsonRpc(body, (code, message) => {
      throw new ApiError(code, message, false);
    });
  }
}
