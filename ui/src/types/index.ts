// ---- Metrics ----
export interface MetricsResponse {
  dispatched: number
  executed: number
  deduplicated: number
  suppressed: number
  rerouted: number
  throttled: number
  failed: number
  pending_approval?: number
  chains_started: number
  chains_completed: number
  chains_failed: number
  chains_cancelled: number
  circuit_open?: number
  circuit_transitions?: number
  circuit_fallbacks?: number
  scheduled?: number
  llm_guardrail_allowed?: number
  llm_guardrail_denied?: number
  llm_guardrail_errors?: number
  recurring_dispatched?: number
  recurring_errors?: number
  recurring_skipped?: number
  quota_exceeded?: number
  quota_warned?: number
  quota_degraded?: number
  quota_notified?: number
  retention_deleted_state?: number
  retention_skipped_compliance?: number
  retention_errors?: number
  wasm_invocations?: number
  wasm_errors?: number
  embedding?: EmbeddingMetrics
}

export interface EmbeddingMetrics {
  similarity_requests: number
  similarity_errors: number
  topic_cache_hits: number
  topic_cache_misses: number
  text_cache_hits: number
  text_cache_misses: number
}

export interface HealthResponse {
  status: string
  metrics: MetricsResponse
}

// ---- Attachments ----
export interface Attachment {
  id: string
  name: string
  filename: string
  content_type: string
  data_base64: string
}

// ---- Dispatch ----
export interface DispatchRequest {
  namespace: string
  tenant: string
  provider: string
  action_type: string
  payload: Record<string, unknown>
  metadata?: Record<string, string>
  dedup_key?: string
  fingerprint?: string
  status?: string
  starts_at?: string
  ends_at?: string
  attachments?: Attachment[]
}

export interface DispatchResponse {
  action_id: string
  outcome: string
  details: Record<string, unknown> | null
}

// ---- Rules ----
export interface RuleSummary {
  name: string
  priority: number
  description?: string
  enabled: boolean
}

// ---- Audit ----
export interface AuditRecord {
  id: string
  action_id: string
  chain_id?: string
  namespace: string
  tenant: string
  provider: string
  action_type: string
  verdict: string
  matched_rule?: string
  outcome: string
  action_payload?: Record<string, unknown>
  verdict_details: Record<string, unknown>
  outcome_details: Record<string, unknown>
  metadata: Record<string, string>
  dispatched_at: string
  completed_at: string
  duration_ms: number
  expires_at?: string
  caller_id: string
  auth_method: string
  record_hash?: string
  previous_hash?: string
  sequence_number?: number
  attachment_metadata?: Record<string, unknown>[]
}

export interface AuditPage {
  records: AuditRecord[]
  total: number
  limit: number
  offset: number
}

export interface AuditQuery {
  namespace?: string
  tenant?: string
  provider?: string
  action_type?: string
  outcome?: string
  verdict?: string
  matched_rule?: string
  caller_id?: string
  chain_id?: string
  from?: string
  to?: string
  limit?: number
  offset?: number
}

export interface ReplayResult {
  original_action_id: string
  new_action_id: string
  success: boolean
  error?: string
}

export interface ReplaySummary {
  replayed: number
  failed: number
  skipped: number
  results: ReplayResult[]
}

// ---- Chains ----
export interface ChainSummary {
  chain_id: string
  chain_name: string
  status: string
  current_step: number
  total_steps: number
  started_at: string
  updated_at: string
}

export interface ChainStepStatus {
  name: string
  provider: string
  status: string
  response_body?: Record<string, unknown>
  error?: string
  completed_at?: string
  sub_chain?: string
  child_chain_id?: string
  parallel_sub_steps?: ChainStepStatus[]
}

export interface ChainDetailResponse {
  chain_id: string
  chain_name: string
  status: string
  current_step: number
  total_steps: number
  steps: ChainStepStatus[]
  started_at: string
  updated_at: string
  expires_at?: string
  cancel_reason?: string
  cancelled_by?: string
  execution_path: string[]
  parent_chain_id?: string
  child_chain_ids?: string[]
}

