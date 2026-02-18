/**
 * Acteon Node.js/TypeScript Client
 *
 * HTTP client for the Acteon action gateway.
 *
 * @example
 * ```typescript
 * import { ActeonClient, createAction } from "@acteon/client";
 *
 * const client = new ActeonClient("http://localhost:8080");
 *
 * const action = createAction(
 *   "notifications",
 *   "tenant-1",
 *   "email",
 *   "send_notification",
 *   { to: "user@example.com", subject: "Hello" }
 * );
 *
 * const outcome = await client.dispatch(action);
 * console.log(`Outcome: ${outcome.type}`);
 * ```
 */

export { ActeonClient, type ActeonClientOptions } from "./client.js";
export { ActeonError, ConnectionError, HttpError, ApiError } from "./errors.js";
export {
  type Action,
  type ActionOutcome,
  type ActionError,
  type ProviderResponse,
  type BatchResult,
  type ErrorResponse,
  type RuleInfo,
  type ReloadResult,
  type EvaluateRulesRequest,
  type SemanticMatchDetail,
  type RuleTraceEntry,
  type EvaluateRulesResponse,
  type AuditQuery,
  type AuditRecord,
  type AuditPage,
  type EventQuery,
  type EventState,
  type EventListResponse,
  type TransitionResponse,
  type GroupSummary,
  type GroupListResponse,
  type GroupDetail,
  type FlushGroupResponse,
  type ApprovalActionResponse,
  type ApprovalStatus,
  type ApprovalListResponse,
  createAction,
  type WebhookPayload,
  createWebhookAction,
  type CreateRecurringAction,
  type CreateRecurringResponse,
  type RecurringFilter,
  type RecurringSummary,
  type ListRecurringResponse,
  type RecurringDetail,
  type UpdateRecurringAction,
  type CreateQuotaRequest,
  type UpdateQuotaRequest,
  type QuotaPolicy,
  type ListQuotasResponse,
  type QuotaUsage,
  type CreateRetentionRequest,
  type UpdateRetentionRequest,
  type RetentionPolicy,
  type ListRetentionResponse,
  type ChainSummary,
  type ListChainsResponse,
  type ChainStepStatus,
  type ChainDetailResponse,
  type DagNode,
  type DagEdge,
  type DagResponse,
  type DlqStatsResponse,
  type DlqEntry,
  type DlqDrainResponse,
  type SseEvent,
  type SubscribeOptions,
  type StreamOptions,
  type WasmPluginConfig,
  type WasmPlugin,
  type RegisterPluginRequest,
  type ListPluginsResponse,
  type PluginInvocationRequest,
  type PluginInvocationResponse,
  type ComplianceMode,
  type ComplianceStatus,
  type HashChainVerification,
  type VerifyHashChainRequest,
} from "./models.js";