export interface BranchCondition {
  field: string
  operator: 'Eq' | 'Neq' | 'Contains' | 'Exists'
  value?: unknown
  target: string
}

export type ParallelJoinPolicy = 'all' | 'any'
export type ParallelFailurePolicy = 'fail_fast' | 'best_effort'

export interface ParallelStepGroup {
  steps: ChainStepConfig[]
  join: ParallelJoinPolicy
  on_failure: ParallelFailurePolicy
  timeout_seconds?: number
}

export interface ChainStepConfig {
  name: string
  provider: string
  action_type: string
  payload_template: Record<string, unknown>
  on_failure?: string
  delay_seconds?: number
  branches: BranchCondition[]
  default_next?: string
  sub_chain?: string
  parallel?: ParallelStepGroup
}

// ---- Chain DAG ----
export interface DagNode {
  name: string
  node_type: 'step' | 'sub_chain' | 'parallel'
  provider?: string
  action_type?: string
  sub_chain_name?: string
  status?: string
  child_chain_id?: string
  children?: DagResponse
  parallel_children?: DagNode[]
  parallel_join?: string
}

export interface DagEdge {
  source: string
  target: string
  label?: string
  on_execution_path: boolean
}

export interface DagResponse {
  chain_name: string
  chain_id?: string
  status?: string
  nodes: DagNode[]
  edges: DagEdge[]
  execution_path: string[]
}

// ---- Approvals ----
export interface ApprovalStatus {
  token: string
  status: string
  rule: string
  message?: string
  created_at: string
  expires_at: string
  decided_at?: string
}

// ---- Circuit Breakers ----
export interface CircuitBreakerStatus {
  provider: string
  state: 'closed' | 'open' | 'half_open'
  failure_threshold: number
  success_threshold: number
  recovery_timeout_seconds: number
  fallback_provider?: string
}

// ---- Provider Health ----
export interface ProviderHealthStatus {
  provider: string
  healthy: boolean
  health_check_error?: string
  circuit_breaker_state?: string
  total_requests: number
  successes: number
  failures: number
  success_rate: number
  avg_latency_ms: number
  p50_latency_ms: number
  p95_latency_ms: number
  p99_latency_ms: number
  last_request_at?: number
  last_error?: string
}

// ---- DLQ ----
export interface DlqStats {
  enabled: boolean
  count: number
}

// ---- Events ----
export interface EventState {
  fingerprint: string
  state: string
  action_type?: string
  updated_at?: string
}

// ---- Groups ----
export interface EventGroup {
  group_id: string
  group_key: string
  event_count: number
  state: string
  notify_at: string
  created_at: string
}

// ---- Stream ----
export interface StreamEvent {
  id: string
  timestamp: string
  event_type: string
  namespace: string
  tenant: string
  action_type: string
  action_id: string
}

// ---- Auth ----
export interface LoginRequest {
  username: string
  password: string
}

export interface LoginResponse {
  token: string
}

// ---- Similarity ----
export interface SimilarityRequest {
  text: string
  topic: string
}

export interface SimilarityResponse {
  similarity: number
  topic: string
}

// ---- Recurring Actions ----

/** Summary returned in list responses. */
export interface RecurringActionSummary {
  id: string
  namespace: string
  tenant: string
  cron_expr: string
  timezone: string
  enabled: boolean
  provider: string
  action_type: string
  next_execution_at: string | null
  execution_count: number
  description: string | null
  created_at: string
}

/** Full detail returned by GET /v1/recurring/:id. */
export interface RecurringAction extends RecurringActionSummary {
  payload: Record<string, unknown>
  metadata: Record<string, string>
  dedup_key: string | null
  last_executed_at: string | null
  ends_at: string | null
  max_executions: number | null
  labels: Record<string, string>
  updated_at: string
}

export interface RecurringActionListResponse {
  recurring_actions: RecurringActionSummary[]
  count: number
}

export interface CreateRecurringActionRequest {
  namespace: string
  tenant: string
  cron_expression: string
  timezone: string
  provider: string
  action_type: string
  payload: Record<string, unknown>
  metadata?: Record<string, string>
  dedup_key?: string | null
  description?: string | null
  ends_at?: string | null
  max_executions?: number | null
  enabled?: boolean
}

export interface CreateRecurringActionResponse {
  id: string
  name: string | null
  next_execution_at: string | null
  status: string
}

export interface UpdateRecurringActionRequest {
  namespace: string
  tenant: string
  name?: string | null
  cron_expression?: string
  timezone?: string
  enabled?: boolean
  provider?: string
  action_type?: string
  payload?: Record<string, unknown>
  metadata?: Record<string, string>
  dedup_key?: string | null
  description?: string | null
  max_executions?: number | null
  ends_at?: string | null
}

export interface PauseResumeResponse {
  id: string
  enabled: boolean
  next_execution_at: string | null
}

// ---- Rule Playground ----

export interface EvaluateRulesRequest {
  namespace: string
  tenant: string
  provider: string
  action_type: string
  payload: Record<string, unknown>
  metadata?: Record<string, string>
  include_disabled?: boolean
  evaluate_all?: boolean
  evaluate_at?: string | null
  mock_state?: Record<string, string>
}

export interface SemanticMatchDetail {
  extracted_text: string
  topic: string
  similarity: number
  threshold: number
}

export interface RuleTraceEntry {
  rule_name: string
  priority: number
  enabled: boolean
  condition_display: string
  result: 'matched' | 'not_matched' | 'skipped' | 'error'
  evaluation_duration_us: number
  action: string
  source: string
  description?: string
  skip_reason?: string
  error?: string
  semantic_details?: SemanticMatchDetail
  modify_patch?: Record<string, unknown>
  modified_payload_preview?: Record<string, unknown>
  wasm_details?: WasmTraceDetails
}

export interface WasmTraceDetails {
  plugin: string
  function: string
  verdict: boolean
  message?: string
  duration_us: number
  memory_used_bytes?: number
}

export interface EvaluateRulesResponse {
  verdict: string
  matched_rule?: string
  has_errors: boolean
  total_rules_evaluated: number
  total_rules_skipped: number
  evaluation_duration_us: number
  trace: RuleTraceEntry[]
  context: {
    time: Record<string, unknown>
    environment_keys: string[]
    accessed_state_keys?: string[]
    effective_timezone?: string
  }
  modified_payload?: Record<string, unknown>
}

// ---- WASM Plugins ----

export interface WasmPlugin {
  name: string
  description: string | null
  enabled: boolean
  memory_limit_bytes: number
  timeout_ms: number
  invocation_count: number
  last_invoked_at: string | null
  registered_at: string
}

export interface WasmPluginListResponse {
  plugins: WasmPlugin[]
  total: number
}

export interface WasmTestRequest {
  function: string
  input: Record<string, unknown>
}

export interface WasmTestResponse {
  verdict: boolean
  message: string | null
  metadata: Record<string, unknown> | null
  duration_us: number
}

// ---- Config ----
export interface ConfigResponse {
  server: {
    host: string
    port: number
    shutdown_timeout_seconds: number
    external_url: string | null
    max_sse_connections_per_tenant: number | null
  }
  state: {
    backend: string
    has_url: boolean
    prefix: string | null
    region: string | null
    table_name: string | null
  }
  executor: {
    max_retries: number | null
    timeout_seconds: number | null
    max_concurrent: number | null
    dlq_enabled: boolean
  }
  rules: {
    directory: string | null
    default_timezone: string | null
  }
  audit: {
    enabled: boolean
    backend: string
    has_url: boolean
    prefix: string
    ttl_seconds: number | null
    cleanup_interval_seconds: number
    store_payload: boolean
    redact: { enabled: boolean; field_count: number; placeholder: string }
  }
  auth: {
    enabled: boolean
    config_path: string | null
    watch: boolean | null
  }
  rate_limit: {
    enabled: boolean
    config_path: string | null
    on_error: string
  }
  llm_guardrail: {
    enabled: boolean
    endpoint: string
    model: string
    has_api_key: boolean
    policy: string
    policy_keys: string[]
    fail_open: boolean
    timeout_seconds: number | null
    temperature: number | null
    max_tokens: number | null
  }
  embedding: {
    enabled: boolean
    endpoint: string
    model: string
    has_api_key: boolean
    timeout_seconds: number
    fail_open: boolean
    topic_cache_capacity: number
    topic_cache_ttl_seconds: number
    text_cache_capacity: number
    text_cache_ttl_seconds: number
  }
  circuit_breaker: {
    enabled: boolean
    failure_threshold: number
    success_threshold: number
    recovery_timeout_seconds: number
    provider_overrides: string[]
  }
  background: {
    enabled: boolean
    enable_group_flush: boolean
    enable_timeout_processing: boolean
    enable_approval_retry: boolean
    enable_scheduled_actions: boolean
    enable_recurring_actions: boolean
    enable_retention_reaper: boolean
    enable_template_sync: boolean
    group_flush_interval_seconds: number
    timeout_check_interval_seconds: number
    cleanup_interval_seconds: number
    scheduled_check_interval_seconds: number
    recurring_check_interval_seconds: number
    retention_check_interval_seconds: number
    template_sync_interval_seconds: number
    max_recurring_actions_per_tenant: number
  }
  telemetry: {
    enabled: boolean
    endpoint: string
    service_name: string
    sample_ratio: number
    protocol: string
    timeout_seconds: number
    resource_attribute_keys: string[]
  }
  chains: {
    max_concurrent_advances: number
    completed_chain_ttl_seconds: number
    definitions: Array<{ name: string; steps_count: number; timeout_seconds: number | null }>
  }
  providers: Array<{ name: string; provider_type: string; url: string | null; header_count: number; has_token?: boolean; has_auth_token?: boolean; has_webhook_url?: boolean; email_backend?: string; aws_region?: string }>
  ui: { enabled: boolean; dist_path: string }
  encryption: { enabled: boolean }
  wasm: { enabled: boolean; plugin_dir: string | null; default_memory_limit_bytes: number; default_timeout_ms: number }
  compliance: { mode: string; immutable_audit: boolean; hash_chain: boolean; sync_audit_writes: boolean }
  attachments: { max_attachments_per_action: number; max_inline_bytes: number }
}

// ---- Quotas ----

export type QuotaWindow = 'hourly' | 'daily' | 'weekly' | 'monthly'
export type OverageBehavior = 'block' | 'warn' | 'degrade' | 'notify'

export interface QuotaPolicy {
  id: string
  namespace: string
  tenant: string
  max_actions: number
  window: QuotaWindow
  overage_behavior: OverageBehavior
  enabled: boolean
  description: string | null
  labels: Record<string, string>
  created_at: string
  updated_at: string
}

export interface QuotaUsage {
  tenant: string
  namespace: string
  used: number
  limit: number
  remaining: number
  window: QuotaWindow
  resets_at: string
  overage_behavior: OverageBehavior
}

export interface QuotaListResponse {
  quotas: QuotaPolicy[]
  count: number
}

export interface CreateQuotaRequest {
  namespace: string
  tenant: string
  max_actions: number
  window: QuotaWindow
  overage_behavior: OverageBehavior
  enabled?: boolean
  description?: string | null
  labels?: Record<string, string>
}

export interface UpdateQuotaRequest {
  max_actions?: number
  window?: QuotaWindow
  overage_behavior?: OverageBehavior
  enabled?: boolean
  description?: string | null
  labels?: Record<string, string>
}

export interface CreateQuotaResponse {
  id: string
}

// ---- Retention Policies ----

export interface RetentionPolicy {
  id: string
  namespace: string
  tenant: string
  enabled: boolean
  audit_ttl_seconds: number | null
  state_ttl_seconds: number | null
  event_ttl_seconds: number | null
  compliance_hold: boolean
  created_at: string
  updated_at: string
  description: string | null
  labels: Record<string, string>
}

export interface RetentionListResponse {
  policies: RetentionPolicy[]
  count: number
}

export interface CreateRetentionRequest {
  namespace: string
  tenant: string
  audit_ttl_seconds?: number | null
  state_ttl_seconds?: number | null
  event_ttl_seconds?: number | null
  compliance_hold?: boolean
  enabled?: boolean
  description?: string | null
  labels?: Record<string, string>
}

export interface UpdateRetentionRequest {
  audit_ttl_seconds?: number | null
  state_ttl_seconds?: number | null
  event_ttl_seconds?: number | null
  compliance_hold?: boolean
  enabled?: boolean
  description?: string | null
  labels?: Record<string, string>
}

export interface CreateRetentionResponse {
  id: string
}

// ---- Payload Templates ----

/**
 * A profile field is either an inline string value or a $ref pointing to
 * another template by name.
 */
export type TemplateProfileField = string | { $ref: string }

export interface Template {
  id: string
  name: string
  namespace: string
  tenant: string
  content: string
  description: string | null
  created_at: string
  updated_at: string
  labels: Record<string, string>
}

export interface TemplateProfile {
  id: string
  name: string
  namespace: string
  tenant: string
  fields: Record<string, TemplateProfileField>
  description: string | null
  created_at: string
  updated_at: string
  labels: Record<string, string>
}

export interface TemplateListResponse {
  templates: Template[]
  count: number
}

export interface TemplateProfileListResponse {
  profiles: TemplateProfile[]
  count: number
}

export interface CreateTemplateRequest {
  name: string
  namespace: string
  tenant: string
  content: string
  description?: string | null
  labels?: Record<string, string>
}

export interface UpdateTemplateRequest {
  content?: string
  description?: string | null
  labels?: Record<string, string>
}

export interface CreateTemplateResponse {
  id: string
}

export interface CreateProfileRequest {
  name: string
  namespace: string
  tenant: string
  fields: Record<string, TemplateProfileField>
  description?: string | null
  labels?: Record<string, string>
}

export interface UpdateProfileRequest {
  fields?: Record<string, TemplateProfileField>
  description?: string | null
  labels?: Record<string, string>
}

export interface CreateProfileResponse {
  id: string
}

export interface RenderPreviewRequest {
  profile: string
  namespace: string
  tenant: string
  payload: Record<string, unknown>
}

export interface RenderPreviewResponse {
  rendered: Record<string, string>
}

export interface TemplateQueryParams {
  namespace?: string
  tenant?: string
}

// ---- Compliance ----

export type ComplianceMode = 'none' | 'soc2' | 'hipaa'

export interface ComplianceStatus {
  mode: ComplianceMode
  sync_audit_writes: boolean
  immutable_audit: boolean
  hash_chain: boolean
}

export interface HashChainVerification {
  valid: boolean
  records_checked: number
  first_broken_at: string | null
  first_record_id: string | null
  last_record_id: string | null
}

// ---- Analytics ----

export type AnalyticsMetric = 'volume' | 'outcome_breakdown' | 'top_action_types' | 'latency' | 'error_rate';
export type AnalyticsInterval = 'hourly' | 'daily' | 'weekly' | 'monthly';

export interface AnalyticsQuery {
  metric: AnalyticsMetric;
  namespace?: string;
  tenant?: string;
  provider?: string;
  action_type?: string;
  outcome?: string;
  interval?: AnalyticsInterval;
  from?: string;
  to?: string;
  group_by?: string;
  top_n?: number;
}

export interface AnalyticsBucket {
  timestamp: string;
  count: number;
  group?: string;
  avg_duration_ms?: number;
  p50_duration_ms?: number;
  p95_duration_ms?: number;
  p99_duration_ms?: number;
  error_rate?: number;
}

export interface AnalyticsTopEntry {
  label: string;
  count: number;
  percentage: number;
}

export interface AnalyticsResponse {
  metric: AnalyticsMetric;
  interval: AnalyticsInterval;
  from: string;
  to: string;
  buckets: AnalyticsBucket[];
  top_entries: AnalyticsTopEntry[];
  total_count: number;
}
